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
    match msg {
        C2Message::ContactReport(m) => {
            zulu_date_time("mo:ObservationDateTime", &m.observed_at)?;
            coordinate(m.latitude, m.longitude)
        }
        C2Message::Sitrep(m) => {
            date_time("nc:DateTime", &m.reported_at)?;
            if m.personnel_effective > MAX_AVAILABLE_PERSONNEL {
                return Err(CodecError::Encode(format!(
                    "personnel effective {} exceeds usmtf:AvailablePersonnelQuantity range 0..={MAX_AVAILABLE_PERSONNEL}",
                    m.personnel_effective
                )));
            }
            Ok(())
        }
        C2Message::Rfi(m) => date_time("nc:DateTime", &m.requested_at),
        C2Message::PositionReport(m) => {
            date_time("nc:DateTime", &m.reported_at)?;
            unit_name(&m.reporting_unit)?;
            coordinate(m.latitude, m.longitude)
        }
    }
}

fn coordinate(latitude: f64, longitude: f64) -> Result<(), CodecError> {
    match (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude) {
        true => Ok(()),
        false => Err(CodecError::Encode(format!(
            "coordinate {latitude}, {longitude} is outside -90..=90, -180..=180"
        ))),
    }
}

fn unit_name(name: &str) -> Result<(), CodecError> {
    let token = !name.starts_with(' ')
        && !name.ends_with(' ')
        && !name.contains("  ")
        && !name.contains(['\t', '\n', '\r']);
    match token && UNIT_NAME_LENGTH.contains(&name.chars().count()) {
        true => Ok(()),
        false => Err(CodecError::Encode(format!(
            "mo:UnitName {name:?} must be a token of 4 to 100 characters"
        ))),
    }
}

fn zulu_date_time(element: &str, value: &str) -> Result<(), CodecError> {
    date_time(element, value)?;
    match value.ends_with('Z') {
        true => Ok(()),
        false => Err(CodecError::Encode(format!(
            "{element} {value:?} must be in UTC (Z)"
        ))),
    }
}

fn date_time(element: &str, value: &str) -> Result<(), CodecError> {
    match is_date_time(value) {
        true => Ok(()),
        false => Err(CodecError::Encode(format!(
            "{element} {value:?} is not an xs:dateTime"
        ))),
    }
}

fn is_date_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    let number = |range: std::ops::Range<usize>, max: u32| -> bool {
        value
            .get(range)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u32>().ok())
            .is_some_and(|parsed| parsed <= max)
    };
    let shape = bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':';
    let fields = number(0..4, 9999)
        && number(5..7, 12)
        && !value.get(5..7).is_some_and(|month| month == "00")
        && number(8..10, 31)
        && !value.get(8..10).is_some_and(|day| day == "00")
        && number(11..13, 23)
        && number(14..16, 59)
        && number(17..19, 59);
    if !(shape && fields) {
        return false;
    }
    let rest = &value[19..];
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &fraction[digits..]
        }
        None => rest,
    };
    match rest {
        "" | "Z" => true,
        zone => {
            let zone_bytes = zone.as_bytes();
            zone_bytes.len() == 6
                && matches!(zone_bytes[0], b'+' | b'-')
                && zone_bytes[3] == b':'
                && zone[1..3].bytes().all(|byte| byte.is_ascii_digit())
                && zone[4..6].bytes().all(|byte| byte.is_ascii_digit())
                && zone[1..3].parse::<u32>().is_ok_and(|hours| hours <= 14)
                && zone[4..6].parse::<u32>().is_ok_and(|minutes| minutes <= 59)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_rejects_date_times() {
        for good in [
            "2026-09-29T14:30:00Z",
            "2026-09-29T14:30:00",
            "2026-09-29T14:30:00.25Z",
            "2026-09-29T14:30:00-07:00",
        ] {
            assert!(is_date_time(good), "{good}");
        }
        for bad in [
            "yesterday",
            "2026-13-29T14:30:00Z",
            "2026-00-29T14:30:00Z",
            "2026-09-29 14:30:00Z",
            "2026-09-29T24:00:01Z",
            "2026-09-29T14:30:00.Z",
            "2026-09-29T14:30:00+7",
            "26-09-29T14:30:00Z",
        ] {
            assert!(!is_date_time(bad), "{bad}");
        }
    }

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
