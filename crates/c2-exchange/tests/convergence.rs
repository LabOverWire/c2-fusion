//! Shared-picture convergence across peer-to-peer nodes, including a link
//! partition and rejoin. Nodes exchange C2 messages over an in-memory pipe via
//! the stitch-p2p session protocol; the same mechanism runs over QUIC in the
//! DDIL harness. Convergence itself is machine-checked in the stitch-p2p TLA+
//! specs (InvConvergence); these tests exercise the C2 mapping on top of it.

use std::time::Duration;

use c2_exchange::{ingest, picture};
use c2_model::{ContactReport, C2Message, Domain, FunctionalService, Rfi, Sitrep};
use stitch_p2p::{session, Store, PeerId, PEER_ID_LEN};
use tokio::io::{split, DuplexStream};
use tokio::task::JoinHandle;

const PULL: Duration = Duration::from_millis(20);

fn peer(n: u8) -> PeerId {
    let mut id = [0u8; PEER_ID_LEN];
    id[0] = n;
    id
}

fn connect(a: &Store, a_io: DuplexStream, b: &Store, b_io: DuplexStream) -> Vec<JoinHandle<()>> {
    let rx_a = a.node().register_session();
    let (ar, aw) = split(a_io);
    let sa = a.node().state();
    let rx_b = b.node().register_session();
    let (br, bw) = split(b_io);
    let sb = b.node().state();
    vec![
        tokio::spawn(async move {
            let _ = session::run(sa, ar, aw, rx_a, PULL).await;
        }),
        tokio::spawn(async move {
            let _ = session::run(sb, br, bw, rx_b, PULL).await;
        }),
    ]
}

async fn wait_until<F, Fut>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if cond().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

fn contact(id: &str) -> C2Message {
    C2Message::ContactReport(ContactReport {
        id: id.to_string(),
        reporting_unit: "UAV-7".to_string(),
        domain: Domain::Air,
        service: FunctionalService::Intelligence,
        observed_at: "2026-09-29T14:30:00Z".to_string(),
        latitude: 49.2827,
        longitude: -123.1207,
        description: "Vehicle stationary".to_string(),
    })
}

fn sitrep(id: &str) -> C2Message {
    C2Message::Sitrep(Sitrep {
        id: id.to_string(),
        reporting_unit: "Coy-A".to_string(),
        domain: Domain::Land,
        service: FunctionalService::CommandAndControl,
        reported_at: "2026-09-29T15:00:00Z".to_string(),
        summary: "Holding position".to_string(),
        personnel_effective: 118,
    })
}

fn rfi(id: &str) -> C2Message {
    C2Message::Rfi(Rfi {
        id: id.to_string(),
        requesting_unit: "HQ-1".to_string(),
        domain: Domain::Land,
        service: FunctionalService::Intelligence,
        requested_at: "2026-09-29T15:10:00Z".to_string(),
        question: "Confirm bridge status".to_string(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_nodes_converge_on_shared_picture() {
    let edge = Store::new(peer(1));
    let hq = Store::new(peer(2));

    ingest(&edge, &contact("C-1")).await.unwrap();
    ingest(&hq, &sitrep("S-1")).await.unwrap();

    let (edge_io, hq_io) = tokio::io::duplex(64 * 1024);
    let handles = connect(&edge, edge_io, &hq, hq_io);

    let converged = wait_until(Duration::from_secs(3), || async {
        picture(&edge).await.unwrap() == picture(&hq).await.unwrap()
            && picture(&edge).await.unwrap().len() == 2
    })
    .await;
    for h in &handles {
        h.abort();
    }

    assert!(converged, "nodes did not converge on the shared picture");
    let pic = picture(&edge).await.unwrap();
    assert_eq!(pic.get("C-1"), Some(&contact("C-1")));
    assert_eq!(pic.get("S-1"), Some(&sitrep("S-1")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn picture_converges_after_partition_and_rejoin() {
    let edge = Store::new(peer(1));
    let hq = Store::new(peer(2));

    ingest(&edge, &contact("C-1")).await.unwrap();
    let (edge_io, hq_io) = tokio::io::duplex(64 * 1024);
    let handles = connect(&edge, edge_io, &hq, hq_io);

    let first = wait_until(Duration::from_secs(3), || async {
        picture(&hq).await.unwrap().contains_key("C-1")
    })
    .await;
    assert!(first, "initial replication failed before partition");

    // Partition: drop both sessions. Each node keeps writing while cut off.
    for h in &handles {
        h.abort();
    }
    ingest(&edge, &contact("C-2")).await.unwrap();
    ingest(&hq, &rfi("R-1")).await.unwrap();

    // During the partition neither node has the other's new write.
    assert!(!picture(&edge).await.unwrap().contains_key("R-1"));
    assert!(!picture(&hq).await.unwrap().contains_key("C-2"));

    // Rejoin over a fresh link.
    let (edge_io2, hq_io2) = tokio::io::duplex(64 * 1024);
    let handles2 = connect(&edge, edge_io2, &hq, hq_io2);

    let converged = wait_until(Duration::from_secs(3), || async {
        let a = picture(&edge).await.unwrap();
        let b = picture(&hq).await.unwrap();
        a == b && a.len() == 3
    })
    .await;
    for h in &handles2 {
        h.abort();
    }

    assert!(converged, "picture did not converge after rejoin");
    let pic = picture(&hq).await.unwrap();
    assert!(pic.contains_key("C-1"));
    assert!(pic.contains_key("C-2"));
    assert!(pic.contains_key("R-1"));
}
