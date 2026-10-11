use std::collections::HashMap;

use c2_model::{C2Message, ContactReport, PositionReport, Rfi, Sitrep};
use serde_json::{json, Map, Value};

use crate::codes::{domain_code, domain_from, service_code, service_from};
use crate::niem::{check, C2F, MO, NC, USMTF};
use crate::{refuse_label, Codec, CodecError};

const CONTEXT: &str = "@context";
const CONTACT_REPORT: &str = "c2f:ContactReportMessage";
const SITUATION_REPORT: &str = "c2f:SituationReportMessage";
const INFORMATION_REQUEST: &str = "c2f:InformationRequestMessage";
const POSITION_REPORT: &str = "c2f:PositionReportMessage";
const TEXT: &str = "nc:TextLiteral";

const AUTHOR_NAME: &[&str] = &[
    "nc:DocumentAuthor",
    "nc:EntityOrganization",
    "nc:OrganizationName",
    TEXT,
];
const DOCUMENT_ID: &[&str] = &["nc:DocumentIdentification", "nc:IdentificationID"];
const CREATED_AT: &[&str] = &["nc:DocumentCreationDate", "nc:DateTime"];
const DOMAIN_CODE: &str = "c2f:OperationalDomainCode";
const SERVICE_CODE: &str = "c2f:FunctionalServiceCode";

pub struct NiemJsonCodec;

impl Codec for NiemJsonCodec {
    fn wire_format(&self) -> &'static str {
        "niem-json"
    }

    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError> {
        refuse_label(msg, self.wire_format())?;
        check(msg)?;
        let (root, body) = match msg {
            C2Message::ContactReport(m) => (
                CONTACT_REPORT,
                json!({
                    "nc:DocumentAuthor": author(&m.reporting_unit),
                    "nc:DocumentDescriptionText": [text(&m.description)],
                    "nc:DocumentIdentification": identification(&m.id),
                    "mo:Observation": {
                        "mo:ObservedObjectLocation": [{
                            "mo:ObservedObject2DGeospatialCoordinate": [coordinate(m.latitude, m.longitude)],
                        }],
                        "mo:ObservationDateTime": [m.observed_at],
                    },
                    DOMAIN_CODE: domain_code(&m.domain),
                    SERVICE_CODE: service_code(&m.service),
                }),
            ),
            C2Message::Sitrep(m) => (
                SITUATION_REPORT,
                json!({
                    "nc:DocumentAuthor": author(&m.reporting_unit),
                    "nc:DocumentCreationDate": [{"nc:DateTime": [m.reported_at]}],
                    "nc:DocumentIdentification": identification(&m.id),
                    "nc:DocumentSummaryText": [text(&m.summary)],
                    "usmtf:AvailablePersonnelQuantity": m.personnel_effective,
                    DOMAIN_CODE: domain_code(&m.domain),
                    SERVICE_CODE: service_code(&m.service),
                }),
            ),
            C2Message::Rfi(m) => (
                INFORMATION_REQUEST,
                json!({
                    "nc:RequestDate": [{"nc:DateTime": [m.requested_at]}],
                    "nc:RequestIdentification": identification(&m.id),
                    "nc:RequestText": [text(&m.question)],
                    "c2f:RequestingOrganization": {"nc:OrganizationName": [text(&m.requesting_unit)]},
                    DOMAIN_CODE: domain_code(&m.domain),
                    SERVICE_CODE: service_code(&m.service),
                }),
            ),
            C2Message::PositionReport(m) => (
                POSITION_REPORT,
                json!({
                    "nc:DocumentCreationDate": [{"nc:DateTime": [m.reported_at]}],
                    "nc:DocumentIdentification": identification(&m.id),
                    "mo:Unit": {
                        "mo:UnitName": [m.reporting_unit],
                        "mo:UnitPresentLocation": [{
                            "nc:Location2DGeospatialCoordinate": [coordinate(m.latitude, m.longitude)],
                        }],
                    },
                    DOMAIN_CODE: domain_code(&m.domain),
                    SERVICE_CODE: service_code(&m.service),
                }),
            ),
        };
        let mut document = Map::new();
        document.insert(
            CONTEXT.to_string(),
            json!({"c2f": C2F, "nc": NC, "mo": MO, "usmtf": USMTF}),
        );
        document.insert(root.to_string(), body);
        serde_json::to_vec(&Value::Object(document)).map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))?;
        let document = value.as_object().ok_or_else(|| {
            CodecError::Decode("a NIEM JSON message must be an object".to_string())
        })?;
        let prefixes = context(document)?;
        let mut roots = document.iter().filter(|(key, _)| key.as_str() != CONTEXT);
        let (root, body) = match (roots.next(), roots.next()) {
            (Some(root), None) => root,
            _ => {
                return Err(CodecError::Decode(
                    "a NIEM JSON message must hold exactly one message property".to_string(),
                ))
            }
        };
        let body = canonical(body, &prefixes)?;
        match canonical_key(root, &prefixes)?.as_str() {
            CONTACT_REPORT => Ok(C2Message::ContactReport(ContactReport {
                id: string(&body, DOCUMENT_ID)?,
                reporting_unit: string(&body, AUTHOR_NAME)?,
                domain: domain_from(&string(&body, &[DOMAIN_CODE])?)?,
                service: service_from(&string(&body, &[SERVICE_CODE])?)?,
                observed_at: string(&body, &["mo:Observation", "mo:ObservationDateTime"])?,
                latitude: degrees(&body, &observed(LATITUDE))?,
                longitude: degrees(&body, &observed(LONGITUDE))?,
                description: string(&body, &["nc:DocumentDescriptionText", TEXT])?,
                label: None,
            })),
            SITUATION_REPORT => Ok(C2Message::Sitrep(Sitrep {
                id: string(&body, DOCUMENT_ID)?,
                reporting_unit: string(&body, AUTHOR_NAME)?,
                domain: domain_from(&string(&body, &[DOMAIN_CODE])?)?,
                service: service_from(&string(&body, &[SERVICE_CODE])?)?,
                reported_at: string(&body, CREATED_AT)?,
                summary: string(&body, &["nc:DocumentSummaryText", TEXT])?,
                personnel_effective: find(&body, &["usmtf:AvailablePersonnelQuantity"])?
                    .as_u64()
                    .and_then(|quantity| u32::try_from(quantity).ok())
                    .ok_or_else(|| {
                        CodecError::Decode("personnel quantity is not a count".to_string())
                    })?,
                label: None,
            })),
            INFORMATION_REQUEST => Ok(C2Message::Rfi(Rfi {
                id: string(&body, &["nc:RequestIdentification", "nc:IdentificationID"])?,
                requesting_unit: string(
                    &body,
                    &["c2f:RequestingOrganization", "nc:OrganizationName", TEXT],
                )?,
                domain: domain_from(&string(&body, &[DOMAIN_CODE])?)?,
                service: service_from(&string(&body, &[SERVICE_CODE])?)?,
                requested_at: string(&body, &["nc:RequestDate", "nc:DateTime"])?,
                question: string(&body, &["nc:RequestText", TEXT])?,
                label: None,
            })),
            POSITION_REPORT => Ok(C2Message::PositionReport(PositionReport {
                id: string(&body, DOCUMENT_ID)?,
                reporting_unit: string(&body, &["mo:Unit", "mo:UnitName"])?,
                domain: domain_from(&string(&body, &[DOMAIN_CODE])?)?,
                service: service_from(&string(&body, &[SERVICE_CODE])?)?,
                reported_at: string(&body, CREATED_AT)?,
                latitude: degrees(&body, &unit(LATITUDE))?,
                longitude: degrees(&body, &unit(LONGITUDE))?,
                label: None,
            })),
            other => Err(CodecError::Decode(format!(
                "unknown NIEM message property {other}"
            ))),
        }
    }
}

const LATITUDE: [&str; 2] = ["nc:GeographicCoordinateLatitude", "nc:LatitudeDegreeValue"];
const LONGITUDE: [&str; 2] = [
    "nc:GeographicCoordinateLongitude",
    "nc:LongitudeDegreeValue",
];

fn observed(axis: [&'static str; 2]) -> [&'static str; 5] {
    [
        "mo:Observation",
        "mo:ObservedObjectLocation",
        "mo:ObservedObject2DGeospatialCoordinate",
        axis[0],
        axis[1],
    ]
}

fn unit(axis: [&'static str; 2]) -> [&'static str; 5] {
    [
        "mo:Unit",
        "mo:UnitPresentLocation",
        "nc:Location2DGeospatialCoordinate",
        axis[0],
        axis[1],
    ]
}

fn text(value: &str) -> Value {
    json!({ TEXT: value })
}

fn author(unit: &str) -> Value {
    json!([{"nc:EntityOrganization": [{"nc:OrganizationName": [text(unit)]}]}])
}

fn identification(id: &str) -> Value {
    json!([{"nc:IdentificationID": [id]}])
}

fn coordinate(latitude: f64, longitude: f64) -> Value {
    json!({
        "nc:GeographicCoordinateLatitude": {"nc:LatitudeDegreeValue": [latitude]},
        "nc:GeographicCoordinateLongitude": {"nc:LongitudeDegreeValue": [longitude]},
    })
}

fn context(document: &Map<String, Value>) -> Result<HashMap<String, String>, CodecError> {
    let terms = document
        .get(CONTEXT)
        .and_then(Value::as_object)
        .ok_or_else(|| CodecError::Decode("missing embedded @context object".to_string()))?;
    terms
        .iter()
        .map(|(prefix, uri)| match uri.as_str() {
            Some(uri) => Ok((prefix.clone(), uri.to_string())),
            None => Err(CodecError::Decode(format!(
                "@context term {prefix} is not a namespace URI"
            ))),
        })
        .collect()
}

fn canonical_key(key: &str, prefixes: &HashMap<String, String>) -> Result<String, CodecError> {
    let (prefix, local) = key
        .split_once(':')
        .ok_or_else(|| CodecError::Decode(format!("key {key} has no namespace prefix")))?;
    let uri = prefixes
        .get(prefix)
        .ok_or_else(|| CodecError::Decode(format!("prefix {prefix} is not in @context")))?;
    let canonical = match uri.as_str() {
        C2F => "c2f",
        NC => "nc",
        MO => "mo",
        USMTF => "usmtf",
        other => return Ok(format!("{{{other}}}{local}")),
    };
    Ok(format!("{canonical}:{local}"))
}

fn canonical(value: &Value, prefixes: &HashMap<String, String>) -> Result<Value, CodecError> {
    match value {
        Value::Object(object) => object
            .iter()
            .map(|(key, child)| Ok((canonical_key(key, prefixes)?, canonical(child, prefixes)?)))
            .collect::<Result<Map<String, Value>, CodecError>>()
            .map(Value::Object),
        Value::Array(items) => items
            .iter()
            .map(|item| canonical(item, prefixes))
            .collect::<Result<Vec<Value>, CodecError>>()
            .map(Value::Array),
        other => Ok(other.clone()),
    }
}

fn find<'a>(body: &'a Value, path: &[&str]) -> Result<&'a Value, CodecError> {
    let mut node = body;
    for key in path {
        node = node
            .get(*key)
            .ok_or_else(|| CodecError::Decode(format!("missing {}", path.join("/"))))?;
        if let Value::Array(items) = node {
            node = items
                .first()
                .ok_or_else(|| CodecError::Decode(format!("empty {}", path.join("/"))))?;
        }
    }
    Ok(node)
}

fn string(body: &Value, path: &[&str]) -> Result<String, CodecError> {
    find(body, path)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| CodecError::Decode(format!("{} is not a string", path.join("/"))))
}

fn degrees(body: &Value, path: &[&str]) -> Result<f64, CodecError> {
    find(body, path)?
        .as_f64()
        .ok_or_else(|| CodecError::Decode(format!("{} is not a number", path.join("/"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_model::{Domain, FunctionalService};
    use std::path::PathBuf;

    fn messages() -> Vec<C2Message> {
        vec![
            C2Message::ContactReport(ContactReport {
                id: "C-1".to_string(),
                reporting_unit: "UAV-7".to_string(),
                domain: Domain::Air,
                service: FunctionalService::Intelligence,
                observed_at: "2026-09-29T14:30:00Z".to_string(),
                latitude: 45.5017,
                longitude: -73.5673,
                description: "Vehicle stationary".to_string(),
                label: None,
            }),
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
            C2Message::PositionReport(PositionReport {
                id: "POS-node-a".to_string(),
                reporting_unit: "node-a".to_string(),
                domain: Domain::Land,
                service: FunctionalService::CommandAndControl,
                reported_at: "2026-10-11T00:45:00Z".to_string(),
                latitude: 45.5017,
                longitude: -73.5673,
                label: None,
            }),
        ]
    }

    fn schema_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/niem")
    }

    fn validator() -> jsonschema::Validator {
        let text =
            std::fs::read_to_string(schema_dir().join("json/c2-fusion-messages.schema.json"))
                .expect("schema");
        let schema: Value = serde_json::from_str(&text).expect("schema json");
        jsonschema::validator_for(&schema).expect("compile schema")
    }

    fn errors(validator: &jsonschema::Validator, bytes: &[u8]) -> Vec<String> {
        let instance: Value = serde_json::from_slice(bytes).expect("json");
        validator
            .iter_errors(&instance)
            .map(|error| format!("{} at {}", error, error.instance_path()))
            .collect()
    }

    #[test]
    fn every_message_kind_round_trips() {
        for msg in messages() {
            let bytes = NiemJsonCodec.encode(&msg).expect("encode");
            assert_eq!(NiemJsonCodec.decode(&bytes).expect("decode"), msg);
        }
    }

    #[test]
    fn every_encoded_message_validates_against_the_niem_json_schema() {
        let validator = validator();
        for msg in messages() {
            let bytes = NiemJsonCodec.encode(&msg).expect("encode");
            let found = errors(&validator, &bytes);
            assert!(found.is_empty(), "{}: {found:?}", msg.id());
        }
    }

    #[test]
    fn published_samples_validate_and_decode() {
        let validator = validator();
        for (file, msg) in ["contact", "sitrep", "rfi", "position"]
            .iter()
            .zip(messages())
        {
            let bytes =
                std::fs::read(schema_dir().join(format!("samples/{file}.json"))).expect("sample");
            let found = errors(&validator, &bytes);
            assert!(found.is_empty(), "{file}: {found:?}");
            assert_eq!(NiemJsonCodec.decode(&bytes).expect("decode"), msg, "{file}");
        }
    }

    #[test]
    fn schema_rejects_an_invalid_instance() {
        let bytes = NiemJsonCodec.encode(&messages()[0]).expect("encode");
        let invalid = String::from_utf8(bytes)
            .expect("utf8")
            .replace("45.5017", "200.0");
        assert!(!errors(&validator(), invalid.as_bytes()).is_empty());
        let missing_context =
            String::from_utf8(NiemJsonCodec.encode(&messages()[1]).expect("encode")).expect("utf8");
        let without = missing_context.replacen("\"@context\"", "\"@ignored\"", 1);
        assert!(!errors(&validator(), without.as_bytes()).is_empty());
    }

    #[test]
    fn decodes_prefixes_declared_in_the_context() {
        let json = format!(
            r#"{{"@context":{{"x":"{C2F}","core":"{NC}"}},"x:InformationRequestMessage":{{"core:RequestDate":[{{"core:DateTime":["2026-09-29T16:00:00Z"]}}],"core:RequestIdentification":[{{"core:IdentificationID":["RFI-7"]}}],"core:RequestText":[{{"core:TextLiteral":"Bridge intact?"}}],"x:RequestingOrganization":{{"core:OrganizationName":[{{"core:TextLiteral":"HQ-2"}}]}},"x:OperationalDomainCode":"LAND","x:FunctionalServiceCode":"INTELLIGENCE"}}}}"#
        );
        assert_eq!(
            NiemJsonCodec.decode(json.as_bytes()).expect("decode"),
            C2Message::Rfi(Rfi {
                id: "RFI-7".to_string(),
                requesting_unit: "HQ-2".to_string(),
                domain: Domain::Land,
                service: FunctionalService::Intelligence,
                requested_at: "2026-09-29T16:00:00Z".to_string(),
                question: "Bridge intact?".to_string(),
                label: None,
            })
        );
    }

    #[test]
    fn refuses_values_and_documents_outside_the_exchange() {
        let C2Message::PositionReport(mut position) = messages().remove(3) else {
            return;
        };
        position.reporting_unit = "HQ".to_string();
        assert!(NiemJsonCodec
            .encode(&C2Message::PositionReport(position))
            .is_err());
        assert!(NiemJsonCodec.decode(b"[]").is_err());
        assert!(NiemJsonCodec.decode(b"{}").is_err());
        let bytes = NiemJsonCodec.encode(&messages()[2]).expect("encode");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(NiemJsonCodec
            .decode(
                text.replace("\"mo\":", "\"moo\":")
                    .replace("\"nc\":", "\"core\":")
                    .as_bytes()
            )
            .is_err());
        let mut without_question: Value = serde_json::from_str(&text).expect("json");
        without_question["c2f:InformationRequestMessage"]
            .as_object_mut()
            .expect("message")
            .remove("nc:RequestText");
        assert!(NiemJsonCodec
            .decode(without_question.to_string().as_bytes())
            .is_err());
    }
}
