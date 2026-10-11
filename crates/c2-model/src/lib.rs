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
    PositionReport(PositionReport),
}

impl C2Message {
    pub fn id(&self) -> &str {
        match self {
            C2Message::ContactReport(m) => &m.id,
            C2Message::Sitrep(m) => &m.id,
            C2Message::Rfi(m) => &m.id,
            C2Message::PositionReport(m) => &m.id,
        }
    }

    pub fn label(&self) -> Option<&ConfidentialityLabel> {
        match self {
            C2Message::ContactReport(m) => m.label.as_ref(),
            C2Message::Sitrep(m) => m.label.as_ref(),
            C2Message::Rfi(m) => m.label.as_ref(),
            C2Message::PositionReport(m) => m.label.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfidentialityLabel {
    pub policy: Policy,
    pub classification: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<Category>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub name: String,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub kind: CategoryKind,
    pub tag: String,
    pub uri: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CategoryKind {
    Permissive,
    Restrictive,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ConfidentialityLabel>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ConfidentialityLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rfi {
    pub id: String,
    pub requesting_unit: String,
    pub domain: Domain,
    pub service: FunctionalService,
    pub requested_at: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ConfidentialityLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionReport {
    pub id: String,
    pub reporting_unit: String,
    pub domain: Domain,
    pub service: FunctionalService,
    pub reported_at: String,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ConfidentialityLabel>,
}
