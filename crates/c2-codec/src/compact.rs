use bebytes::BeBytes;

use crate::{Codec, CodecError};
use c2_model::{C2Message, ContactReport, Domain, FunctionalService, PositionReport, Rfi, Sitrep};

const VERSION: u8 = 2;

const CONTACT_REPORT: u8 = 0;
const SITREP: u8 = 1;
const RFI: u8 = 2;
const POSITION_REPORT: u8 = 3;

pub struct CompactCodec;

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct Prefix {
    #[bits(4)]
    version: u8,
    #[bits(4)]
    kind: u8,
    #[bits(4)]
    domain: u8,
    #[bits(4)]
    service: u8,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct ContactWire {
    id_len: u8,
    #[FromField(id_len)]
    id: String,
    unit_len: u8,
    #[FromField(unit_len)]
    reporting_unit: String,
    time_len: u8,
    #[FromField(time_len)]
    observed_at: String,
    latitude: f64,
    longitude: f64,
    description_len: u8,
    #[FromField(description_len)]
    description: String,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct SitrepWire {
    id_len: u8,
    #[FromField(id_len)]
    id: String,
    unit_len: u8,
    #[FromField(unit_len)]
    reporting_unit: String,
    time_len: u8,
    #[FromField(time_len)]
    reported_at: String,
    summary_len: u8,
    #[FromField(summary_len)]
    summary: String,
    personnel_effective: u32,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct RfiWire {
    id_len: u8,
    #[FromField(id_len)]
    id: String,
    unit_len: u8,
    #[FromField(unit_len)]
    requesting_unit: String,
    time_len: u8,
    #[FromField(time_len)]
    requested_at: String,
    question_len: u8,
    #[FromField(question_len)]
    question: String,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct PositionWire {
    id_len: u8,
    #[FromField(id_len)]
    id: String,
    unit_len: u8,
    #[FromField(unit_len)]
    reporting_unit: String,
    time_len: u8,
    #[FromField(time_len)]
    reported_at: String,
    latitude: f64,
    longitude: f64,
}

impl Codec for CompactCodec {
    fn wire_format(&self) -> &'static str {
        "compact-bin"
    }

    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError> {
        let (kind, domain, service, body) = match msg {
            C2Message::ContactReport(m) => (
                CONTACT_REPORT,
                &m.domain,
                &m.service,
                ContactWire {
                    id_len: text_len(&m.id)?,
                    id: m.id.clone(),
                    unit_len: text_len(&m.reporting_unit)?,
                    reporting_unit: m.reporting_unit.clone(),
                    time_len: text_len(&m.observed_at)?,
                    observed_at: m.observed_at.clone(),
                    latitude: m.latitude,
                    longitude: m.longitude,
                    description_len: text_len(&m.description)?,
                    description: m.description.clone(),
                }
                .to_be_bytes(),
            ),
            C2Message::Sitrep(m) => (
                SITREP,
                &m.domain,
                &m.service,
                SitrepWire {
                    id_len: text_len(&m.id)?,
                    id: m.id.clone(),
                    unit_len: text_len(&m.reporting_unit)?,
                    reporting_unit: m.reporting_unit.clone(),
                    time_len: text_len(&m.reported_at)?,
                    reported_at: m.reported_at.clone(),
                    summary_len: text_len(&m.summary)?,
                    summary: m.summary.clone(),
                    personnel_effective: m.personnel_effective,
                }
                .to_be_bytes(),
            ),
            C2Message::Rfi(m) => (
                RFI,
                &m.domain,
                &m.service,
                RfiWire {
                    id_len: text_len(&m.id)?,
                    id: m.id.clone(),
                    unit_len: text_len(&m.requesting_unit)?,
                    requesting_unit: m.requesting_unit.clone(),
                    time_len: text_len(&m.requested_at)?,
                    requested_at: m.requested_at.clone(),
                    question_len: text_len(&m.question)?,
                    question: m.question.clone(),
                }
                .to_be_bytes(),
            ),
            C2Message::PositionReport(m) => (
                POSITION_REPORT,
                &m.domain,
                &m.service,
                PositionWire {
                    id_len: text_len(&m.id)?,
                    id: m.id.clone(),
                    unit_len: text_len(&m.reporting_unit)?,
                    reporting_unit: m.reporting_unit.clone(),
                    time_len: text_len(&m.reported_at)?,
                    reported_at: m.reported_at.clone(),
                    latitude: m.latitude,
                    longitude: m.longitude,
                }
                .to_be_bytes(),
            ),
        };
        let prefix = Prefix {
            version: VERSION,
            kind,
            domain: domain_code(domain),
            service: service_code(service),
        };
        let mut bytes = prefix.to_be_bytes();
        bytes.extend_from_slice(&body);
        Ok(bytes)
    }

    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError> {
        let (prefix, consumed) = Prefix::try_from_be_bytes(bytes).map_err(malformed)?;
        if prefix.version != VERSION {
            return Err(CodecError::Decode(format!(
                "unknown compact version {}",
                prefix.version
            )));
        }
        let domain = domain_from(prefix.domain)?;
        let service = service_from(prefix.service)?;
        let body = &bytes[consumed..];
        match prefix.kind {
            CONTACT_REPORT => {
                let wire: ContactWire = whole(body)?;
                Ok(C2Message::ContactReport(ContactReport {
                    id: wire.id,
                    reporting_unit: wire.reporting_unit,
                    domain,
                    service,
                    observed_at: wire.observed_at,
                    latitude: wire.latitude,
                    longitude: wire.longitude,
                    description: wire.description,
                }))
            }
            SITREP => {
                let wire: SitrepWire = whole(body)?;
                Ok(C2Message::Sitrep(Sitrep {
                    id: wire.id,
                    reporting_unit: wire.reporting_unit,
                    domain,
                    service,
                    reported_at: wire.reported_at,
                    summary: wire.summary,
                    personnel_effective: wire.personnel_effective,
                }))
            }
            RFI => {
                let wire: RfiWire = whole(body)?;
                Ok(C2Message::Rfi(Rfi {
                    id: wire.id,
                    requesting_unit: wire.requesting_unit,
                    domain,
                    service,
                    requested_at: wire.requested_at,
                    question: wire.question,
                }))
            }
            POSITION_REPORT => {
                let wire: PositionWire = whole(body)?;
                Ok(C2Message::PositionReport(PositionReport {
                    id: wire.id,
                    reporting_unit: wire.reporting_unit,
                    domain,
                    service,
                    reported_at: wire.reported_at,
                    latitude: wire.latitude,
                    longitude: wire.longitude,
                }))
            }
            other => Err(CodecError::Decode(format!("unknown message kind {other}"))),
        }
    }
}

fn whole<T: BeBytes>(body: &[u8]) -> Result<T, CodecError> {
    let (wire, consumed) = T::try_from_be_bytes(body).map_err(malformed)?;
    match body.len() - consumed {
        0 => Ok(wire),
        extra => Err(CodecError::Decode(format!("{extra} trailing bytes"))),
    }
}

fn malformed(error: bebytes::BeBytesError) -> CodecError {
    CodecError::Decode(format!("malformed compact message: {error}"))
}

fn text_len(value: &str) -> Result<u8, CodecError> {
    u8::try_from(value.len())
        .map_err(|_| CodecError::Encode(format!("text of {} bytes exceeds 255", value.len())))
}

fn domain_code(domain: &Domain) -> u8 {
    match domain {
        Domain::Land => 0,
        Domain::Air => 1,
        Domain::Maritime => 2,
        Domain::Space => 3,
        Domain::Cyber => 4,
    }
}

fn domain_from(code: u8) -> Result<Domain, CodecError> {
    match code {
        0 => Ok(Domain::Land),
        1 => Ok(Domain::Air),
        2 => Ok(Domain::Maritime),
        3 => Ok(Domain::Space),
        4 => Ok(Domain::Cyber),
        other => Err(CodecError::Decode(format!("unknown domain {other}"))),
    }
}

fn service_code(service: &FunctionalService) -> u8 {
    match service {
        FunctionalService::CommandAndControl => 0,
        FunctionalService::Intelligence => 1,
        FunctionalService::Logistics => 2,
        FunctionalService::Medical => 3,
    }
}

fn service_from(code: u8) -> Result<FunctionalService, CodecError> {
    match code {
        0 => Ok(FunctionalService::CommandAndControl),
        1 => Ok(FunctionalService::Intelligence),
        2 => Ok(FunctionalService::Logistics),
        3 => Ok(FunctionalService::Medical),
        other => Err(CodecError::Decode(format!("unknown service {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::niem::NiemCodec;

    fn contact() -> C2Message {
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

    fn position() -> C2Message {
        C2Message::PositionReport(PositionReport {
            id: "POS-node-a".to_string(),
            reporting_unit: "node-a".to_string(),
            domain: Domain::Land,
            service: FunctionalService::CommandAndControl,
            reported_at: "2026-10-11T00:45:00Z".to_string(),
            latitude: 45.5017,
            longitude: -73.5673,
        })
    }

    #[test]
    fn position_report_fits_one_lora_frame() {
        let bytes = CompactCodec.encode(&position()).expect("encode");
        assert_eq!(bytes.len(), 57);
    }

    #[test]
    fn every_message_kind_round_trips() {
        let messages = [
            contact(),
            C2Message::Sitrep(Sitrep {
                id: "S-1".to_string(),
                reporting_unit: "Coy-A".to_string(),
                domain: Domain::Land,
                service: FunctionalService::CommandAndControl,
                reported_at: "2026-09-29T15:00:00Z".to_string(),
                summary: "Holding position".to_string(),
                personnel_effective: 118,
            }),
            C2Message::Rfi(Rfi {
                id: "RFI-42".to_string(),
                requesting_unit: "HQ-1".to_string(),
                domain: Domain::Maritime,
                service: FunctionalService::Logistics,
                requested_at: "2026-09-29T16:00:00Z".to_string(),
                question: "Fuel state?".to_string(),
            }),
            position(),
        ];
        for msg in messages {
            let bytes = CompactCodec.encode(&msg).expect("encode");
            assert_eq!(CompactCodec.decode(&bytes).expect("decode"), msg);
        }
    }

    #[test]
    fn contact_report_is_much_smaller_than_json() {
        let compact = CompactCodec.encode(&contact()).expect("encode");
        let json = NiemCodec.encode(&contact()).expect("encode");
        assert_eq!(compact.len(), 68);
        assert!(compact.len() * 3 < json.len());
    }

    #[test]
    fn rejects_malformed_input() {
        let bytes = CompactCodec.encode(&contact()).expect("encode");
        for broken in [
            &bytes[..bytes.len() - 1],
            &[0x10, 0x11, 3, b'C', b'-', b'1'][..],
            &[0x29, 0x11, 3, b'C', b'-', b'1'][..],
            &[0x20, 0x91, 3, b'C', b'-', b'1'][..],
        ] {
            assert!(CompactCodec.decode(broken).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(CompactCodec.decode(&trailing).is_err());
    }

    #[test]
    fn rejects_overlong_text() {
        let C2Message::ContactReport(mut report) = contact() else {
            return;
        };
        report.description = "x".repeat(256);
        assert!(CompactCodec
            .encode(&C2Message::ContactReport(report))
            .is_err());
    }
}
