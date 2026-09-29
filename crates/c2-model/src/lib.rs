use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Domain {
    Land,
    Air,
    Maritime,
    Space,
    Cyber,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionalService {
    CommandAndControl,
    Intelligence,
    Logistics,
    Medical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message_type")]
pub enum C2Message {
    ContactReport(ContactReport),
    Sitrep(Sitrep),
    Rfi(Rfi),
}

impl C2Message {
    pub fn id(&self) -> &str {
        match self {
            C2Message::ContactReport(m) => &m.id,
            C2Message::Sitrep(m) => &m.id,
            C2Message::Rfi(m) => &m.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContactReport {
    pub id: String,
    pub reporting_unit: String,
    pub domain: Domain,
    pub service: FunctionalService,
    pub observed_at: String,
    pub latitude: f64,
    pub longitude: f64,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sitrep {
    pub id: String,
    pub reporting_unit: String,
    pub domain: Domain,
    pub service: FunctionalService,
    pub reported_at: String,
    pub summary: String,
    pub personnel_effective: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rfi {
    pub id: String,
    pub requesting_unit: String,
    pub domain: Domain,
    pub service: FunctionalService,
    pub requested_at: String,
    pub question: String,
}
