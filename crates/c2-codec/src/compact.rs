use bebytes::BeBytes;

use crate::{Codec, CodecError};
use c2_model::{
    C2Message, Category, CategoryKind, ConfidentialityLabel, ContactReport, Domain,
    FunctionalService, Policy, PositionReport, Rfi, Sitrep,
};

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
    #[bits(1)]
    labelled: u8,
    #[bits(3)]
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

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct LabelWire {
    policy_name_len: u8,
    #[FromField(policy_name_len)]
    policy_name: String,
    policy_uri_len: u8,
    #[FromField(policy_uri_len)]
    policy_uri: String,
    classification_len: u8,
    #[FromField(classification_len)]
    classification: String,
    created_len: u8,
    #[FromField(created_len)]
    created_at: String,
    review_len: u8,
    #[FromField(review_len)]
    review_at: String,
    category_count: u8,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct CategoryWire {
    kind: u8,
    tag_len: u8,
    #[FromField(tag_len)]
    tag: String,
    uri_len: u8,
    #[FromField(uri_len)]
    uri: String,
    value_count: u8,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct ValueWire {
    value_len: u8,
    #[FromField(value_len)]
    value: String,
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
        let label = msg.label().map(encode_label).transpose()?;
        let prefix = Prefix {
            version: VERSION,
            labelled: u8::from(label.is_some()),
            kind,
            domain: domain_code(domain),
            service: service_code(service),
        };
        let mut bytes = prefix.to_be_bytes();
        bytes.extend_from_slice(&body);
        bytes.extend(label.unwrap_or_default());
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
                let (wire, label): (ContactWire, _) = with_label(body, prefix.labelled)?;
                Ok(C2Message::ContactReport(ContactReport {
                    id: wire.id,
                    reporting_unit: wire.reporting_unit,
                    domain,
                    service,
                    observed_at: wire.observed_at,
                    latitude: wire.latitude,
                    longitude: wire.longitude,
                    description: wire.description,
                    label,
                }))
            }
            SITREP => {
                let (wire, label): (SitrepWire, _) = with_label(body, prefix.labelled)?;
                Ok(C2Message::Sitrep(Sitrep {
                    id: wire.id,
                    reporting_unit: wire.reporting_unit,
                    domain,
                    service,
                    reported_at: wire.reported_at,
                    summary: wire.summary,
                    personnel_effective: wire.personnel_effective,
                    label,
                }))
            }
            RFI => {
                let (wire, label): (RfiWire, _) = with_label(body, prefix.labelled)?;
                Ok(C2Message::Rfi(Rfi {
                    id: wire.id,
                    requesting_unit: wire.requesting_unit,
                    domain,
                    service,
                    requested_at: wire.requested_at,
                    question: wire.question,
                    label,
                }))
            }
            POSITION_REPORT => {
                let (wire, label): (PositionWire, _) = with_label(body, prefix.labelled)?;
                Ok(C2Message::PositionReport(PositionReport {
                    id: wire.id,
                    reporting_unit: wire.reporting_unit,
                    domain,
                    service,
                    reported_at: wire.reported_at,
                    latitude: wire.latitude,
                    longitude: wire.longitude,
                    label,
                }))
            }
            other => Err(CodecError::Decode(format!("unknown message kind {other}"))),
        }
    }
}

fn with_label<T: BeBytes>(
    body: &[u8],
    labelled: u8,
) -> Result<(T, Option<ConfidentialityLabel>), CodecError> {
    let (wire, consumed) = T::try_from_be_bytes(body).map_err(malformed)?;
    let rest = &body[consumed..];
    let (label, rest) = match labelled {
        0 => (None, rest),
        _ => {
            let (label, rest) = decode_label(rest)?;
            (Some(label), rest)
        }
    };
    match rest.len() {
        0 => Ok((wire, label)),
        extra => Err(CodecError::Decode(format!("{extra} trailing bytes"))),
    }
}

fn encode_label(label: &ConfidentialityLabel) -> Result<Vec<u8>, CodecError> {
    let mut bytes = LabelWire {
        policy_name_len: text_len(&label.policy.name)?,
        policy_name: label.policy.name.clone(),
        policy_uri_len: text_len(&label.policy.uri)?,
        policy_uri: label.policy.uri.clone(),
        classification_len: text_len(&label.classification)?,
        classification: label.classification.clone(),
        created_len: optional_len(&label.created_at)?,
        created_at: label.created_at.clone().unwrap_or_default(),
        review_len: optional_len(&label.review_at)?,
        review_at: label.review_at.clone().unwrap_or_default(),
        category_count: count(label.categories.len())?,
    }
    .to_be_bytes();
    for category in &label.categories {
        bytes.extend(
            CategoryWire {
                kind: kind_code(category.kind),
                tag_len: text_len(&category.tag)?,
                tag: category.tag.clone(),
                uri_len: text_len(&category.uri)?,
                uri: category.uri.clone(),
                value_count: count(category.values.len())?,
            }
            .to_be_bytes(),
        );
        for value in &category.values {
            bytes.extend(
                ValueWire {
                    value_len: text_len(value)?,
                    value: value.clone(),
                }
                .to_be_bytes(),
            );
        }
    }
    Ok(bytes)
}

fn decode_label(bytes: &[u8]) -> Result<(ConfidentialityLabel, &[u8]), CodecError> {
    let (wire, consumed) = LabelWire::try_from_be_bytes(bytes).map_err(malformed)?;
    let mut rest = &bytes[consumed..];
    let mut categories = Vec::with_capacity(usize::from(wire.category_count));
    for _ in 0..wire.category_count {
        let (category, consumed) = CategoryWire::try_from_be_bytes(rest).map_err(malformed)?;
        rest = &rest[consumed..];
        let mut values = Vec::with_capacity(usize::from(category.value_count));
        for _ in 0..category.value_count {
            let (value, consumed) = ValueWire::try_from_be_bytes(rest).map_err(malformed)?;
            rest = &rest[consumed..];
            values.push(value.value);
        }
        categories.push(Category {
            kind: kind_from(category.kind)?,
            tag: category.tag,
            uri: category.uri,
            values,
        });
    }
    let label = ConfidentialityLabel {
        policy: Policy {
            name: wire.policy_name,
            uri: wire.policy_uri,
        },
        classification: wire.classification,
        categories,
        created_at: present(wire.created_at),
        review_at: present(wire.review_at),
    };
    Ok((label, rest))
}

fn optional_len(value: &Option<String>) -> Result<u8, CodecError> {
    match value.as_deref() {
        None => Ok(0),
        Some("") => Err(CodecError::Encode(
            "an optional label time cannot be empty".to_string(),
        )),
        Some(text) => text_len(text),
    }
}

fn present(value: String) -> Option<String> {
    match value.is_empty() {
        true => None,
        false => Some(value),
    }
}

fn count(len: usize) -> Result<u8, CodecError> {
    u8::try_from(len).map_err(|_| CodecError::Encode(format!("{len} entries exceed 255")))
}

fn kind_code(kind: CategoryKind) -> u8 {
    match kind {
        CategoryKind::Permissive => 0,
        CategoryKind::Restrictive => 1,
    }
}

fn kind_from(code: u8) -> Result<CategoryKind, CodecError> {
    match code {
        0 => Ok(CategoryKind::Permissive),
        1 => Ok(CategoryKind::Restrictive),
        other => Err(CodecError::Decode(format!("unknown category kind {other}"))),
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
    use crate::json::JsonCodec;

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
            label: None,
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
            label: None,
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
                label: None,
            }),
            C2Message::Rfi(Rfi {
                id: "RFI-42".to_string(),
                requesting_unit: "HQ-1".to_string(),
                domain: Domain::Maritime,
                service: FunctionalService::Logistics,
                requested_at: "2026-09-29T16:00:00Z".to_string(),
                question: "Fuel state?".to_string(),
                label: None,
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
        let json = JsonCodec.encode(&contact()).expect("encode");
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
    fn rejects_every_truncation() {
        for msg in [contact(), position()] {
            let bytes = CompactCodec.encode(&msg).expect("encode");
            for cut in 0..bytes.len() {
                assert!(CompactCodec.decode(&bytes[..cut]).is_err(), "cut at {cut}");
            }
        }
    }

    fn labelled(msg: C2Message) -> C2Message {
        let label = Some(crate::sample_label());
        match msg {
            C2Message::ContactReport(m) => C2Message::ContactReport(ContactReport { label, ..m }),
            C2Message::Sitrep(m) => C2Message::Sitrep(Sitrep { label, ..m }),
            C2Message::Rfi(m) => C2Message::Rfi(Rfi { label, ..m }),
            C2Message::PositionReport(m) => {
                C2Message::PositionReport(PositionReport { label, ..m })
            }
        }
    }

    #[test]
    fn labelled_messages_round_trip_and_reject_every_truncation() {
        for msg in [labelled(contact()), labelled(position())] {
            let bytes = CompactCodec.encode(&msg).expect("encode");
            assert_eq!(CompactCodec.decode(&bytes).expect("decode"), msg);
            for cut in 0..bytes.len() {
                assert!(CompactCodec.decode(&bytes[..cut]).is_err(), "cut at {cut}");
            }
        }
    }

    #[test]
    fn label_adds_its_own_section_only() {
        let plain = CompactCodec.encode(&position()).expect("encode");
        let with_label = CompactCodec.encode(&labelled(position())).expect("encode");
        assert_eq!(plain.len(), 57);
        assert_eq!(with_label.len(), 57 + 133);
        assert_eq!(plain[2..], with_label[2..57]);
    }

    #[test]
    fn refuses_empty_optional_label_time() {
        let mut label = crate::sample_label();
        label.review_at = Some(String::new());
        let C2Message::PositionReport(report) = position() else {
            return;
        };
        let msg = C2Message::PositionReport(PositionReport {
            label: Some(label),
            ..report
        });
        assert!(CompactCodec.encode(&msg).is_err());
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
