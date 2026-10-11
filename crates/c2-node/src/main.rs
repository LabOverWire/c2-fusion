use std::collections::BTreeMap;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use c2_codec::Registry;
use c2_exchange::{ingest_wire_with, picture};
use c2_lora::{Bandwidth, Config, Modulation};
use c2_model::C2Message;
use mqp2p::{Peer, PeerConfig};
use sha2::{Digest, Sha256};
use stitch_p2p::{PeerId, Store, Swarm, SyncNode};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

#[cfg(target_os = "linux")]
mod gnss;
mod lora;

const PULL: Duration = Duration::from_millis(200);
const RETRY: Duration = Duration::from_secs(1);
const BROKER_RETRY_MAX: Duration = Duration::from_secs(60);
const BROKER_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const LEASE_SECS: u64 = 6;

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
                bandwidth: Bandwidth::Khz125,
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

async fn connect_and_register(
    name: &str,
    broker: &str,
    bind: SocketAddr,
    credentials: &Option<(String, String)>,
) -> Peer {
    let mut delay = RETRY;
    let mut peer = loop {
        if broker_reachable(broker).await {
            let mut config = PeerConfig::new(name, broker)
                .without_stun()
                .with_bind_addr(bind)
                .with_lease_secs(LEASE_SECS);
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

async fn join_mesh(
    name: String,
    broker: String,
    bind: SocketAddr,
    credentials: Option<(String, String)>,
    node: SyncNode,
) -> Swarm {
    let peer = connect_and_register(&name, &broker, bind, &credentials).await;
    Swarm::spawn(Arc::new(peer), node, PULL)
}

async fn run_inbox(store: Arc<Store>, name: String, dir: String, fmt: String) {
    let registry = Registry::with_defaults();
    loop {
        match tokio::fs::read_dir(&dir).await {
            Ok(mut rd) => {
                while let Ok(Some(entry)) = rd.next_entry().await {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    match tokio::fs::read(&path).await {
                        Ok(bytes) => {
                            match ingest_wire_with(&store, &registry, &fmt, &bytes).await {
                                Ok(msg) => println!("INBOX name={name} ingested id={}", msg.id()),
                                Err(e) => eprintln!("INBOX name={name} error={e}"),
                            }
                        }
                        Err(e) => eprintln!("INBOX name={name} read-error={e}"),
                    }
                    if let Err(e) = tokio::fs::rename(&path, path.with_extension("done")).await {
                        eprintln!("INBOX name={name} rename-error={e}");
                    }
                }
            }
            Err(e) => eprintln!("INBOX name={name} dir={dir} error={e}"),
        }
        sleep(RETRY).await;
    }
}

fn picture_digest(entries: &BTreeMap<&String, &C2Message>) -> Result<String> {
    let digest = Sha256::digest(serde_json::to_vec(entries)?);
    Ok(digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

async fn run_printer(store: Arc<Store>, name: String, interval: Duration) {
    loop {
        match picture(&store).await {
            Ok(pic) => {
                let entries: BTreeMap<&String, &C2Message> = pic.iter().collect();
                let ids: Vec<&str> = entries.keys().map(|id| id.as_str()).collect();
                match picture_digest(&entries) {
                    Ok(digest) => println!(
                        "PICTURE name={name} count={} digest={digest} ids={}",
                        ids.len(),
                        ids.join(",")
                    ),
                    Err(error) => eprintln!("PICTURE name={name} digest-error={error}"),
                }
            }
            Err(error) => eprintln!("PICTURE name={name} error={error}"),
        }
        sleep(interval).await;
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
    let mut tasks = Vec::new();
    let store = Arc::new(match std::env::var("C2_STORE_DIR") {
        Ok(dir) => Store::open(node_identity(&name), &dir)?,
        Err(_) => Store::new(node_identity(&name)),
    });

    if let Some(settings) = lora_settings()? {
        tasks.push(tokio::spawn(lora::run(
            settings,
            store.node().clone(),
            name.clone(),
        )));
    }

    #[cfg(target_os = "linux")]
    if let Some(settings) = gnss_settings()? {
        tasks.push(tokio::spawn(gnss::run(
            settings,
            name.clone(),
            Arc::clone(&store),
        )));
    }
    #[cfg(not(target_os = "linux"))]
    reject_gnss(&name)?;

    if let (Ok(fmt), Ok(file)) = (env_req("C2_INGEST_FMT"), env_req("C2_INGEST_FILE")) {
        let bytes = tokio::fs::read(&file).await?;
        let registry = Registry::with_defaults();
        let msg = ingest_wire_with(&store, &registry, &fmt, &bytes).await?;
        println!("INGEST name={name} id={} fmt={fmt}", msg.id());
    }

    let mesh = tokio::spawn(join_mesh(
        name.clone(),
        broker,
        bind,
        credentials,
        store.node().clone(),
    ));

    if let Ok(inbox) = env_req("C2_INBOX") {
        let fmt = env_or("C2_INBOX_FMT", "c2-json");
        tasks.push(tokio::spawn(run_inbox(
            Arc::clone(&store),
            name.clone(),
            inbox,
            fmt,
        )));
    }
    tasks.push(tokio::spawn(run_printer(
        Arc::clone(&store),
        name.clone(),
        interval,
    )));

    tokio::signal::ctrl_c().await?;
    for t in tasks {
        t.abort();
    }
    mesh.abort();
    if let Ok(swarm) = mesh.await {
        swarm.abort();
    }
    Ok(())
}
