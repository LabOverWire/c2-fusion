use std::time::Duration;

use c2_codec::json::JsonCodec;
use c2_codec::mtf_xml::MtfXmlCodec;
use c2_codec::{Codec, Registry};
use c2_exchange::{
    ingest, ingest_wire, ingest_wire_with, picture, picture_encoded, ExchangeError, Picture,
};
use c2_model::{C2Message, ContactReport, Domain, FunctionalService, Sitrep};
use stitch_p2p::{session, PeerId, Store, PEER_ID_LEN};
use tokio::io::{split, DuplexStream};
use tokio::task::JoinHandle;

const PULL: Duration = Duration::from_millis(20);

const CONTACT_MTF_XML: &str = "<c2message><message_type>ContactReport</message_type><id>C-1</id><reporting_unit>UAV-7</reporting_unit><domain>Air</domain><service>Intelligence</service><observed_at>2026-09-29T14:30:00Z</observed_at><latitude>49.2827</latitude><longitude>-123.1207</longitude><description>Vehicle stationary</description></c2message>";

const SITREP_C2_JSON: &str = r#"{"message_type":"Sitrep","id":"S-1","reporting_unit":"Coy-A","domain":"Land","service":"CommandAndControl","reported_at":"2026-09-29T15:00:00Z","summary":"Holding position","personnel_effective":118}"#;

fn expected_contact() -> C2Message {
    C2Message::ContactReport(ContactReport {
        id: "C-1".to_string(),
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

fn expected_sitrep() -> C2Message {
    C2Message::Sitrep(Sitrep {
        id: "S-1".to_string(),
        reporting_unit: "Coy-A".to_string(),
        domain: Domain::Land,
        service: FunctionalService::CommandAndControl,
        reported_at: "2026-09-29T15:00:00Z".to_string(),
        summary: "Holding position".to_string(),
        personnel_effective: 118,
        label: None,
    })
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_wire_formats_converge_to_one_picture_and_reexport() {
    let edge = Store::new(peer(1));
    let hq = Store::new(peer(2));

    let a = ingest_wire(&edge, &MtfXmlCodec, CONTACT_MTF_XML.as_bytes())
        .await
        .unwrap();
    let b = ingest_wire(&hq, &JsonCodec, SITREP_C2_JSON.as_bytes())
        .await
        .unwrap();
    assert_eq!(a, expected_contact());
    assert_eq!(b, expected_sitrep());

    let (edge_io, hq_io) = tokio::io::duplex(64 * 1024);
    let handles = connect(&edge, edge_io, &hq, hq_io);

    let converged = wait_until(Duration::from_secs(3), || async {
        let a = picture(&edge).await.messages;
        let b = picture(&hq).await.messages;
        a == b && a.len() == 2
    })
    .await;
    for h in &handles {
        h.abort();
    }
    assert!(converged, "wire-format messages did not converge");

    let pic = picture(&edge).await.messages;
    assert_eq!(pic.get("C-1"), Some(&expected_contact()));
    assert_eq!(pic.get("S-1"), Some(&expected_sitrep()));

    let exported = picture_encoded(&edge, &JsonCodec).await.unwrap();
    assert_eq!(exported.len(), 2);
    let contact_json = exported.get("C-1").unwrap();
    assert_eq!(JsonCodec.decode(contact_json).unwrap(), expected_contact());
}

#[tokio::test]
async fn unknown_wire_format_is_an_error() {
    let store = Store::new(peer(1));
    let registry = Registry::with_defaults();
    let result = ingest_wire_with(&store, &registry, "stanag-5653", b"...").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn message_that_cannot_replicate_is_refused_at_ingest() {
    let store = Store::new(peer(1));
    let C2Message::ContactReport(mut report) = expected_contact() else {
        unreachable!()
    };
    report.id = "x".repeat(c2_model::MAX_ID_BYTES + 1);
    let result = ingest(&store, &C2Message::ContactReport(report)).await;
    assert!(
        matches!(result, Err(ExchangeError::Invalid(_))),
        "{result:?}"
    );
    assert_eq!(picture(&store).await, Picture::default());
}
