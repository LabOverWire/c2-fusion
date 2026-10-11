use std::time::Duration;

use crate::nmea::Rmc;

pub const PAIRING_WINDOW: Duration = Duration::from_millis(900);

const NANOS_PER_SECOND: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpsEdge {
    pub monotonic_nanos: u64,
    pub system_nanos: i64,
}

pub fn clock_offset(edge: PpsEdge, rmc: Rmc, monotonic_now_nanos: u64) -> Option<i64> {
    let gnss_nanos = rmc.unix_nanos.filter(|_| rmc.valid)?;
    let age = Duration::from_nanos(monotonic_now_nanos.checked_sub(edge.monotonic_nanos)?);
    match (gnss_nanos % NANOS_PER_SECOND == 0, age < PAIRING_WINDOW) {
        (true, true) => Some(gnss_nanos - edge.system_nanos),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_791_667_199 * NANOS_PER_SECOND;

    fn edge() -> PpsEdge {
        PpsEdge {
            monotonic_nanos: 5_000_000_000,
            system_nanos: SECOND - 2_500_000,
        }
    }

    fn rmc(unix_nanos: i64) -> Rmc {
        Rmc {
            valid: true,
            unix_nanos: Some(unix_nanos),
        }
    }

    #[test]
    fn offset_is_gnss_second_minus_system_time_at_edge() {
        assert_eq!(
            clock_offset(edge(), rmc(SECOND), 5_120_000_000),
            Some(2_500_000)
        );
    }

    #[test]
    fn stale_edge_is_not_paired() {
        assert_eq!(clock_offset(edge(), rmc(SECOND), 5_900_000_000), None);
    }

    #[test]
    fn edge_after_sentence_is_not_paired() {
        assert_eq!(clock_offset(edge(), rmc(SECOND), 4_999_000_000), None);
    }

    #[test]
    fn invalid_or_fractional_time_is_not_paired() {
        let invalid = Rmc {
            valid: false,
            unix_nanos: Some(SECOND),
        };
        assert_eq!(clock_offset(edge(), invalid, 5_120_000_000), None);
        assert_eq!(
            clock_offset(edge(), rmc(SECOND + 500_000_000), 5_120_000_000),
            None
        );
    }
}
