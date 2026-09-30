use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use c2_codec::Registry;
use c2_exchange::{ingest_wire_with, picture};
use mqp2p::{Peer, PeerConfig};
use stitch_p2p::{peer_id_from_fingerprint, Store, Swarm};
use tokio::time::sleep;

const PULL: Duration = Duration::from_millis(200);
const RETRY: Duration = Duration::from_secs(1);

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn env_req(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| format!("missing required env var {key}").into())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Connect to the MQDB signaling broker and register this peer, retrying until
/// the broker is reachable (it may start after the node).
async fn connect_and_register(
    name: &str,
    broker: &str,
    bind: SocketAddr,
    credentials: &Option<(String, String)>,
) -> Peer {
    loop {
        let mut config = PeerConfig::new(name, broker)
            .without_stun()
            .with_bind_addr(bind);
        if let Some((user, pass)) = credentials {
            config = config.with_credentials(user.clone(), pass.clone());
        }
        match Peer::new(config).await {
            Ok(mut peer) => match peer.register().await {
                Ok(id) => {
                    println!("REGISTERED name={name} peer_id={id} broker={broker}");
                    return peer;
                }
                Err(e) => eprintln!("register failed for {name}: {e}"),
            },
            Err(e) => eprintln!("broker connect failed for {name}: {e}"),
        }
        sleep(RETRY).await;
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
            println!(
                "PICTURE name={name} count={} ids={}",
                ids.len(),
                ids.join(",")
            );
        }
        sleep(interval).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
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
    let peer = connect_and_register(&name, &broker, bind, &credentials).await;
    let self_id = peer_id_from_fingerprint(peer.fingerprint())
        .ok_or_else(|| -> Box<dyn Error + Send + Sync> { "invalid peer fingerprint".into() })?;
    let store = Arc::new(Store::new(self_id));

    if let (Ok(fmt), Ok(file)) = (env_req("C2_INGEST_FMT"), env_req("C2_INGEST_FILE")) {
        let bytes = tokio::fs::read(&file).await?;
        let registry = Registry::with_defaults();
        let msg = ingest_wire_with(&store, &registry, &fmt, &bytes).await?;
        println!("INGEST name={name} id={} fmt={fmt}", msg.id());
    }

    // Discovery and sync: the Swarm discovers peers through MQDB and bridges each
    // connection into the sync session. Held until shutdown; Drop tears it down.
    let _swarm = Swarm::spawn(Arc::new(peer), store.node().clone(), PULL);

    let mut tasks = Vec::new();
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
