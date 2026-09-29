use c2_model::C2Message;

pub mod mtf_xml;
pub mod niem;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("unsupported wire format: {0}")]
    Unsupported(String),
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
            codecs: vec![Box::new(mtf_xml::MtfXmlCodec), Box::new(niem::NiemCodec)],
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
        })
    }

    #[test]
    fn registry_exposes_both_formats() {
        let reg = Registry::with_defaults();
        let mut formats = reg.wire_formats();
        formats.sort();
        assert_eq!(formats, vec!["mtf-xml", "niem-json"]);
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
    fn unknown_format_is_none() {
        let reg = Registry::with_defaults();
        assert!(reg.get("stanag-5653").is_none());
    }
}
