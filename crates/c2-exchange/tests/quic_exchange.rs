//! Full stack over a real QUIC connection: two Stores exchange C2 messages via
//! the sync session running over mqp2p's fingerprint-pinned QUIC, then converge.
//!
//! This uses direct QUIC dial (`QuicEndpoint::connect` to a known address), not
//! the `Peer`/`Swarm` discovery path. Direct dial has no STUN and no UDP
//! hole-punching, which is what makes it work inside a container bridge network
//! where hole-punching fails. See the DDIL harness transport notes.

use std::net::UdpSocket;
use std::time::Duration;

use c2_exchange::{ingest, picture};
use c2_model::{ContactReport, C2Message, Domain, FunctionalService, Sitrep};
use mqp2p::quic::{generate_self_signed_cert, QuicEndpoint};
use stitch_p2p::{session, PeerId, Store, PEER_ID_LEN};

const PULL: Duration = Duration::from_millis(50);

fn peer(n: u8) -> PeerId {
    let mut id = [0u8; PEER_ID_LEN];
    id[0] = n;
    id
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_converge_over_real_quic() {
    let id_a = generate_self_signed_cert().unwrap();
    let id_b = generate_self_signed_cert().unwrap();
    let fp_a = id_a.fingerprint.clone();
    let fp_b = id_b.fingerprint.clone();

    let ep_a = QuicEndpoint::bind(UdpSocket::bind("127.0.0.1:0").unwrap(), id_a).unwrap();
    let ep_b = QuicEndpoint::bind(UdpSocket::bind("127.0.0.1:0").unwrap(), id_b).unwrap();
    let addr_b = ep_b.local_addr().unwrap();

    let edge = Store::new(peer(1));
    let hq = Store::new(peer(2));

    ingest(&edge, &contact("C-1")).await.unwrap();
    ingest(&hq, &sitrep("S-1")).await.unwrap();

    // Acceptor (HQ): accept the connection verifying the edge fingerprint, then
    // the bidi stream the edge opens when it sends its first sync message.
    let hq_state = hq.node().state();
    let hq_rx = hq.node().register_session();
    let acceptor = tokio::spawn(async move {
        let conn = ep_b.accept_with_fingerprint(&fp_a).await.unwrap();
        let (send, recv) = conn.accept_bi().await.unwrap();
        let _ = session::run(hq_state, recv, send, hq_rx, PULL).await;
    });

    // Connector (edge): dial HQ directly by address, verifying its fingerprint.
    let conn = ep_a.connect(addr_b, &fp_b).await.unwrap();
    let (send, recv) = conn.open_bi().await.unwrap();
    let edge_state = edge.node().state();
    let edge_rx = edge.node().register_session();
    let connector = tokio::spawn(async move {
        let _ = session::run(edge_state, recv, send, edge_rx, PULL).await;
    });

    let converged = wait_until(Duration::from_secs(10), || async {
        let a = picture(&edge).await.unwrap();
        let b = picture(&hq).await.unwrap();
        a == b && a.len() == 2
    })
    .await;

    acceptor.abort();
    connector.abort();

    assert!(converged, "nodes did not converge over QUIC");
    let pic = picture(&hq).await.unwrap();
    assert_eq!(pic.get("C-1"), Some(&contact("C-1")));
    assert_eq!(pic.get("S-1"), Some(&sitrep("S-1")));
}
