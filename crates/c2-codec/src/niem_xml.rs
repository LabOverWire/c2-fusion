use std::collections::HashMap;

use c2_model::{C2Message, ContactReport, Domain, FunctionalService, PositionReport, Rfi, Sitrep};
use quick_xml::events::{BytesDecl, BytesStart, BytesText, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::Writer;

use crate::codes::{domain_code, domain_from, service_code, service_from};
use crate::niem::{check, C2F, MO, NC, USMTF};
use crate::{refuse_label, Codec, CodecError};

const CONTACT_REPORT: &str = "c2f:ContactReportMessage";
const SITUATION_REPORT: &str = "c2f:SituationReportMessage";
const INFORMATION_REQUEST: &str = "c2f:InformationRequestMessage";
const POSITION_REPORT: &str = "c2f:PositionReportMessage";

const AUTHOR_NAME: &str = "nc:DocumentAuthor/nc:EntityOrganization/nc:OrganizationName";
const DOCUMENT_ID: &str = "nc:DocumentIdentification/nc:IdentificationID";
const CREATED_AT: &str = "nc:DocumentCreationDate/nc:DateTime";
const DOMAIN_CODE: &str = "c2f:OperationalDomainCode";
const SERVICE_CODE: &str = "c2f:FunctionalServiceCode";
const OBSERVED_COORDINATE: &str =
    "mo:Observation/mo:ObservedObjectLocation/mo:ObservedObject2DGeospatialCoordinate";
const UNIT_COORDINATE: &str = "mo:Unit/mo:UnitPresentLocation/nc:Location2DGeospatialCoordinate";
const LATITUDE: &str = "nc:GeographicCoordinateLatitude/nc:LatitudeDegreeValue";
const LONGITUDE: &str = "nc:GeographicCoordinateLongitude/nc:LongitudeDegreeValue";

pub struct NiemXmlCodec;

type XmlWriter = Writer<Vec<u8>>;

impl Codec for NiemXmlCodec {
    fn wire_format(&self) -> &'static str {
        "niem-xml"
    }

    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError> {
        refuse_label(msg, self.wire_format())?;
        check(msg)?;
        let mut w = Writer::new(Vec::new());
        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .map_err(encode_error)?;
        match msg {
            C2Message::ContactReport(m) => {
                open_root(&mut w, CONTACT_REPORT)?;
                author(&mut w, &m.reporting_unit)?;
                leaf(&mut w, "nc:DocumentDescriptionText", &m.description)?;
                identification(&mut w, "nc:DocumentIdentification", &m.id)?;
                open(&mut w, "mo:Observation")?;
                open(&mut w, "mo:ObservedObjectLocation")?;
                coordinate(
                    &mut w,
                    "mo:ObservedObject2DGeospatialCoordinate",
                    m.latitude,
                    m.longitude,
                )?;
                close(&mut w, "mo:ObservedObjectLocation")?;
                leaf(&mut w, "mo:ObservationDateTime", &m.observed_at)?;
                close(&mut w, "mo:Observation")?;
                codes(&mut w, &m.domain, &m.service)?;
                close(&mut w, CONTACT_REPORT)?;
            }
            C2Message::Sitrep(m) => {
                open_root(&mut w, SITUATION_REPORT)?;
                author(&mut w, &m.reporting_unit)?;
                date(&mut w, "nc:DocumentCreationDate", &m.reported_at)?;
                identification(&mut w, "nc:DocumentIdentification", &m.id)?;
                leaf(&mut w, "nc:DocumentSummaryText", &m.summary)?;
                leaf(
                    &mut w,
                    "usmtf:AvailablePersonnelQuantity",
                    &m.personnel_effective.to_string(),
                )?;
                codes(&mut w, &m.domain, &m.service)?;
                close(&mut w, SITUATION_REPORT)?;
            }
            C2Message::Rfi(m) => {
                open_root(&mut w, INFORMATION_REQUEST)?;
                date(&mut w, "nc:RequestDate", &m.requested_at)?;
                identification(&mut w, "nc:RequestIdentification", &m.id)?;
                leaf(&mut w, "nc:RequestText", &m.question)?;
                open(&mut w, "c2f:RequestingOrganization")?;
                leaf(&mut w, "nc:OrganizationName", &m.requesting_unit)?;
                close(&mut w, "c2f:RequestingOrganization")?;
                codes(&mut w, &m.domain, &m.service)?;
                close(&mut w, INFORMATION_REQUEST)?;
            }
            C2Message::PositionReport(m) => {
                open_root(&mut w, POSITION_REPORT)?;
                date(&mut w, "nc:DocumentCreationDate", &m.reported_at)?;
                identification(&mut w, "nc:DocumentIdentification", &m.id)?;
                open(&mut w, "mo:Unit")?;
                leaf(&mut w, "mo:UnitName", &m.reporting_unit)?;
                open(&mut w, "mo:UnitPresentLocation")?;
                coordinate(
                    &mut w,
                    "nc:Location2DGeospatialCoordinate",
                    m.latitude,
                    m.longitude,
                )?;
                close(&mut w, "mo:UnitPresentLocation")?;
                close(&mut w, "mo:Unit")?;
                codes(&mut w, &m.domain, &m.service)?;
                close(&mut w, POSITION_REPORT)?;
            }
        }
        Ok(w.into_inner())
    }

    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError> {
        let text = std::str::from_utf8(bytes).map_err(|e| CodecError::Decode(e.to_string()))?;
        let msg = message(&Document::parse(text)?)?;
        check(&msg)?;
        Ok(msg)
    }
}

fn message(document: &Document) -> Result<C2Message, CodecError> {
    match document.root.as_str() {
        CONTACT_REPORT => Ok(C2Message::ContactReport(ContactReport {
            id: document.text(DOCUMENT_ID)?,
            reporting_unit: document.text(AUTHOR_NAME)?,
            domain: domain_from(&document.text(DOMAIN_CODE)?)?,
            service: service_from(&document.text(SERVICE_CODE)?)?,
            observed_at: document.text("mo:Observation/mo:ObservationDateTime")?,
            latitude: document.degrees(&format!("{OBSERVED_COORDINATE}/{LATITUDE}"))?,
            longitude: document.degrees(&format!("{OBSERVED_COORDINATE}/{LONGITUDE}"))?,
            description: document.text("nc:DocumentDescriptionText")?,
            label: None,
        })),
        SITUATION_REPORT => Ok(C2Message::Sitrep(Sitrep {
            id: document.text(DOCUMENT_ID)?,
            reporting_unit: document.text(AUTHOR_NAME)?,
            domain: domain_from(&document.text(DOMAIN_CODE)?)?,
            service: service_from(&document.text(SERVICE_CODE)?)?,
            reported_at: document.text(CREATED_AT)?,
            summary: document.text("nc:DocumentSummaryText")?,
            personnel_effective: document
                .text("usmtf:AvailablePersonnelQuantity")?
                .trim()
                .parse()
                .map_err(|e| CodecError::Decode(format!("personnel quantity: {e}")))?,
            label: None,
        })),
        INFORMATION_REQUEST => Ok(C2Message::Rfi(Rfi {
            id: document.text("nc:RequestIdentification/nc:IdentificationID")?,
            requesting_unit: document.text("c2f:RequestingOrganization/nc:OrganizationName")?,
            domain: domain_from(&document.text(DOMAIN_CODE)?)?,
            service: service_from(&document.text(SERVICE_CODE)?)?,
            requested_at: document.text("nc:RequestDate/nc:DateTime")?,
            question: document.text("nc:RequestText")?,
            label: None,
        })),
        POSITION_REPORT => Ok(C2Message::PositionReport(PositionReport {
            id: document.text(DOCUMENT_ID)?,
            reporting_unit: document.text("mo:Unit/mo:UnitName")?,
            domain: domain_from(&document.text(DOMAIN_CODE)?)?,
            service: service_from(&document.text(SERVICE_CODE)?)?,
            reported_at: document.text(CREATED_AT)?,
            latitude: document.degrees(&format!("{UNIT_COORDINATE}/{LATITUDE}"))?,
            longitude: document.degrees(&format!("{UNIT_COORDINATE}/{LONGITUDE}"))?,
            label: None,
        })),
        other => Err(CodecError::Decode(format!(
            "unknown NIEM message root {other}"
        ))),
    }
}

struct Document {
    root: String,
    texts: HashMap<String, String>,
}

impl Document {
    fn parse(xml: &str) -> Result<Self, CodecError> {
        let mut reader = NsReader::from_str(xml);
        let mut path: Vec<String> = Vec::new();
        let mut root = None;
        let mut texts = HashMap::new();
        let mut closed = false;
        loop {
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|e| CodecError::Decode(e.to_string()))?;
            if closed {
                match event {
                    Event::Eof => break,
                    Event::Text(text) if text.xml10_content().trim().is_empty() => continue,
                    Event::Comment(_) | Event::PI(_) => continue,
                    _ => {
                        return Err(CodecError::Decode(
                            "content after the root element".to_string(),
                        ))
                    }
                }
            }
            match event {
                Event::Start(start) => {
                    let name = qualified(&namespace, &start);
                    match root {
                        None => root = Some(name),
                        Some(_) => {
                            path.push(name);
                            open_element(&mut texts, &path)?;
                        }
                    }
                }
                Event::Empty(start) => {
                    let name = qualified(&namespace, &start);
                    if root.is_none() {
                        root = Some(name);
                        closed = true;
                        continue;
                    }
                    path.push(name);
                    open_element(&mut texts, &path)?;
                    path.pop();
                }
                Event::Text(text) => append(&mut texts, &path, &text.xml10_content()),
                Event::CData(data) => append(&mut texts, &path, &data.xml10_content()),
                Event::GeneralRef(reference) => {
                    let resolved = match reference
                        .resolve_char_ref()
                        .map_err(|e| CodecError::Decode(e.to_string()))?
                    {
                        Some(ch) => ch.to_string(),
                        None => predefined_entity(&reference.xml10_content())?.to_string(),
                    };
                    append(&mut texts, &path, &resolved);
                }
                Event::End(_) => {
                    if path.pop().is_none() {
                        closed = true;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(Self {
            root: root.ok_or_else(|| CodecError::Decode("empty document".to_string()))?,
            texts,
        })
    }

    fn text(&self, path: &str) -> Result<String, CodecError> {
        self.texts
            .get(path)
            .cloned()
            .ok_or_else(|| CodecError::Decode(format!("missing {path}")))
    }

    fn degrees(&self, path: &str) -> Result<f64, CodecError> {
        self.text(path)?
            .trim()
            .parse()
            .map_err(|e| CodecError::Decode(format!("{path}: {e}")))
    }
}

fn open_element(texts: &mut HashMap<String, String>, path: &[String]) -> Result<(), CodecError> {
    let key = path.join("/");
    match texts.contains_key(&key) {
        true => Err(CodecError::Decode(format!("repeated element {key}"))),
        false => {
            texts.insert(key, String::new());
            Ok(())
        }
    }
}

fn append(texts: &mut HashMap<String, String>, path: &[String], value: &str) {
    texts.entry(path.join("/")).or_default().push_str(value);
}

fn predefined_entity(name: &str) -> Result<char, CodecError> {
    match name {
        "amp" => Ok('&'),
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "quot" => Ok('"'),
        "apos" => Ok('\''),
        other => Err(CodecError::Decode(format!("undefined entity &{other};"))),
    }
}

fn qualified(namespace: &ResolveResult<'_>, start: &BytesStart<'_>) -> String {
    let local = start.local_name().into_inner();
    let prefix = match namespace {
        ResolveResult::Bound(ns) => match ns.into_inner() {
            C2F => "c2f",
            NC => "nc",
            MO => "mo",
            USMTF => "usmtf",
            other => return format!("{{{other}}}{local}"),
        },
        _ => return local.to_string(),
    };
    format!("{prefix}:{local}")
}

fn encode_error(error: impl std::fmt::Display) -> CodecError {
    CodecError::Encode(error.to_string())
}

fn open_root(w: &mut XmlWriter, name: &str) -> Result<(), CodecError> {
    let root = BytesStart::new(name).with_attributes([
        ("xmlns:c2f", C2F),
        ("xmlns:nc", NC),
        ("xmlns:mo", MO),
        ("xmlns:usmtf", USMTF),
    ]);
    w.write_event(Event::Start(root)).map_err(encode_error)
}

fn open(w: &mut XmlWriter, name: &str) -> Result<(), CodecError> {
    w.write_event(Event::Start(BytesStart::new(name)))
        .map_err(encode_error)
}

fn close(w: &mut XmlWriter, name: &str) -> Result<(), CodecError> {
    w.write_event(Event::End(BytesStart::new(name).to_end()))
        .map_err(encode_error)
}

fn leaf(w: &mut XmlWriter, name: &str, value: &str) -> Result<(), CodecError> {
    open(w, name)?;
    w.write_event(Event::Text(BytesText::new(value)))
        .map_err(encode_error)?;
    close(w, name)
}

fn author(w: &mut XmlWriter, unit: &str) -> Result<(), CodecError> {
    open(w, "nc:DocumentAuthor")?;
    open(w, "nc:EntityOrganization")?;
    leaf(w, "nc:OrganizationName", unit)?;
    close(w, "nc:EntityOrganization")?;
    close(w, "nc:DocumentAuthor")
}

fn identification(w: &mut XmlWriter, name: &str, id: &str) -> Result<(), CodecError> {
    open(w, name)?;
    leaf(w, "nc:IdentificationID", id)?;
    close(w, name)
}

fn date(w: &mut XmlWriter, name: &str, value: &str) -> Result<(), CodecError> {
    open(w, name)?;
    leaf(w, "nc:DateTime", value)?;
    close(w, name)
}

fn coordinate(
    w: &mut XmlWriter,
    name: &str,
    latitude: f64,
    longitude: f64,
) -> Result<(), CodecError> {
    open(w, name)?;
    open(w, "nc:GeographicCoordinateLatitude")?;
    leaf(w, "nc:LatitudeDegreeValue", &latitude.to_string())?;
    close(w, "nc:GeographicCoordinateLatitude")?;
    open(w, "nc:GeographicCoordinateLongitude")?;
    leaf(w, "nc:LongitudeDegreeValue", &longitude.to_string())?;
    close(w, "nc:GeographicCoordinateLongitude")?;
    close(w, name)
}

fn codes(
    w: &mut XmlWriter,
    domain: &Domain,
    service: &FunctionalService,
) -> Result<(), CodecError> {
    leaf(w, DOMAIN_CODE, domain_code(domain))?;
    leaf(w, SERVICE_CODE, service_code(service))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

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
                description: "Vehicle <light> & \"stationary\" near Saint-Rémi".to_string(),
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
                domain: Domain::Space,
                service: FunctionalService::Medical,
                reported_at: "2026-10-11T00:45:00Z".to_string(),
                latitude: -33.8688,
                longitude: 151.2093,
                label: None,
            }),
        ]
    }

    fn schema() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/niem/extension/c2-fusion.xsd")
    }

    fn validate(name: &str, xml: &[u8]) -> Result<(), String> {
        let path = std::env::temp_dir().join(format!("c2-niem-{}-{name}.xml", std::process::id()));
        std::fs::write(&path, xml).map_err(|e| e.to_string())?;
        let output = Command::new("xmllint")
            .arg("--noout")
            .arg("--schema")
            .arg(schema())
            .arg(&path)
            .output()
            .map_err(|e| format!("xmllint is required for NIEM validation tests (libxml2): {e}"))?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        match output.status.success() {
            true => Ok(()),
            false => Err(String::from_utf8_lossy(&output.stderr).into_owned()),
        }
    }

    #[test]
    fn every_message_kind_round_trips() {
        for msg in messages() {
            let bytes = NiemXmlCodec.encode(&msg).expect("encode");
            assert_eq!(NiemXmlCodec.decode(&bytes).expect("decode"), msg);
        }
    }

    #[test]
    fn every_encoded_message_validates_against_the_niem_schemas() {
        for (index, msg) in messages().iter().enumerate() {
            let bytes = NiemXmlCodec.encode(msg).expect("encode");
            if let Err(errors) = validate(&index.to_string(), &bytes) {
                panic!("message {index} failed NIEM validation:\n{errors}");
            }
        }
    }

    #[test]
    fn published_samples_validate_and_decode() {
        let samples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/niem/samples");
        let expected = [
            (
                "contact.xml",
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
            ),
            ("sitrep.xml", messages().remove(1)),
            ("rfi.xml", messages().remove(2)),
            (
                "position.xml",
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
            ),
        ];
        for (file, message) in expected {
            let bytes = std::fs::read(samples.join(file)).expect("sample");
            if let Err(errors) = validate(file, &bytes) {
                panic!("{file} failed NIEM validation:\n{errors}");
            }
            assert_eq!(
                NiemXmlCodec.decode(&bytes).expect("decode"),
                message,
                "{file}"
            );
        }
    }

    #[test]
    fn schema_rejects_an_invalid_instance() {
        let bytes = NiemXmlCodec.encode(&messages()[0]).expect("encode");
        let invalid = String::from_utf8(bytes)
            .expect("utf8")
            .replace(">45.5017<", ">200<");
        assert!(validate("invalid", invalid.as_bytes()).is_err());
    }

    #[test]
    fn decodes_any_namespace_prefixes() {
        let xml = format!(
            "<?xml version=\"1.0\"?><RequestMsg:InformationRequestMessage xmlns:RequestMsg=\"{C2F}\" xmlns=\"{NC}\"><RequestDate><DateTime>2026-09-29T16:00:00Z</DateTime></RequestDate><RequestIdentification><IdentificationID>RFI-7</IdentificationID></RequestIdentification><RequestText><![CDATA[Bridge <intact>?]]> &#x2713; &amp; out</RequestText><RequestMsg:RequestingOrganization><OrganizationName>HQ-2</OrganizationName></RequestMsg:RequestingOrganization><RequestMsg:OperationalDomainCode> LAND </RequestMsg:OperationalDomainCode><RequestMsg:FunctionalServiceCode>INTELLIGENCE</RequestMsg:FunctionalServiceCode></RequestMsg:InformationRequestMessage>"
        );
        assert_eq!(
            NiemXmlCodec.decode(xml.as_bytes()).expect("decode"),
            C2Message::Rfi(Rfi {
                id: "RFI-7".to_string(),
                requesting_unit: "HQ-2".to_string(),
                domain: Domain::Land,
                service: FunctionalService::Intelligence,
                requested_at: "2026-09-29T16:00:00Z".to_string(),
                question: "Bridge <intact>? \u{2713} & out".to_string(),
                label: None,
            })
        );
    }

    #[test]
    fn refuses_values_that_would_break_the_schemas() {
        let C2Message::Sitrep(mut sitrep) = messages().remove(1) else {
            return;
        };
        sitrep.personnel_effective = 10_000;
        assert!(NiemXmlCodec.encode(&C2Message::Sitrep(sitrep)).is_err());
        let C2Message::PositionReport(position) = messages().remove(3) else {
            return;
        };
        for latitude in [f64::NAN, 90.5, -91.0] {
            let mut bad = position.clone();
            bad.latitude = latitude;
            assert!(NiemXmlCodec
                .encode(&C2Message::PositionReport(bad))
                .is_err());
        }
        let mut short_name = position.clone();
        short_name.reporting_unit = "HQ".to_string();
        assert!(NiemXmlCodec
            .encode(&C2Message::PositionReport(short_name))
            .is_err());
        let mut bad_time = position;
        bad_time.reported_at = "yesterday".to_string();
        assert!(NiemXmlCodec
            .encode(&C2Message::PositionReport(bad_time))
            .is_err());
        let C2Message::ContactReport(mut contact) = messages().remove(0) else {
            return;
        };
        contact.observed_at = "2026-09-29T14:30:00-07:00".to_string();
        assert!(NiemXmlCodec
            .encode(&C2Message::ContactReport(contact))
            .is_err());
    }

    #[test]
    fn rejects_unknown_or_incomplete_documents() {
        assert!(NiemXmlCodec
            .decode(format!("<c2f:Other xmlns:c2f=\"{C2F}\"/>").as_bytes())
            .is_err());
        let complete = NiemXmlCodec.encode(&messages()[2]).expect("encode");
        let text = String::from_utf8(complete).expect("utf8");
        let without_question = text.replace("<nc:RequestText>Fuel state?</nc:RequestText>", "");
        assert!(NiemXmlCodec.decode(without_question.as_bytes()).is_err());
        assert!(NiemXmlCodec.decode(b"").is_err());
        assert!(NiemXmlCodec
            .decode(&text.as_bytes()[..text.len() / 2])
            .is_err());
    }
}
