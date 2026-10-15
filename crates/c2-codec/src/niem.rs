use c2_model::C2Message;

use crate::CodecError;

pub const C2F: &str = "https://laboverwire.com/ns/c2-fusion/1.0/";
pub const NC: &str = "https://docs.oasis-open.org/niemopen/ns/model/niem-core/6.0/";
pub const MO: &str =
    "https://docs.oasis-open.org/niemopen/ns/model/domains/militaryOperations/6.0/";
pub const USMTF: &str = "https://docs.oasis-open.org/niemopen/ns/model/domains/usmtf/6.0/";

const MAX_AVAILABLE_PERSONNEL: u32 = 9999;
const UNIT_NAME_LENGTH: std::ops::RangeInclusive<usize> = 4..=100;

pub fn check(msg: &C2Message) -> Result<(), CodecError> {
    msg.validate()?;
    match msg {
        C2Message::ContactReport(m) => zulu("mo:ObservationDateTime", &m.observed_at),
        C2Message::Sitrep(m) => match m.personnel_effective <= MAX_AVAILABLE_PERSONNEL {
            true => Ok(()),
            false => Err(CodecError::Invalid(format!(
                "personnel effective {} exceeds usmtf:AvailablePersonnelQuantity range 0..={MAX_AVAILABLE_PERSONNEL}",
                m.personnel_effective
            ))),
        },
        C2Message::Rfi(_) => Ok(()),
        C2Message::PositionReport(m) => unit_name(&m.reporting_unit),
    }
}

fn unit_name(name: &str) -> Result<(), CodecError> {
    let token = !name.starts_with(' ')
        && !name.ends_with(' ')
        && !name.contains("  ")
        && !name.contains(['\t', '\n', '\r']);
    match token && UNIT_NAME_LENGTH.contains(&name.chars().count()) {
        true => Ok(()),
        false => Err(CodecError::Invalid(format!(
            "mo:UnitName {name:?} must be a token of 4 to 100 characters"
        ))),
    }
}

fn zulu(element: &str, value: &str) -> Result<(), CodecError> {
    match value.ends_with('Z') {
        true => Ok(()),
        false => Err(CodecError::Invalid(format!(
            "{element} {value:?} must be in UTC (Z)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names_follow_the_token_and_length_facets() {
        assert!(unit_name("node-a").is_ok());
        assert!(unit_name("1st Battalion RCR").is_ok());
        for bad in ["HQ", " node-a", "node-a ", "two  spaces", "tab\tname"] {
            assert!(unit_name(bad).is_err(), "{bad:?}");
        }
        assert!(unit_name(&"x".repeat(101)).is_err());
    }
}
