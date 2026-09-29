use crate::{Codec, CodecError};
use c2_model::C2Message;

pub struct NiemCodec;

impl Codec for NiemCodec {
    fn wire_format(&self) -> &'static str {
        "niem-json"
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
        });
        let codec = NiemCodec;
        let bytes = codec.encode(&msg).unwrap();
        assert_eq!(codec.decode(&bytes).unwrap(), msg);
    }
}
