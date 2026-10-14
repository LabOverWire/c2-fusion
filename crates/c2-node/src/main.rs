use std::collections::BTreeMap;
use std::error::Error;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use c2_codec::Registry;
use c2_exchange::{ingest_wire_with, picture, Picture};
use c2_lora::{Config, Modulation};
use c2_model::C2Message;
use mqp2p::{Peer, PeerConfig};
use sha2::{Digest, Sha256};
use stitch_p2p::{PeerId, Store, Swarm, SyncNode};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

mod events;
#[cfg(target_os = "linux")]
mod gnss;
mod lora;
mod telemetry;

const RETRY: Duration = Duration::from_secs(1);
const BROKER_RETRY_MAX: Duration = Duration::from_secs(60);
const BROKER_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn env_req(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| format!("missing required env var {key}").into())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_parse<T>(key: &str, default: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    env_or(key, default)
        .parse()
        .map_err(|error| format!("invalid {key}: {error}").into())
}

fn lora_settings() -> Result<Option<lora::LoraSettings>> {
    let Ok(port) = std::env::var("C2_LORA_PORT") else {
        return Ok(None);
    };
    Ok(Some(lora::LoraSettings {
        port,
        reset_gpio: env_parse("C2_LORA_RESET_GPIO", "470")?,
        config: Config {
            address: env_req("C2_LORA_ADDRESS")?.parse()?,
            network: env_parse("C2_LORA_NETWORK", "6")?,
            band_hz: env_parse("C2_LORA_BAND_HZ", "915000000")?,
            power_dbm: env_parse("C2_LORA_POWER_DBM", "15")?,
            modulation: Modulation {
                spreading_factor: env_parse("C2_LORA_SPREADING_FACTOR", "9")?,
                bandwidth: env_parse("C2_LORA_BANDWIDTH_KHZ", "125")?,
                coding_rate: 1,
                preamble: 4,
            },
        },
    }))
}

#[cfg(target_os = "linux")]
fn gnss_settings() -> Result<Option<gnss::GnssSettings>> {
    let Ok(port) = std::env::var("C2_GNSS_PORT") else {
        return Ok(None);
    };
    Ok(Some(gnss::GnssSettings {
        port,
        baud_rate: env_parse("C2_GNSS_BAUD", "115200")?,
        pps_chip: env_or("C2_PPS_CHIP", "/dev/gpiochip0"),
        pps_line: env_parse("C2_PPS_LINE", "20")?,
        position_interval: Duration::from_secs(env_parse("C2_POSITION_INTERVAL_S", "60")?),
    }))
}

#[cfg(not(target_os = "linux"))]
fn reject_gnss(name: &str) -> Result<()> {
    match std::env::var("C2_GNSS_PORT") {
        Ok(port) => {
            Err(format!("{name}: C2_GNSS_PORT={port} needs linux for pps and clock").into())
        }
        Err(_) => Ok(()),
    }
}

async fn broker_reachable(broker: &str) -> bool {
    matches!(
        timeout(BROKER_PROBE_TIMEOUT, TcpStream::connect(broker)).await,
        Ok(Ok(_))
    )
}

async fn connect_and_register(settings: &MeshSettings) -> Peer {
    let MeshSettings {
        name,
        broker,
        bind,
        credentials,
        segmentation_offload,
        lease_secs,
        ..
    } = settings;
    let mut delay = RETRY;
    let mut peer = loop {
        if broker_reachable(broker).await {
            let mut config = PeerConfig::new(name.as_str(), broker.as_str())
                .without_stun()
                .with_bind_addr(*bind)
                .with_lease_secs(*lease_secs);
            if !segmentation_offload {
                config = config.without_segmentation_offload();
            }
            if let Some((user, pass)) = credentials {
                config = config.with_credentials(user.clone(), pass.clone());
            }
            match Peer::new(config).await {
                Ok(peer) => break peer,
                Err(e) => eprintln!(
                    "broker connect failed for {name}: {e} retry_s={}",
                    delay.as_secs()
                ),
            }
        } else {
            eprintln!(
                "broker unreachable for {name}: {broker} retry_s={}",
                delay.as_secs()
            );
        }
        sleep(delay).await;
        delay = (delay * 2).min(BROKER_RETRY_MAX);
    };
    loop {
        match peer.register().await {
            Ok(id) => {
                println!("REGISTERED name={name} peer_id={id} broker={broker}");
                return peer;
            }
            Err(e) => eprintln!(
                "register failed for {name}: {e} retry_s={}",
                delay.as_secs()
            ),
        }
        sleep(delay).await;
        delay = (delay * 2).min(BROKER_RETRY_MAX);
    }
}

fn node_identity(name: &str) -> PeerId {
    Sha256::digest(name.as_bytes()).into()
}

fn open_store(name: &str, dir: Option<&str>) -> Result<Store> {
    let dir = dir.ok_or(
        "missing required env var C2_STORE_DIR: a node restarted without its store reuses write sequence numbers its peers already hold, and they drop its new writes",
    )?;
    std::fs::create_dir_all(dir).map_err(|e| format!("C2_STORE_DIR={dir}: {e}"))?;
    Store::open(node_identity(name), dir).map_err(|e| format!("C2_STORE_DIR={dir}: {e}").into())
}

struct MeshSettings {
    name: String,
    broker: String,
    bind: SocketAddr,
    credentials: Option<(String, String)>,
    segmentation_offload: bool,
    pull: Duration,
    discovery: Duration,
    lease_secs: u64,
}

async fn join_mesh(
    settings: MeshSettings,
    node: SyncNode,
    joined: watch::Sender<Option<Arc<Peer>>>,
) -> Swarm {
    let peer = Arc::new(connect_and_register(&settings).await);
    joined.send_replace(Some(Arc::clone(&peer)));
    Swarm::spawn_with_discovery_interval(peer, node, settings.pull, settings.discovery)
}

async fn take_inbox_file(store: &Store, registry: &Registry, name: &str, fmt: &str, path: &Path) {
    let file = path.display();
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("INBOX name={name} file={file} read-error={e}");
            return;
        }
    };
    let extension = match ingest_wire_with(store, registry, fmt, &bytes).await {
        Ok(msg) => {
            println!("INBOX name={name} ingested id={}", msg.id());
            "done"
        }
        Err(e) => {
            eprintln!("INBOX name={name} file={file} rejected error={e}");
            "rejected"
        }
    };
    if let Err(e) = tokio::fs::rename(path, path.with_extension(extension)).await {
        eprintln!("INBOX name={name} file={file} rename-error={e}");
    }
}

async fn scan_inbox(store: &Store, registry: &Registry, name: &str, dir: &str, fmt: &str) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("INBOX name={name} dir={dir} error={e}");
            return;
        }
    };
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    take_inbox_file(store, registry, name, fmt, &path).await;
                }
            }
            Ok(None) => return,
            Err(e) => {
                eprintln!("INBOX name={name} dir={dir} scan-error={e}");
                return;
            }
        }
    }
}

async fn run_inbox(store: Arc<Store>, name: String, dir: String, fmt: String) {
    let registry = Registry::with_defaults();
    loop {
        scan_inbox(&store, &registry, &name, &dir, &fmt).await;
        sleep(RETRY).await;
    }
}

fn picture_digest(entries: &BTreeMap<&String, &C2Message>) -> Result<[u8; 4]> {
    let digest = Sha256::digest(serde_json::to_vec(entries)?);
    Ok([digest[0], digest[1], digest[2], digest[3]])
}

fn picture_line(name: &str, picture: &Picture) -> Result<String> {
    let entries: BTreeMap<&String, &C2Message> = picture.messages.iter().collect();
    let digest: String = picture_digest(&entries)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let ids: Vec<&str> = entries.keys().map(|id| id.as_str()).collect();
    let unreadable = match picture.unreadable.is_empty() {
        true => String::new(),
        false => format!(
            " unreadable={}",
            picture
                .unreadable
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    Ok(format!(
        "PICTURE name={name} count={} digest={digest} ids={}{unreadable}",
        ids.len(),
        ids.join(",")
    ))
}

async fn run_printer(store: Arc<Store>, name: String, interval: Duration) {
    loop {
        match picture_line(&name, &picture(&store).await) {
            Ok(line) => println!("{line}"),
            Err(error) => eprintln!("PICTURE name={name} digest-error={error}"),
        }
        sleep(interval).await;
    }
}

#[cfg(unix)]
async fn terminated() -> std::io::Result<()> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
        .recv()
        .await;
    Ok(())
}

#[cfg(not(unix))]
async fn terminated() -> std::io::Result<()> {
    std::future::pending().await
}

fn supervise(
    tasks: &mut JoinSet<&'static str>,
    task: &'static str,
    work: impl Future<Output = ()> + Send + 'static,
) {
    tasks.spawn(async move {
        work.await;
        task
    });
}

async fn stop_reason(tasks: &mut JoinSet<&'static str>) -> Result<&'static str> {
    tokio::select! {
        interrupted = tokio::signal::ctrl_c() => interrupted.map(|()| "SIGINT").map_err(Into::into),
        terminated = terminated() => terminated.map(|()| "SIGTERM").map_err(Into::into),
        Some(ended) = tasks.join_next() => match ended {
            Ok(task) => Err(format!("{task} task ended").into()),
            Err(error) => Err(format!("task failed: {error}").into()),
        },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time()
        .with_max_level(env_parse::<tracing::Level>("C2_LOG", "warn")?)
        .init();
    let name = env_req("C2_NAME")?;
    let broker = env_req("C2_BROKER")?;
    let bind: SocketAddr = env_or("C2_LISTEN", "0.0.0.0:9000").parse()?;
    let interval = Duration::from_millis(env_or("C2_PICTURE_INTERVAL_MS", "1000").parse()?);

    let credentials = match (
        std::env::var("C2_BROKER_USER"),
        std::env::var("C2_BROKER_PASS"),
    ) {
        (Ok(user), Ok(pass)) => Some((user, pass)),
        _ => None,
    };
    let mut tasks = JoinSet::new();
    let store = Arc::new(open_store(
        &name,
        std::env::var("C2_STORE_DIR").ok().as_deref(),
    )?);

    let (joined, peer_handle) = watch::channel(None);
    let telemetry = telemetry::Telemetry::from_env(&name)?;
    if let Some(telemetry) = &telemetry {
        println!(
            "TELEMETRY name={name} otlp interval_ms={}",
            telemetry.interval().as_millis()
        );
        supervise(
            &mut tasks,
            "telemetry mutations",
            telemetry::run_mutations(store.subscribe().await, telemetry.metrics()),
        );
        supervise(
            &mut tasks,
            "telemetry sampler",
            telemetry::run_sampler(
                Arc::clone(&store),
                peer_handle,
                name.clone(),
                telemetry.metrics(),
                telemetry.interval(),
            ),
        );
    }

    if let Some(address) = std::env::var("C2_EVENTS_LISTEN")
        .ok()
        .filter(|address| !address.is_empty())
    {
        let listener = TcpListener::bind(&address).await?;
        println!(
            "EVENTS name={name} listen={} schema={}",
            listener.local_addr()?,
            events::SCHEMA
        );
        supervise(
            &mut tasks,
            "events",
            events::serve(
                listener,
                Arc::clone(&store),
                name.clone(),
                node_identity(&name),
                events::WRITE_TIMEOUT,
            ),
        );
    }

    if let Some(settings) = lora_settings()? {
        supervise(
            &mut tasks,
            "lora",
            lora::run(settings, store.node().clone(), name.clone()),
        );
    }

    #[cfg(target_os = "linux")]
    if let Some(settings) = gnss_settings()? {
        supervise(
            &mut tasks,
            "gnss",
            gnss::run(settings, name.clone(), Arc::clone(&store)),
        );
    }
    #[cfg(not(target_os = "linux"))]
    reject_gnss(&name)?;

    if let (Ok(fmt), Ok(file)) = (env_req("C2_INGEST_FMT"), env_req("C2_INGEST_FILE")) {
        let bytes = tokio::fs::read(&file)
            .await
            .map_err(|e| format!("C2_INGEST_FILE={file}: {e}"))?;
        let registry = Registry::with_defaults();
        let msg = ingest_wire_with(&store, &registry, &fmt, &bytes).await?;
        println!("INGEST name={name} id={} fmt={fmt}", msg.id());
    }

    let mesh = tokio::spawn(join_mesh(
        MeshSettings {
            name: name.clone(),
            broker,
            bind,
            credentials,
            segmentation_offload: env_parse("C2_QUIC_SEGMENTATION_OFFLOAD", "true")?,
            pull: Duration::from_millis(env_parse("C2_PULL_MS", "200")?),
            discovery: Duration::from_millis(env_parse("C2_DISCOVERY_MS", "2000")?),
            lease_secs: env_parse("C2_LEASE_SECS", "6")?,
        },
        store.node().clone(),
        joined,
    ));

    if let Ok(inbox) = env_req("C2_INBOX") {
        let fmt = env_or("C2_INBOX_FMT", "c2-json");
        supervise(
            &mut tasks,
            "inbox",
            run_inbox(Arc::clone(&store), name.clone(), inbox, fmt),
        );
    }
    supervise(
        &mut tasks,
        "printer",
        run_printer(Arc::clone(&store), name.clone(), interval),
    );

    let stopped = stop_reason(&mut tasks).await;
    match &stopped {
        Ok(signal) => println!("STOP name={name} signal={signal}"),
        Err(error) => eprintln!("STOP name={name} error={error}"),
    }
    tasks.shutdown().await;
    mesh.abort();
    if let Ok(swarm) = mesh.await {
        swarm.abort();
    }
    if let Some(telemetry) = telemetry {
        telemetry.shutdown()?;
    }
    stopped.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_exchange::{ingest, ENTITY};
    use c2_model::{ContactReport, Domain, FunctionalService};
    use stitch_p2p::Op;

    fn contact(id: &str) -> C2Message {
        C2Message::ContactReport(ContactReport {
            id: id.to_string(),
            reporting_unit: "UAV-7".to_string(),
            domain: Domain::Air,
            service: FunctionalService::Intelligence,
            observed_at: "2026-09-29T16:00:00Z".to_string(),
            latitude: 49.3,
            longitude: -123.1,
            description: "test contact".to_string(),
            label: None,
        })
    }

    fn json(msg: &C2Message) -> Vec<u8> {
        serde_json::to_vec(msg).expect("model json")
    }

    #[test]
    fn a_node_without_a_store_directory_refuses_to_start() {
        let error = open_store("edge", None).err().expect("refused");
        assert!(error.to_string().contains("C2_STORE_DIR"), "{error}");
    }

    #[tokio::test]
    async fn a_restarted_node_continues_its_write_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_dir = dir.path().join("var/lib/c2/store");
        let path = store_dir.to_str().expect("utf-8 path");
        {
            let store = open_store("edge", Some(path)).expect("open");
            ingest(&store, &contact("C-1")).await.expect("ingest");
        }
        sleep(Duration::from_millis(300)).await;
        let store = open_store("edge", Some(path)).expect("reopen");
        let frame = store
            .node()
            .local_write(Op::Insert, ENTITY, "C-2", json(&contact("C-2")))
            .await;
        assert_eq!(frame.seq, 2);
    }

    #[tokio::test]
    async fn inbox_marks_each_file_by_outcome() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inbox = dir.path();
        std::fs::write(inbox.join("good.json"), json(&contact("C-1"))).expect("write");
        std::fs::write(inbox.join("bad.json"), b"{\"message_type\":").expect("write");
        std::fs::write(inbox.join("partial.tmp"), b"{").expect("write");
        let store = Store::new(node_identity("edge"));
        let registry = Registry::with_defaults();

        scan_inbox(
            &store,
            &registry,
            "edge",
            inbox.to_str().expect("utf-8 path"),
            "c2-json",
        )
        .await;

        let mut names: Vec<String> = std::fs::read_dir(inbox)
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(names, ["bad.rejected", "good.done", "partial.tmp"]);
        assert_eq!(
            picture(&store)
                .await
                .messages
                .into_keys()
                .collect::<Vec<_>>(),
            ["C-1"]
        );
    }

    #[tokio::test]
    async fn picture_line_lists_unreadable_records() {
        let store = Store::new(node_identity("edge"));
        ingest(&store, &contact("C-1")).await.expect("ingest");
        let clean = picture_line("edge", &picture(&store).await).expect("line");
        assert!(clean.ends_with(" ids=C-1"), "{clean}");

        store
            .node()
            .local_write(Op::Insert, ENTITY, "BAD", b"{}".to_vec())
            .await;
        let line = picture_line("edge", &picture(&store).await).expect("line");
        assert_eq!(line, format!("{clean} unreadable=BAD"));
    }
}
