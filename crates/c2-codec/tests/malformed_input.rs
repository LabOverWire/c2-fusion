use c2_codec::coie_text::CoieTextCodec;
use c2_codec::compact::CompactCodec;
use c2_codec::json::JsonCodec;
use c2_codec::mtf_xml::MtfXmlCodec;
use c2_codec::niem_json::NiemJsonCodec;
use c2_codec::niem_xml::NiemXmlCodec;
use c2_codec::{Codec, CodecError, Registry};
use c2_model::{C2Message, ContactReport, Domain, FunctionalService, PositionReport, Sitrep};
use serde_json::{json, Value};

fn contact() -> C2Message {
    C2Message::ContactReport(ContactReport {
        id: "C-1".to_string(),
        reporting_unit: "UAV-7".to_string(),
        domain: Domain::Air,
        service: FunctionalService::Intelligence,
        observed_at: "2026-10-29T14:30:00Z".to_string(),
        latitude: 45.5017,
        longitude: -73.5673,
        description: "Vehicle stationary".to_string(),
        label: None,
    })
}

fn contact_with(change: impl FnOnce(&mut ContactReport)) -> C2Message {
    let C2Message::ContactReport(mut report) = contact() else {
        unreachable!()
    };
    change(&mut report);
    C2Message::ContactReport(report)
}

fn sitrep() -> C2Message {
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

fn position() -> C2Message {
    C2Message::PositionReport(PositionReport {
        id: "POS-node-b".to_string(),
        reporting_unit: "node-b".to_string(),
        domain: Domain::Land,
        service: FunctionalService::CommandAndControl,
        reported_at: "2026-10-11T00:45:00Z".to_string(),
        latitude: -33.8688,
        longitude: 151.2093,
        label: None,
    })
}

fn wire(codec: &dyn Codec, msg: &C2Message) -> String {
    let bytes = codec.encode(msg).expect("baseline encodes");
    assert_eq!(&codec.decode(&bytes).expect("baseline decodes"), msg);
    String::from_utf8(bytes).expect("text wire format")
}

fn replaced(text: &str, from: &str, to: &str) -> Vec<u8> {
    assert!(text.contains(from), "{from} not in {text}");
    text.replacen(from, to, 1).into_bytes()
}

fn refused(codec: &dyn Codec, bytes: &[u8]) -> CodecError {
    match codec.decode(bytes) {
        Ok(msg) => panic!("{} accepted {msg:?}", codec.wire_format()),
        Err(error) => error,
    }
}

fn niem_json(msg: &C2Message) -> Value {
    serde_json::from_str(&wire(&NiemJsonCodec, msg)).expect("json")
}

#[test]
fn niem_xml_refuses_repeated_elements() {
    let xml = wire(&NiemXmlCodec, &contact());
    let bytes = replaced(
        &xml,
        "<nc:IdentificationID>C-1</nc:IdentificationID>",
        "<nc:IdentificationID>C-1</nc:IdentificationID><nc:IdentificationID>EVIL</nc:IdentificationID>",
    );
    assert!(matches!(
        refused(&NiemXmlCodec, &bytes),
        CodecError::Decode(_)
    ));
}

#[test]
fn niem_xml_refuses_content_after_the_root() {
    let xml = wire(&NiemXmlCodec, &contact());
    let trailing = format!("{xml}<other><x>EVIL</x></other>");
    assert!(matches!(
        refused(&NiemXmlCodec, trailing.as_bytes()),
        CodecError::Decode(_)
    ));
    let whitespace = format!("{xml}\n  \n");
    assert_eq!(
        NiemXmlCodec.decode(whitespace.as_bytes()).expect("decodes"),
        contact()
    );
}

#[test]
fn niem_xml_decode_applies_the_encode_checks() {
    let xml = wire(&NiemXmlCodec, &contact());
    for (from, to) in [
        (">45.5017<", ">999<"),
        (">45.5017<", ">NaN<"),
        (">2026-10-29T14:30:00Z<", ">garbage<"),
        (">2026-10-29T14:30:00Z<", ">2026-10-29T14:30:00+01:00<"),
    ] {
        assert!(
            matches!(
                refused(&NiemXmlCodec, &replaced(&xml, from, to)),
                CodecError::Invalid(_)
            ),
            "{to}"
        );
    }
    let sitrep = wire(&NiemXmlCodec, &sitrep());
    let bytes = replaced(&sitrep, ">118<", ">100000<");
    assert!(matches!(
        refused(&NiemXmlCodec, &bytes),
        CodecError::Invalid(_)
    ));
    let position = wire(&NiemXmlCodec, &position());
    let bytes = replaced(&position, ">node-b<", ">HQ<");
    assert!(matches!(
        refused(&NiemXmlCodec, &bytes),
        CodecError::Invalid(_)
    ));
}

#[test]
fn niem_json_decode_applies_the_encode_checks() {
    let text = wire(&NiemJsonCodec, &contact());
    let bytes = replaced(&text, "[45.5017]", "[999.0]");
    assert!(matches!(
        refused(&NiemJsonCodec, &bytes),
        CodecError::Invalid(_)
    ));

    let mut doc = niem_json(&sitrep());
    doc["c2f:SituationReportMessage"]["usmtf:AvailablePersonnelQuantity"] = json!([100000]);
    let bytes = serde_json::to_vec(&doc).expect("json");
    assert!(matches!(
        refused(&NiemJsonCodec, &bytes),
        CodecError::Invalid(_)
    ));
}

#[test]
fn niem_json_refuses_several_values_for_one_field() {
    let mut doc = niem_json(&contact());
    doc["c2f:ContactReportMessage"]["nc:DocumentIdentification"] =
        json!([{"nc:IdentificationID": ["C-1", "EVIL"]}]);
    let bytes = serde_json::to_vec(&doc).expect("json");
    assert!(matches!(
        refused(&NiemJsonCodec, &bytes),
        CodecError::Decode(_)
    ));
}

#[test]
fn niem_json_refuses_two_prefixes_for_one_property() {
    let mut doc = niem_json(&contact());
    doc["@context"]["alias"] = doc["@context"]["nc"].clone();
    doc["c2f:ContactReportMessage"]["alias:DocumentDescriptionText"] =
        json!({"nc:TextValue": "EVIL"});
    let bytes = serde_json::to_vec(&doc).expect("json");
    assert!(matches!(
        refused(&NiemJsonCodec, &bytes),
        CodecError::Decode(_)
    ));
}

#[test]
fn niem_json_refuses_duplicate_keys() {
    let text = wire(&NiemJsonCodec, &contact());
    let bytes = replaced(
        &text,
        r#""nc:IdentificationID":["C-1"]"#,
        r#""nc:IdentificationID":["C-1"],"nc:IdentificationID":["EVIL"]"#,
    );
    assert!(matches!(
        refused(&NiemJsonCodec, &bytes),
        CodecError::Decode(_)
    ));
}

#[test]
fn c2_json_refuses_duplicate_keys() {
    let text = wire(&JsonCodec, &contact());
    let bytes = replaced(&text, r#""id":"C-1""#, r#""id":"C-1","id":"EVIL""#);
    assert!(matches!(refused(&JsonCodec, &bytes), CodecError::Decode(_)));
}

#[test]
fn coie_decode_applies_the_model_checks() {
    let text = wire(&CoieTextCodec, &contact());
    for (from, to) in [
        ("POSITION/45.5017/", "POSITION/NaN/"),
        ("/-73.5673//", "/inf//"),
        ("POSITION/45.5017/", "POSITION/200/"),
        ("TIME/2026-10-29T14:30:00Z//", "TIME/xxxxx10yyyy//"),
    ] {
        assert!(
            matches!(
                refused(&CoieTextCodec, &replaced(&text, from, to)),
                CodecError::Invalid(_)
            ),
            "{to}"
        );
    }
}

#[test]
fn coie_refuses_free_text_it_cannot_read_back() {
    let msg = contact_with(|m| m.description = "/x".to_string());
    assert!(matches!(
        CoieTextCodec.encode(&msg),
        Err(CodecError::Encode(_))
    ));
}

#[test]
fn coie_reports_a_bad_time_as_invalid_on_decode() {
    let text = wire(&CoieTextCodec, &contact());
    let bytes = replaced(&text, "TIME/2026-10-29T14:30:00Z//", "TIME/garbage//");
    assert!(matches!(
        refused(&CoieTextCodec, &bytes),
        CodecError::Invalid(_)
    ));
}

#[test]
fn mtf_keeps_numeric_looking_strings() {
    let msg = contact_with(|m| {
        m.id = "1001".to_string();
        m.description = "1e5".to_string();
    });
    let bytes = MtfXmlCodec.encode(&msg).expect("encodes");
    assert_eq!(MtfXmlCodec.decode(&bytes).expect("decodes"), msg);
}

#[test]
fn mtf_refuses_repeated_and_unknown_elements() {
    let xml = wire(&MtfXmlCodec, &contact());
    for (from, to) in [
        ("<id>C-1</id>", "<id>C-1</id><id>EVIL</id>"),
        ("<id>C-1</id>", "<id>C-1</id><bogus>x</bogus>"),
        ("<latitude>45.5017</latitude>", "<latitude>north</latitude>"),
    ] {
        assert!(
            matches!(
                refused(&MtfXmlCodec, &replaced(&xml, from, to)),
                CodecError::Decode(_)
            ),
            "{to}"
        );
    }
}

#[test]
fn every_codec_refuses_invalid_coordinates_and_ids() {
    let registry = Registry::with_defaults();
    let invalid = [
        contact_with(|m| m.latitude = f64::NAN),
        contact_with(|m| m.longitude = f64::INFINITY),
        contact_with(|m| m.latitude = 200.0),
        contact_with(|m| m.observed_at = "2026-02-31T00:00:00Z".to_string()),
        contact_with(|m| m.id = String::new()),
        contact_with(|m| m.id = "x".repeat(256)),
    ];
    for wire_format in registry.wire_formats() {
        let codec = registry.get(wire_format).expect("registered");
        for msg in &invalid {
            assert!(
                matches!(codec.encode(msg), Err(CodecError::Invalid(_))),
                "{wire_format} encoded {msg:?}"
            );
        }
    }
}

#[test]
fn json_and_compact_refuse_invalid_messages_on_decode() {
    let text = wire(&JsonCodec, &contact());
    let bytes = replaced(&text, "45.5017", "999.0");
    assert!(matches!(
        refused(&JsonCodec, &bytes),
        CodecError::Invalid(_)
    ));

    let encoded = CompactCodec.encode(&contact()).expect("encodes");
    let latitude = 45.5017_f64.to_be_bytes();
    let at = encoded
        .windows(8)
        .position(|window| window == latitude)
        .expect("latitude bytes");
    let mut bytes = encoded.clone();
    bytes[at..at + 8].copy_from_slice(&f64::NAN.to_be_bytes());
    assert!(matches!(
        refused(&CompactCodec, &bytes),
        CodecError::Invalid(_)
    ));
}
