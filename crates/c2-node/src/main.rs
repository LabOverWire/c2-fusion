use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use c2_codec::Registry;
use c2_exchange::{ingest_wire_with, picture};
use mqp2p::quic::{generate_self_signed_cert, QuicEndpoint};
use stitch_p2p::{session, PeerId, Store, PEER_ID_LEN};
use tokio::time::sleep;

const PULL: Duration = Duration::from_millis(50);
const RETRY: Duration = Duration::from_secs(1);

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn env_req(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| format!("missing required env var {key}").into())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Deterministic peer id derived from the node name (FNV-1a in the first 8 bytes),
/// so a node keeps its identity across restarts without a coordination service.
fn peer_id(name: &str) -> PeerId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut id = [0u8; PEER_ID_LEN];
    id[..8].copy_from_slice(&hash.to_be_bytes());
    id
}

async fn write_info(shared: &str, name: &str, fingerprint: &str, advertise: &str) -> Result<()> {
    let body = format!("{fingerprint}\n{advertise}\n");
    let tmp = format!("{shared}/{name}.info.tmp");
    let final_path = format!("{shared}/{name}.info");
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, &final_path).await?;
    Ok(())
}

async fn read_info(shared: &str, name: &str) -> Result<(String, String)> {
    let path = format!("{shared}/{name}.info");
    loop {
        if let Ok(body) = tokio::fs::read_to_string(&path).await {
            let mut lines = body.lines();
            if let (Some(fp), Some(advertise)) = (lines.next(), lines.next()) {
                return Ok((fp.to_string(), advertise.to_string()));
            }
        }
        sleep(RETRY).await;
    }
}

async fn resolve(advertise: &str) -> Result<SocketAddr> {
    tokio::net::lookup_host(advertise)
        .await?
        .next()
        .ok_or_else(|| format!("could not resolve {advertise}").into())
}

async fn run_accept(endpoint: Arc<QuicEndpoint>, store: Arc<Store>, peer_fp: String) {
    loop {
        match endpoint.accept_with_fingerprint(&peer_fp).await {
            Ok(conn) => match conn.accept_bi().await {
                Ok((send, recv)) => {
                    let state = store.node().state();
                    let rx = store.node().register_session();
                    tokio::spawn(async move {
                        let _ = session::run(state, recv, send, rx, PULL).await;
                    });
                }
                Err(_) => sleep(RETRY).await,
            },
            Err(_) => sleep(RETRY).await,
        }
    }
}

async fn run_dial(endpoint: Arc<QuicEndpoint>, store: Arc<Store>, fp: String, advertise: String) {
    loop {
        let connected = async {
            let addr = resolve(&advertise).await?;
            let conn = endpoint.connect(addr, &fp).await?;
            let (send, recv) = conn.open_bi().await?;
            let state = store.node().state();
            let rx = store.node().register_session();
            session::run(state, recv, send, rx, PULL)
                .await
                .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(e) })?;
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        }
        .await;
        if connected.is_err() {
            sleep(RETRY).await;
        }
    }
}

async fn run_inbox(store: Arc<Store>, name: String, dir: String, fmt: String) {
    let registry = Registry::with_defaults();
    loop {
        if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(bytes) = tokio::fs::read(&path).await {
                    match ingest_wire_with(&store, &registry, &fmt, &bytes).await {
                        Ok(msg) => println!("INBOX name={name} ingested id={}", msg.id()),
                        Err(e) => eprintln!("INBOX name={name} error={e}"),
                    }
                }
                let _ = tokio::fs::rename(&path, path.with_extension("done")).await;
            }
        }
        sleep(RETRY).await;
    }
}

async fn run_printer(store: Arc<Store>, name: String, interval: Duration) {
    loop {
        if let Ok(pic) = picture(&store).await {
            let mut ids: Vec<String> = pic.keys().cloned().collect();
            ids.sort();
            println!("PICTURE name={name} count={} ids={}", ids.len(), ids.join(","));
        }
        sleep(interval).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let name = env_req("C2_NAME")?;
    let listen: SocketAddr = env_or("C2_LISTEN", "0.0.0.0:9000").parse()?;
    let advertise = env_or("C2_ADVERTISE", &format!("{name}:9000"));
    let shared = env_or("C2_SHARED", "/shared");
    let interval = Duration::from_millis(env_or("C2_PICTURE_INTERVAL_MS", "1000").parse()?);

    let identity = generate_self_signed_cert()?;
    let fingerprint = identity.fingerprint.clone();
    let socket = std::net::UdpSocket::bind(listen)?;
    let endpoint = Arc::new(QuicEndpoint::bind(socket, identity)?);
    write_info(&shared, &name, &fingerprint, &advertise).await?;

    let store = Arc::new(Store::new(peer_id(&name)));

    if let (Ok(fmt), Ok(file)) = (env_req("C2_INGEST_FMT"), env_req("C2_INGEST_FILE")) {
        let bytes = tokio::fs::read(&file).await?;
        let registry = Registry::with_defaults();
        let msg = ingest_wire_with(&store, &registry, &fmt, &bytes).await?;
        println!("INGEST name={name} id={} fmt={fmt}", msg.id());
    }

    let mut tasks = Vec::new();
    if let Ok(accept_name) = env_req("C2_ACCEPT") {
        let (peer_fp, _) = read_info(&shared, &accept_name).await?;
        tasks.push(tokio::spawn(run_accept(
            Arc::clone(&endpoint),
            Arc::clone(&store),
            peer_fp,
        )));
    }
    if let Ok(dial_name) = env_req("C2_DIAL") {
        let (fp, advertise) = read_info(&shared, &dial_name).await?;
        tasks.push(tokio::spawn(run_dial(
            Arc::clone(&endpoint),
            Arc::clone(&store),
            fp,
            advertise,
        )));
    }
    if let Ok(inbox) = env_req("C2_INBOX") {
        let fmt = env_or("C2_INBOX_FMT", "niem-json");
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
    Ok(())
}
