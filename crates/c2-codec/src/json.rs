use crate::{Codec, CodecError};
use c2_model::C2Message;

pub struct JsonCodec;

impl Codec for JsonCodec {
    fn wire_format(&self) -> &'static str {
        "c2-json"
    }

    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError> {
        serde_json::to_vec(msg).map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError> {
        serde_json::from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_model::{Domain, FunctionalService, Rfi};

    #[test]
    fn rfi_round_trips() {
        let msg = C2Message::Rfi(Rfi {
            id: "RFI-42".to_string(),
            requesting_unit: "HQ-1".to_string(),
            domain: Domain::Land,
            service: FunctionalService::CommandAndControl,
            requested_at: "2026-09-23T15:00:00Z".to_string(),
            question: "Confirm bridge status at grid 12AB".to_string(),
            label: None,
        });
        let codec = JsonCodec;
        let bytes = codec.encode(&msg).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("label"));
        assert_eq!(codec.decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn message_written_before_labels_still_decodes() {
        let bytes = br#"{"message_type":"Rfi","id":"RFI-1","requesting_unit":"HQ-1","domain":"Land","service":"Logistics","requested_at":"2026-09-23T15:00:00Z","question":"Fuel state?"}"#;
        let C2Message::Rfi(rfi) = JsonCodec.decode(bytes).unwrap() else {
            panic!("not an rfi");
        };
        assert_eq!(rfi.label, None);
    }

    #[test]
    fn label_round_trips_in_json() {
        let msg = C2Message::Rfi(Rfi {
            id: "RFI-43".to_string(),
            requesting_unit: "HQ-1".to_string(),
            domain: Domain::Land,
            service: FunctionalService::CommandAndControl,
            requested_at: "2026-09-23T15:00:00Z".to_string(),
            question: "Confirm bridge status".to_string(),
            label: Some(crate::sample_label()),
        });
        let bytes = JsonCodec.encode(&msg).unwrap();
        assert_eq!(JsonCodec.decode(&bytes).unwrap(), msg);
    }
}
