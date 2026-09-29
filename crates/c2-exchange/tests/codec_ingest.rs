//! The codec boundary end to end: messages arrive on two nodes in two different
//! wire formats, converge into one shared picture, and the picture is re-exported
//! in a single format. This is essential outcomes 3 and 4 (standards-compliant
//! import/export/exchange, format-agnostic core) demonstrated over the p2p layer.

use std::time::Duration;

use c2_codec::mtf_xml::MtfXmlCodec;
use c2_codec::niem::NiemCodec;
use c2_codec::{Codec, Registry};
use c2_exchange::{ingest_wire, ingest_wire_with, picture, picture_encoded};
use c2_model::{ContactReport, C2Message, Domain, FunctionalService, Sitrep};
use stitch_p2p::{session, PeerId, Store, PEER_ID_LEN};
use tokio::io::{split, DuplexStream};
use tokio::task::JoinHandle;

const PULL: Duration = Duration::from_millis(20);

// A contact report as it would arrive on the wire in XML Message Text Format.
const CONTACT_MTF_XML: &str = "<c2message><message_type>ContactReport</message_type><id>C-1</id><reporting_unit>UAV-7</reporting_unit><domain>Air</domain><service>Intelligence</service><observed_at>2026-09-29T14:30:00Z</observed_at><latitude>49.2827</latitude><longitude>-123.1207</longitude><description>Vehicle stationary</description></c2message>";

// A situation report as it would arrive on the wire in NIEM-style JSON.
const SITREP_NIEM_JSON: &str = r#"{"message_type":"Sitrep","id":"S-1","reporting_unit":"Coy-A","domain":"Land","service":"CommandAndControl","reported_at":"2026-09-29T15:00:00Z","summary":"Holding position","personnel_effective":118}"#;

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

    // Each node receives a message in a different wire format.
    let a = ingest_wire(&edge, &MtfXmlCodec, CONTACT_MTF_XML.as_bytes())
        .await
        .unwrap();
    let b = ingest_wire(&hq, &NiemCodec, SITREP_NIEM_JSON.as_bytes())
        .await
        .unwrap();
    assert_eq!(a, expected_contact());
    assert_eq!(b, expected_sitrep());

    let (edge_io, hq_io) = tokio::io::duplex(64 * 1024);
    let handles = connect(&edge, edge_io, &hq, hq_io);

    let converged = wait_until(Duration::from_secs(3), || async {
        let a = picture(&edge).await.unwrap();
        let b = picture(&hq).await.unwrap();
        a == b && a.len() == 2
    })
    .await;
    for h in &handles {
        h.abort();
    }
    assert!(converged, "wire-format messages did not converge");

    let pic = picture(&edge).await.unwrap();
    assert_eq!(pic.get("C-1"), Some(&expected_contact()));
    assert_eq!(pic.get("S-1"), Some(&expected_sitrep()));

    // The converged picture, assembled from XML and JSON inputs, re-exports in a
    // single format. The XML-sourced contact report comes back out as NIEM JSON.
    let exported = picture_encoded(&edge, &NiemCodec).await.unwrap();
    assert_eq!(exported.len(), 2);
    let contact_json = exported.get("C-1").unwrap();
    assert_eq!(
        NiemCodec.decode(contact_json).unwrap(),
        expected_contact()
    );
}

#[tokio::test]
async fn unknown_wire_format_is_an_error() {
    let store = Store::new(peer(1));
    let registry = Registry::with_defaults();
    let result = ingest_wire_with(&store, &registry, "stanag-5653", b"...").await;
    assert!(result.is_err());
}
