use crate::C2Message;

pub const MAX_ID_BYTES: usize = 255;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum InvalidMessage {
    #[error("id of {0} bytes is outside 1..={MAX_ID_BYTES}")]
    IdLength(usize),
    #[error("coordinate {latitude}, {longitude} is outside -90..=90, -180..=180")]
    Coordinate { latitude: f64, longitude: f64 },
    #[error("{field} {value:?} is not an xs:dateTime")]
    DateTime { field: &'static str, value: String },
}

impl C2Message {
    pub fn validate(&self) -> Result<(), InvalidMessage> {
        let id = self.id().len();
        if !(1..=MAX_ID_BYTES).contains(&id) {
            return Err(InvalidMessage::IdLength(id));
        }
        match self {
            C2Message::ContactReport(m) => {
                date_time("observed_at", &m.observed_at)?;
                coordinate(m.latitude, m.longitude)
            }
            C2Message::Sitrep(m) => date_time("reported_at", &m.reported_at),
            C2Message::Rfi(m) => date_time("requested_at", &m.requested_at),
            C2Message::PositionReport(m) => {
                date_time("reported_at", &m.reported_at)?;
                coordinate(m.latitude, m.longitude)
            }
        }
    }
}

fn coordinate(latitude: f64, longitude: f64) -> Result<(), InvalidMessage> {
    match (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude) {
        true => Ok(()),
        false => Err(InvalidMessage::Coordinate {
            latitude,
            longitude,
        }),
    }
}

fn date_time(field: &'static str, value: &str) -> Result<(), InvalidMessage> {
    match is_date_time(value) {
        true => Ok(()),
        false => Err(InvalidMessage::DateTime {
            field,
            value: value.to_string(),
        }),
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn is_date_time(value: &str) -> bool {
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        value
            .get(range)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|digits| digits.parse().ok())
    };
    let bytes = value.as_bytes();
    let shape = bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':';
    if !shape {
        return false;
    }
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    let rest = &value[19..];
    let (fraction_is_zero, zone) = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            (
                fraction[..digits].bytes().all(|byte| byte == b'0'),
                &fraction[digits..],
            )
        }
        None => (true, rest),
    };
    let date =
        year >= 1 && (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day);
    let time = match hour {
        24 => minute == 0 && second == 0 && fraction_is_zero,
        _ => hour <= 23 && minute <= 59 && second <= 59,
    };
    date && time && is_zone(zone)
}

fn is_zone(zone: &str) -> bool {
    let bytes = zone.as_bytes();
    match bytes {
        [] | [b'Z'] => true,
        [b'+' | b'-', h1, h2, b':', m1, m2]
            if [h1, h2, m1, m2].iter().all(|b| b.is_ascii_digit()) =>
        {
            let hours = u32::from(h1 - b'0') * 10 + u32::from(h2 - b'0');
            let minutes = u32::from(m1 - b'0') * 10 + u32::from(m2 - b'0');
            minutes <= 59 && (hours < 14 || (hours == 14 && minutes == 0))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContactReport, Domain, FunctionalService, Rfi};

    fn contact() -> C2Message {
        C2Message::ContactReport(ContactReport {
            id: "C-1".to_string(),
            reporting_unit: "UAV-7".to_string(),
            domain: Domain::Air,
            service: FunctionalService::Intelligence,
            observed_at: "2026-09-29T14:30:00Z".to_string(),
            latitude: 45.5,
            longitude: -73.5,
            description: "test".to_string(),
            label: None,
        })
    }

    fn with_contact(change: impl FnOnce(&mut ContactReport)) -> C2Message {
        let C2Message::ContactReport(mut report) = contact() else {
            unreachable!()
        };
        change(&mut report);
        C2Message::ContactReport(report)
    }

    #[test]
    fn valid_message_passes() {
        assert_eq!(contact().validate(), Ok(()));
    }

    #[test]
    fn coordinates_must_be_finite_and_in_range() {
        for (latitude, longitude) in [
            (f64::NAN, 0.0),
            (0.0, f64::NAN),
            (f64::INFINITY, 0.0),
            (0.0, f64::NEG_INFINITY),
            (90.1, 0.0),
            (-90.1, 0.0),
            (0.0, 180.1),
            (999.0, 0.0),
        ] {
            let msg = with_contact(|m| {
                m.latitude = latitude;
                m.longitude = longitude;
            });
            assert!(
                matches!(msg.validate(), Err(InvalidMessage::Coordinate { .. })),
                "{latitude}, {longitude}"
            );
        }
        for (latitude, longitude) in [(90.0, 180.0), (-90.0, -180.0), (0.0, 0.0)] {
            let msg = with_contact(|m| {
                m.latitude = latitude;
                m.longitude = longitude;
            });
            assert_eq!(msg.validate(), Ok(()), "{latitude}, {longitude}");
        }
    }

    #[test]
    fn id_must_fit_a_replication_frame() {
        assert_eq!(
            with_contact(|m| m.id = String::new()).validate(),
            Err(InvalidMessage::IdLength(0))
        );
        assert_eq!(
            with_contact(|m| m.id = "x".repeat(MAX_ID_BYTES)).validate(),
            Ok(())
        );
        assert_eq!(
            with_contact(|m| m.id = "x".repeat(MAX_ID_BYTES + 1)).validate(),
            Err(InvalidMessage::IdLength(MAX_ID_BYTES + 1))
        );
    }

    #[test]
    fn every_timestamp_is_checked() {
        let rfi = C2Message::Rfi(Rfi {
            id: "R-1".to_string(),
            requesting_unit: "HQ-1".to_string(),
            domain: Domain::Land,
            service: FunctionalService::Logistics,
            requested_at: "garbage".to_string(),
            question: "?".to_string(),
            label: None,
        });
        assert!(matches!(
            rfi.validate(),
            Err(InvalidMessage::DateTime {
                field: "requested_at",
                ..
            })
        ));
        assert!(matches!(
            with_contact(|m| m.observed_at = "xxxxx10yyyy".to_string()).validate(),
            Err(InvalidMessage::DateTime {
                field: "observed_at",
                ..
            })
        ));
    }

    #[test]
    fn accepts_xs_date_times() {
        for good in [
            "2026-09-29T14:30:00Z",
            "2026-09-29T14:30:00",
            "2026-09-29T14:30:00.25Z",
            "2026-09-29T14:30:00-07:00",
            "2026-09-29T24:00:00Z",
            "2026-09-29T24:00:00.000Z",
            "2028-02-29T00:00:00Z",
            "2000-02-29T00:00:00Z",
            "2026-09-29T14:30:00+14:00",
            "2026-09-29T14:30:00-13:59",
        ] {
            assert!(is_date_time(good), "{good}");
        }
    }

    #[test]
    fn rejects_invalid_date_times() {
        for bad in [
            "yesterday",
            "",
            "2026-13-29T14:30:00Z",
            "2026-00-29T14:30:00Z",
            "2026-09-00T14:30:00Z",
            "2026-02-31T14:30:00Z",
            "2026-02-29T14:30:00Z",
            "1900-02-29T14:30:00Z",
            "2026-04-31T14:30:00Z",
            "0000-01-01T00:00:00Z",
            "2026-09-29 14:30:00Z",
            "2026-09-29T24:00:01Z",
            "2026-09-29T24:01:00Z",
            "2026-09-29T24:00:00.5Z",
            "2026-09-29T14:60:00Z",
            "2026-09-29T14:30:60Z",
            "2026-09-29T14:30:00.Z",
            "2026-09-29T14:30:00+7",
            "2026-09-29T14:30:00+14:59",
            "2026-09-29T14:30:00+15:00",
            "2026-09-29T14:30:00+05:60",
            "2026-09-29T14:30:00Zjunk",
            "26-09-29T14:30:00Z",
            "+026-09-29T14:30:00Z",
            "2026-09-29T14:30:00\u{e9}",
        ] {
            assert!(!is_date_time(bad), "{bad}");
        }
    }
}
