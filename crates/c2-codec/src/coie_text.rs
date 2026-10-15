use c2_model::{C2Message, ContactReport, Domain, FunctionalService, PositionReport, Rfi, Sitrep};

use crate::codes::{domain_code, domain_from, service_code, service_from};
use crate::{refuse_label, validated, Codec, CodecError};

const SET_END: &str = "//";
const FIELD: char = '/';
const NO_DATA: &str = "-";

const CONTACT_REPORT: &str = "SPOTREP";
const SITUATION_REPORT: &str = "SITREP";
const INFORMATION_REQUEST: &str = "RFI";
const POSITION_REPORT: &str = "POSREP";

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

pub struct CoieTextCodec;

struct Set {
    id: String,
    fields: Vec<String>,
}

impl Codec for CoieTextCodec {
    fn wire_format(&self) -> &'static str {
        "coie-text"
    }

    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError> {
        refuse_label(msg, self.wire_format())?;
        msg.validate()?;
        let lines = match msg {
            C2Message::ContactReport(m) => vec![
                msgid(CONTACT_REPORT, &m.reporting_unit, &m.id, &m.observed_at)?,
                set("TIME", &[&m.observed_at])?,
                position(m.latitude, m.longitude)?,
                class(&m.domain, &m.service)?,
                free_text("AMPN", &m.description)?,
            ],
            C2Message::Sitrep(m) => vec![
                msgid(SITUATION_REPORT, &m.reporting_unit, &m.id, &m.reported_at)?,
                set("TIME", &[&m.reported_at])?,
                set("PERS", &[&m.personnel_effective.to_string()])?,
                class(&m.domain, &m.service)?,
                free_text("NARR", &m.summary)?,
            ],
            C2Message::Rfi(m) => vec![
                msgid(
                    INFORMATION_REQUEST,
                    &m.requesting_unit,
                    &m.id,
                    &m.requested_at,
                )?,
                set("TIME", &[&m.requested_at])?,
                class(&m.domain, &m.service)?,
                free_text("NARR", &m.question)?,
            ],
            C2Message::PositionReport(m) => vec![
                msgid(POSITION_REPORT, &m.reporting_unit, &m.id, &m.reported_at)?,
                set("TIME", &[&m.reported_at])?,
                position(m.latitude, m.longitude)?,
                class(&m.domain, &m.service)?,
            ],
        };
        Ok(lines.join("\n").into_bytes())
    }

    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError> {
        let text = std::str::from_utf8(bytes).map_err(|e| CodecError::Decode(e.to_string()))?;
        validated(message(text)?)
    }
}

fn message(text: &str) -> Result<C2Message, CodecError> {
    let sets = parse(text)?;
    let [msgid, rest @ ..] = sets.as_slice() else {
        return Err(CodecError::Decode("message has no sets".to_string()));
    };
    let (identifier, originator, serial, month) = match (msgid.id.as_str(), msgid.fields.as_slice())
    {
        ("MSGID", [identifier, originator, serial, month]) => {
            (identifier, originator, serial, month)
        }
        _ => {
            return Err(CodecError::Decode(
                "message must start with MSGID/identifier/originator/serial/month//".to_string(),
            ))
        }
    };
    let time = single(rest, "TIME")?;
    if month_of(&time)? != month.as_str() {
        return Err(CodecError::Decode(format!(
            "MSGID month {month} does not match TIME {time}"
        )));
    }
    let (domain, service) = match find(rest, "CLASS")?.fields.as_slice() {
        [domain, service] => (domain_from(domain)?, service_from(service)?),
        _ => {
            return Err(CodecError::Decode(
                "CLASS must carry a domain and a service".to_string(),
            ))
        }
    };
    let expected: &[&str] = match identifier.as_str() {
        CONTACT_REPORT => &["TIME", "POSITION", "CLASS", "AMPN"],
        SITUATION_REPORT => &["TIME", "PERS", "CLASS", "NARR"],
        INFORMATION_REQUEST => &["TIME", "CLASS", "NARR"],
        POSITION_REPORT => &["TIME", "POSITION", "CLASS"],
        other => {
            return Err(CodecError::Decode(format!(
                "unknown message identifier {other}"
            )))
        }
    };
    let found: Vec<&str> = rest.iter().map(|set| set.id.as_str()).collect();
    if found != expected {
        return Err(CodecError::Decode(format!(
            "{identifier} sets must be {expected:?}, found {found:?}"
        )));
    }
    match identifier.as_str() {
        CONTACT_REPORT => {
            let (latitude, longitude) = coordinates(rest)?;
            Ok(C2Message::ContactReport(ContactReport {
                id: serial.clone(),
                reporting_unit: originator.clone(),
                domain,
                service,
                observed_at: time,
                latitude,
                longitude,
                description: single(rest, "AMPN")?,
                label: None,
            }))
        }
        SITUATION_REPORT => Ok(C2Message::Sitrep(Sitrep {
            id: serial.clone(),
            reporting_unit: originator.clone(),
            domain,
            service,
            reported_at: time,
            summary: single(rest, "NARR")?,
            personnel_effective: single(rest, "PERS")?
                .parse()
                .map_err(|e| CodecError::Decode(format!("PERS: {e}")))?,
            label: None,
        })),
        INFORMATION_REQUEST => Ok(C2Message::Rfi(Rfi {
            id: serial.clone(),
            requesting_unit: originator.clone(),
            domain,
            service,
            requested_at: time,
            question: single(rest, "NARR")?,
            label: None,
        })),
        _ => {
            let (latitude, longitude) = coordinates(rest)?;
            Ok(C2Message::PositionReport(PositionReport {
                id: serial.clone(),
                reporting_unit: originator.clone(),
                domain,
                service,
                reported_at: time,
                latitude,
                longitude,
                label: None,
            }))
        }
    }
}

fn parse(text: &str) -> Result<Vec<Set>, CodecError> {
    let mut sets = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let end = rest
            .find(SET_END)
            .ok_or_else(|| CodecError::Decode("set is not terminated by //".to_string()))?;
        let body = &rest[..end];
        let (id, fields) = body
            .split_once(FIELD)
            .ok_or_else(|| CodecError::Decode(format!("set {body} has no fields")))?;
        let fields = match is_free_text(id) {
            true => vec![fields.to_string()],
            false => fields.split(FIELD).map(str::to_string).collect(),
        };
        sets.push(Set {
            id: id.to_string(),
            fields: fields
                .into_iter()
                .map(|field| match field.as_str() {
                    NO_DATA => String::new(),
                    _ => field,
                })
                .collect(),
        });
        rest = rest[end + SET_END.len()..].trim_start();
    }
    Ok(sets)
}

fn is_free_text(id: &str) -> bool {
    matches!(id, "AMPN" | "NARR")
}

fn find<'a>(sets: &'a [Set], id: &str) -> Result<&'a Set, CodecError> {
    sets.iter()
        .find(|set| set.id == id)
        .ok_or_else(|| CodecError::Decode(format!("missing {id} set")))
}

fn single(sets: &[Set], id: &str) -> Result<String, CodecError> {
    match find(sets, id)?.fields.as_slice() {
        [value] => Ok(value.clone()),
        other => Err(CodecError::Decode(format!(
            "{id} must carry one field, found {}",
            other.len()
        ))),
    }
}

fn coordinates(sets: &[Set]) -> Result<(f64, f64), CodecError> {
    match find(sets, "POSITION")?.fields.as_slice() {
        [latitude, longitude] => Ok((degrees(latitude)?, degrees(longitude)?)),
        _ => Err(CodecError::Decode(
            "POSITION must carry a latitude and a longitude".to_string(),
        )),
    }
}

fn degrees(value: &str) -> Result<f64, CodecError> {
    value
        .parse()
        .map_err(|e| CodecError::Decode(format!("coordinate {value}: {e}")))
}

fn field(value: &str) -> Result<String, CodecError> {
    match value {
        "" => Ok(NO_DATA.to_string()),
        NO_DATA => Err(CodecError::Encode(
            "a field cannot hold the no-data sign - itself".to_string(),
        )),
        _ if value.contains(FIELD) || value.contains(['\n', '\r']) => Err(CodecError::Encode(
            format!("field {value:?} contains a separator or a line break"),
        )),
        _ => Ok(value.to_string()),
    }
}

fn set(id: &str, values: &[&str]) -> Result<String, CodecError> {
    let fields: Result<Vec<String>, CodecError> = values.iter().map(|value| field(value)).collect();
    Ok(format!("{id}/{}{SET_END}", fields?.join("/")))
}

fn free_text(id: &str, value: &str) -> Result<String, CodecError> {
    match value {
        "" => Ok(format!("{id}/{NO_DATA}{SET_END}")),
        NO_DATA => Err(CodecError::Encode(
            "free text cannot be the no-data sign - itself".to_string(),
        )),
        _ if value.contains(SET_END)
            || value.contains(['\n', '\r'])
            || value.starts_with(FIELD)
            || value.ends_with(FIELD) =>
        {
            Err(CodecError::Encode(format!(
                "free text {value:?} contains //, starts or ends with /, or contains a line break"
            )))
        }
        _ => Ok(format!("{id}/{value}{SET_END}")),
    }
}

fn msgid(
    identifier: &str,
    originator: &str,
    serial: &str,
    time: &str,
) -> Result<String, CodecError> {
    set("MSGID", &[identifier, originator, serial, month_of(time)?])
}

fn position(latitude: f64, longitude: f64) -> Result<String, CodecError> {
    set("POSITION", &[&latitude.to_string(), &longitude.to_string()])
}

fn class(domain: &Domain, service: &FunctionalService) -> Result<String, CodecError> {
    set("CLASS", &[domain_code(domain), service_code(service)])
}

fn month_of(time: &str) -> Result<&'static str, CodecError> {
    let month: usize = time
        .get(5..7)
        .and_then(|digits| digits.parse().ok())
        .filter(|month| (1..=12).contains(month))
        .ok_or_else(|| {
            CodecError::Invalid(format!("time {time:?} is not an ISO 8601 date-time"))
        })?;
    Ok(MONTHS[month - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact() -> C2Message {
        C2Message::ContactReport(ContactReport {
            id: "C-1".to_string(),
            reporting_unit: "UAV-7".to_string(),
            domain: Domain::Air,
            service: FunctionalService::Intelligence,
            observed_at: "2026-09-29T14:30:00Z".to_string(),
            latitude: 45.5017,
            longitude: -73.5673,
            description: "Vehicle stationary, N/S road".to_string(),
            label: None,
        })
    }

    fn messages() -> Vec<C2Message> {
        vec![
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
                question: String::new(),
                label: None,
            }),
            C2Message::PositionReport(PositionReport {
                id: "POS-node-a".to_string(),
                reporting_unit: "node-a".to_string(),
                domain: Domain::Space,
                service: FunctionalService::Medical,
                reported_at: "2026-10-11T00:45:00Z".to_string(),
                latitude: -33.8688,
                longitude: 151.2093,
                label: None,
            }),
        ]
    }

    fn decode_err(text: &str) -> bool {
        CoieTextCodec.decode(text.as_bytes()).is_err()
    }

    #[test]
    fn contact_report_has_the_documented_layout() {
        let text =
            String::from_utf8(CoieTextCodec.encode(&contact()).expect("encode")).expect("utf8");
        assert_eq!(
            text,
            "MSGID/SPOTREP/UAV-7/C-1/SEP//\nTIME/2026-09-29T14:30:00Z//\nPOSITION/45.5017/-73.5673//\nCLASS/AIR/INTELLIGENCE//\nAMPN/Vehicle stationary, N/S road//"
        );
    }

    #[test]
    fn every_message_kind_round_trips() {
        for msg in messages() {
            let bytes = CoieTextCodec.encode(&msg).expect("encode");
            assert_eq!(CoieTextCodec.decode(&bytes).expect("decode"), msg);
        }
    }

    #[test]
    fn empty_fields_travel_as_the_no_data_sign() {
        let text =
            String::from_utf8(CoieTextCodec.encode(&messages()[2]).expect("encode")).expect("utf8");
        assert!(text.ends_with("NARR/-//"));
    }

    #[test]
    fn decodes_with_flexible_whitespace_between_sets() {
        let text = "  MSGID/RFI/HQ-1/RFI-9/SEP//\r\n\r\nTIME/2026-09-29T16:00:00Z// CLASS/LAND/INTELLIGENCE//\tNARR/Bridge status?//\n";
        assert_eq!(
            CoieTextCodec.decode(text.as_bytes()).expect("decode"),
            C2Message::Rfi(Rfi {
                id: "RFI-9".to_string(),
                requesting_unit: "HQ-1".to_string(),
                domain: Domain::Land,
                service: FunctionalService::Intelligence,
                requested_at: "2026-09-29T16:00:00Z".to_string(),
                question: "Bridge status?".to_string(),
                label: None,
            })
        );
    }

    #[test]
    fn refuses_text_it_cannot_represent() {
        let C2Message::ContactReport(report) = contact() else {
            return;
        };
        for description in ["two // marks", "line\nbreak", "-", "ends with/"] {
            let mut bad = report.clone();
            bad.description = description.to_string();
            assert!(
                CoieTextCodec
                    .encode(&C2Message::ContactReport(bad))
                    .is_err(),
                "{description:?}"
            );
        }
        let mut slashed_unit = report.clone();
        slashed_unit.reporting_unit = "1/RCR".to_string();
        assert!(CoieTextCodec
            .encode(&C2Message::ContactReport(slashed_unit))
            .is_err());
        let mut bad_time = report;
        bad_time.observed_at = "yesterday".to_string();
        assert!(CoieTextCodec
            .encode(&C2Message::ContactReport(bad_time))
            .is_err());
    }

    #[test]
    fn character_oriented_text_converts_to_niem_xml_and_back() {
        let text = CoieTextCodec.encode(&contact()).expect("encode");
        let canonical = CoieTextCodec.decode(&text).expect("decode text");
        let xml = crate::niem_xml::NiemXmlCodec
            .encode(&canonical)
            .expect("encode xml");
        let back = crate::niem_xml::NiemXmlCodec
            .decode(&xml)
            .expect("decode xml");
        assert_eq!(CoieTextCodec.encode(&back).expect("re-encode"), text);
    }

    #[test]
    fn rejects_malformed_messages() {
        let good = "MSGID/POSREP/node-a/P-1/OCT//\nTIME/2026-10-11T00:45:00Z//\nPOSITION/1/2//\nCLASS/LAND/MEDICAL//";
        assert!(CoieTextCodec.decode(good.as_bytes()).is_ok());
        assert!(decode_err(""));
        assert!(decode_err(&good.replace("/OCT//", "/NOV//")));
        assert!(decode_err(&good.replace("POSREP", "OPORD")));
        assert!(decode_err(&good.replace("\nPOSITION/1/2//", "")));
        assert!(decode_err(&good.replace(
            "POSITION/1/2//\nCLASS/LAND/MEDICAL//",
            "CLASS/LAND/MEDICAL//\nPOSITION/1/2//"
        )));
        assert!(decode_err(
            &good.replace("CLASS/LAND/MEDICAL//", "CLASS/LAND/MEDICAL")
        ));
        assert!(decode_err(&good.replace("LAND", "SEA")));
        assert!(decode_err(
            &good.replace("POSITION/1/2//", "POSITION/north/2//")
        ));
        assert!(decode_err(&good.replace(
            "MSGID/POSREP/node-a/P-1/OCT//",
            "MSGID/POSREP/node-a//"
        )));
        assert!(decode_err(&format!("{good}\nRMKS/extra//")));
    }
}
