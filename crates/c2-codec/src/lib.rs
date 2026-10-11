use c2_model::C2Message;

mod codes;
pub mod coie_text;
pub mod compact;
pub mod json;
pub mod mtf_xml;
pub mod niem;
pub mod niem_json;
pub mod niem_xml;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("unsupported wire format: {0}")]
    Unsupported(String),
}

fn refuse_label(msg: &C2Message, wire_format: &str) -> Result<(), CodecError> {
    match msg.label() {
        None => Ok(()),
        Some(_) => Err(CodecError::Encode(format!(
            "{wire_format} has no representation for a confidentiality label"
        ))),
    }
}

pub trait Codec: Send + Sync {
    fn wire_format(&self) -> &'static str;
    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError>;
    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError>;
}

pub struct Registry {
    codecs: Vec<Box<dyn Codec>>,
}

impl Registry {
    pub fn with_defaults() -> Self {
        Self {
            codecs: vec![
                Box::new(mtf_xml::MtfXmlCodec),
                Box::new(json::JsonCodec),
                Box::new(compact::CompactCodec),
                Box::new(niem_xml::NiemXmlCodec),
                Box::new(coie_text::CoieTextCodec),
                Box::new(niem_json::NiemJsonCodec),
            ],
        }
    }

    pub fn register(&mut self, codec: Box<dyn Codec>) {
        self.codecs.push(codec);
    }

    pub fn get(&self, wire_format: &str) -> Option<&dyn Codec> {
        self.codecs
            .iter()
            .map(|c| c.as_ref())
            .find(|c| c.wire_format() == wire_format)
    }

    pub fn wire_formats(&self) -> Vec<&'static str> {
        self.codecs.iter().map(|c| c.wire_format()).collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
pub(crate) fn sample_label() -> c2_model::ConfidentialityLabel {
    use c2_model::{Category, CategoryKind, ConfidentialityLabel, Policy};
    ConfidentialityLabel {
        policy: Policy {
            name: "CWIX25".to_string(),
            uri: "urn:oid:1.3.6.1.4.1.31778.102.25".to_string(),
        },
        classification: "UNCLASSIFIED".to_string(),
        categories: vec![Category {
            kind: CategoryKind::Permissive,
            tag: "Releasable To".to_string(),
            uri: "urn:oid:1.3.6.1.4.1.31778.103.15".to_string(),
            values: vec!["CAN".to_string(), "AUS".to_string()],
        }],
        created_at: Some("2026-10-11T12:00:00Z".to_string()),
        review_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_model::{ContactReport, Domain, FunctionalService};

    fn sample() -> C2Message {
        C2Message::ContactReport(ContactReport {
            id: "C-1001".to_string(),
            reporting_unit: "UAV-7".to_string(),
            domain: Domain::Air,
            service: FunctionalService::Intelligence,
            observed_at: "2026-09-23T14:30:00Z".to_string(),
            latitude: 49.2827,
            longitude: -123.1207,
            description: "Vehicle <light> & stationary".to_string(),
            label: None,
        })
    }

    #[test]
    fn registry_exposes_every_format() {
        let reg = Registry::with_defaults();
        let mut formats = reg.wire_formats();
        formats.sort();
        assert_eq!(
            formats,
            vec![
                "c2-json",
                "coie-text",
                "compact-bin",
                "mtf-xml",
                "niem-json",
                "niem-xml"
            ]
        );
    }

    #[test]
    fn same_message_round_trips_through_every_registered_codec() {
        let reg = Registry::with_defaults();
        let msg = sample();
        for wire in reg.wire_formats() {
            let codec = reg.get(wire).unwrap();
            let bytes = codec.encode(&msg).unwrap();
            let back = codec.decode(&bytes).unwrap();
            assert_eq!(msg, back, "round trip failed for {wire}");
        }
    }

    #[test]
    fn labelled_message_round_trips_or_is_refused() {
        let reg = Registry::with_defaults();
        let C2Message::ContactReport(mut report) = sample() else {
            return;
        };
        report.label = Some(sample_label());
        let msg = C2Message::ContactReport(report);
        for wire in reg.wire_formats() {
            let codec = reg.get(wire).unwrap();
            match (wire, codec.encode(&msg)) {
                ("c2-json" | "compact-bin", Ok(bytes)) => {
                    assert_eq!(codec.decode(&bytes).unwrap(), msg, "{wire}")
                }
                (_, Err(CodecError::Encode(reason))) => {
                    assert!(reason.contains("confidentiality label"), "{wire}: {reason}")
                }
                (_, outcome) => panic!("{wire} gave {outcome:?}"),
            }
        }
    }

    #[test]
    fn unknown_format_is_none() {
        let reg = Registry::with_defaults();
        assert!(reg.get("stanag-5653").is_none());
    }
}
