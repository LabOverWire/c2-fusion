use std::time::Duration;

use c2_exchange::{ingest, picture, ENTITY};
use c2_model::{C2Message, ContactReport, Domain, FunctionalService, Rfi, Sitrep};
use stitch_p2p::{session, Op, PeerId, Store, PEER_ID_LEN};
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
        label: None,
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
        label: None,
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
        label: None,
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
        picture(&edge).await.messages == picture(&hq).await.messages
            && picture(&edge).await.messages.len() == 2
    })
    .await;
    for h in &handles {
        h.abort();
    }

    assert!(converged, "nodes did not converge on the shared picture");
    let pic = picture(&edge).await.messages;
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
        picture(&hq).await.messages.contains_key("C-1")
    })
    .await;
    assert!(first, "initial replication failed before partition");

    for h in &handles {
        h.abort();
    }
    ingest(&edge, &contact("C-2")).await.unwrap();
    ingest(&hq, &rfi("R-1")).await.unwrap();

    assert!(!picture(&edge).await.messages.contains_key("R-1"));
    assert!(!picture(&hq).await.messages.contains_key("C-2"));

    let (edge_io2, hq_io2) = tokio::io::duplex(64 * 1024);
    let handles2 = connect(&edge, edge_io2, &hq, hq_io2);

    let converged = wait_until(Duration::from_secs(3), || async {
        let a = picture(&edge).await.messages;
        let b = picture(&hq).await.messages;
        a == b && a.len() == 3
    })
    .await;
    for h in &handles2 {
        h.abort();
    }

    assert!(converged, "picture did not converge after rejoin");
    let pic = picture(&hq).await.messages;
    assert!(pic.contains_key("C-1"));
    assert!(pic.contains_key("C-2"));
    assert!(pic.contains_key("R-1"));
}

#[tokio::test]
async fn picture_keeps_readable_messages_beside_unreadable_records() {
    let store = Store::new(peer(1));
    ingest(&store, &contact("C-1")).await.unwrap();
    store
        .create(ENTITY, "NOT-A-MESSAGE", serde_json::json!({"foo": 1}))
        .await
        .unwrap();
    store
        .node()
        .local_write(Op::Insert, ENTITY, "NOT-JSON", b"\xff\x00".to_vec())
        .await;

    let pic = picture(&store).await;
    assert_eq!(pic.messages.len(), 1);
    assert_eq!(pic.messages.get("C-1"), Some(&contact("C-1")));
    assert_eq!(
        pic.unreadable
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["NOT-A-MESSAGE", "NOT-JSON"]
    );
}

#[tokio::test]
async fn picture_treats_a_replicated_invalid_message_as_unreadable() {
    let store = Store::new(peer(1));
    ingest(&store, &contact("C-1")).await.unwrap();
    let mut forged = serde_json::to_value(contact("C-2")).unwrap();
    forged["latitude"] = serde_json::json!(999.0);
    store.create(ENTITY, "C-2", forged).await.unwrap();

    let pic = picture(&store).await;
    assert_eq!(pic.messages.len(), 1, "{:?}", pic.messages.keys());
    assert!(pic.messages.contains_key("C-1"));
    assert_eq!(
        pic.unreadable
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["C-2"],
        "a record that decodes but fails validation must not enter the picture"
    );
}
