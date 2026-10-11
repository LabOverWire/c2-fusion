const GGA_FIELDS: usize = 14;
const RMC_MIN_FIELDS: usize = 11;
const NANOS_PER_SECOND: f64 = 1e9;
const SECONDS_PER_DAY: i64 = 86_400;
const REPLY_PREFIX: &str = "command,";
const REPLY_ACCEPTED: &str = ",response: OK";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NmeaError {
    #[error("sentence is not valid UTF-8")]
    NotText,
    #[error("sentence does not start with '$'")]
    MissingStart,
    #[error("sentence has no checksum")]
    MissingChecksum,
    #[error("checksum mismatch: expected {expected:02X}, computed {computed:02X}")]
    Checksum { expected: u8, computed: u8 },
    #[error("GGA sentence has {0} fields, expected {GGA_FIELDS}")]
    FieldCount(usize),
    #[error("RMC sentence has {0} fields, expected at least {RMC_MIN_FIELDS}")]
    RmcFieldCount(usize),
    #[error("invalid {field}: {value:?}")]
    Field { field: &'static str, value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixQuality {
    NoFix,
    Autonomous,
    Differential,
    Pps,
    RtkFixed,
    RtkFloat,
    DeadReckoning,
    Manual,
    Simulation,
}

impl FixQuality {
    pub fn has_position(self) -> bool {
        self != FixQuality::NoFix
    }

    fn from_field(value: &str) -> Result<Self, NmeaError> {
        match value {
            "0" => Ok(FixQuality::NoFix),
            "1" => Ok(FixQuality::Autonomous),
            "2" => Ok(FixQuality::Differential),
            "3" => Ok(FixQuality::Pps),
            "4" => Ok(FixQuality::RtkFixed),
            "5" => Ok(FixQuality::RtkFloat),
            "6" => Ok(FixQuality::DeadReckoning),
            "7" => Ok(FixQuality::Manual),
            "8" => Ok(FixQuality::Simulation),
            other => Err(invalid("fix quality", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_msl_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gga {
    pub utc_seconds_of_day: Option<f64>,
    pub quality: FixQuality,
    pub satellites: u8,
    pub hdop: Option<f32>,
    pub position: Option<Position>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rmc {
    pub valid: bool,
    pub unix_nanos: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sentence {
    Gga(Gga),
    Rmc(Rmc),
    CommandReply { accepted: bool },
}

pub fn parse(line: &[u8]) -> Result<Option<Sentence>, NmeaError> {
    let text = std::str::from_utf8(line).map_err(|_| NmeaError::NotText)?;
    let body = text
        .trim_end_matches(['\r', '\n'])
        .strip_prefix('$')
        .ok_or(NmeaError::MissingStart)?;
    let (payload, checksum) = body.rsplit_once('*').ok_or(NmeaError::MissingChecksum)?;
    let expected = u8::from_str_radix(checksum, 16).map_err(|_| invalid("checksum", checksum))?;
    let reply = payload.starts_with(REPLY_PREFIX);
    let computed = match reply {
        true => checksum_of(payload) ^ b'$',
        false => checksum_of(payload),
    };
    if expected != computed {
        return Err(NmeaError::Checksum { expected, computed });
    }
    if reply {
        return Ok(Some(Sentence::CommandReply {
            accepted: payload.ends_with(REPLY_ACCEPTED),
        }));
    }
    let (kind, rest) = payload.split_once(',').unwrap_or((payload, ""));
    match (kind.len(), kind.get(2..)) {
        (5, Some("GGA")) => parse_gga(rest).map(|gga| Some(Sentence::Gga(gga))),
        (5, Some("RMC")) => parse_rmc(rest).map(|rmc| Some(Sentence::Rmc(rmc))),
        _ => Ok(None),
    }
}

pub fn checksum_of(payload: &str) -> u8 {
    payload.bytes().fold(0, |acc, byte| acc ^ byte)
}

fn parse_gga(rest: &str) -> Result<Gga, NmeaError> {
    let count = rest.split(',').count();
    if count != GGA_FIELDS {
        return Err(NmeaError::FieldCount(count));
    }
    let mut fields = [""; GGA_FIELDS];
    fields
        .iter_mut()
        .zip(rest.split(','))
        .for_each(|(slot, value)| *slot = value);
    let [time, lat, lat_hemi, lon, lon_hemi, quality, sats, hdop, alt, alt_unit, ..] = fields;

    let quality = FixQuality::from_field(quality)?;
    let latitude = coordinate(lat, lat_hemi, 2, 90.0, ('N', 'S'), "latitude")?;
    let longitude = coordinate(lon, lon_hemi, 3, 180.0, ('E', 'W'), "longitude")?;
    let position = match (quality.has_position(), latitude, longitude) {
        (false, _, _) => None,
        (true, Some(latitude), Some(longitude)) => Some(Position {
            latitude,
            longitude,
            altitude_msl_m: altitude(alt, alt_unit)?,
        }),
        (true, _, _) => return Err(invalid("position", lat)),
    };

    Ok(Gga {
        utc_seconds_of_day: time_of_day(time)?,
        quality,
        satellites: optional(sats, "satellites")?.unwrap_or(0),
        hdop: optional(hdop, "hdop")?,
        position,
    })
}

fn parse_rmc(rest: &str) -> Result<Rmc, NmeaError> {
    let count = rest.split(',').count();
    if count < RMC_MIN_FIELDS {
        return Err(NmeaError::RmcFieldCount(count));
    }
    let mut fields = rest.split(',');
    let time = fields.next().unwrap_or_default();
    let status = fields.next().unwrap_or_default();
    let date = fields.nth(6).unwrap_or_default();
    let valid = match status {
        "A" => true,
        "V" | "" => false,
        other => return Err(invalid("rmc status", other)),
    };
    let unix_nanos = match (time_of_day(time)?, day_number(date)?) {
        (Some(seconds), Some(days)) => Some(
            days * SECONDS_PER_DAY * 1_000_000_000 + (seconds * NANOS_PER_SECOND).round() as i64,
        ),
        _ => None,
    };
    Ok(Rmc { valid, unix_nanos })
}

fn day_number(value: &str) -> Result<Option<i64>, NmeaError> {
    if value.is_empty() {
        return Ok(None);
    }
    let part = |range: std::ops::Range<usize>| -> Result<i64, NmeaError> {
        value
            .get(range)
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| invalid("rmc date", value))
    };
    let (day, month, year) = (part(0..2)?, part(2..4)?, 2000 + part(4..6)?);
    if value.len() != 6 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(invalid("rmc date", value));
    }
    Ok(Some(days_from_civil(year, month, day)))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn invalid(field: &'static str, value: &str) -> NmeaError {
    NmeaError::Field {
        field,
        value: value.to_string(),
    }
}

fn optional<T: std::str::FromStr>(
    value: &str,
    field: &'static str,
) -> Result<Option<T>, NmeaError> {
    match value {
        "" => Ok(None),
        value => value.parse().map(Some).map_err(|_| invalid(field, value)),
    }
}

fn time_of_day(value: &str) -> Result<Option<f64>, NmeaError> {
    if value.is_empty() {
        return Ok(None);
    }
    let hours: u32 = value
        .get(0..2)
        .and_then(|h| h.parse().ok())
        .ok_or_else(|| invalid("utc time", value))?;
    let minutes: u32 = value
        .get(2..4)
        .and_then(|m| m.parse().ok())
        .ok_or_else(|| invalid("utc time", value))?;
    let seconds: f64 = value
        .get(4..)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("utc time", value))?;
    match (hours < 24, minutes < 60, (0.0..61.0).contains(&seconds)) {
        (true, true, true) => Ok(Some(f64::from(hours * 3600 + minutes * 60) + seconds)),
        _ => Err(invalid("utc time", value)),
    }
}

fn coordinate(
    value: &str,
    hemisphere: &str,
    degree_digits: usize,
    limit: f64,
    (positive, negative): (char, char),
    field: &'static str,
) -> Result<Option<f64>, NmeaError> {
    if value.is_empty() {
        return Ok(None);
    }
    let degrees: f64 = value
        .get(..degree_digits)
        .and_then(|d| d.parse().ok())
        .ok_or_else(|| invalid(field, value))?;
    let minutes: f64 = value
        .get(degree_digits..)
        .and_then(|m| m.parse().ok())
        .ok_or_else(|| invalid(field, value))?;
    if !(0.0..60.0).contains(&minutes) {
        return Err(invalid(field, value));
    }
    let magnitude = degrees + minutes / 60.0;
    if magnitude > limit {
        return Err(invalid(field, value));
    }
    match hemisphere.chars().next() {
        Some(h) if h == positive => Ok(Some(magnitude)),
        Some(h) if h == negative => Ok(Some(-magnitude)),
        _ => Err(invalid(field, hemisphere)),
    }
}

fn altitude(value: &str, unit: &str) -> Result<Option<f64>, NmeaError> {
    match (value, unit) {
        ("", _) => Ok(None),
        (value, "M") => optional(value, "altitude"),
        (_, unit) => Err(invalid("altitude unit", unit)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentence(payload: &str) -> String {
        let sum = checksum_of(payload);
        format!("${payload}*{sum:02X}\r\n")
    }

    fn gga(line: &str) -> Gga {
        match parse(line.as_bytes()) {
            Ok(Some(Sentence::Gga(gga))) => gga,
            other => panic!("expected GGA, got {other:?}"),
        }
    }

    fn rmc(line: &str) -> Rmc {
        match parse(line.as_bytes()) {
            Ok(Some(Sentence::Rmc(rmc))) => rmc,
            other => panic!("expected RMC, got {other:?}"),
        }
    }

    #[test]
    fn parses_rmc_time_and_date() {
        let line = sentence("GNRMC,211959.00,A,4530.0000,N,07330.0000,W,0.01,,101026,,,A,V");
        assert_eq!(
            rmc(&line),
            Rmc {
                valid: true,
                unix_nanos: Some(1_791_667_199_000_000_000),
            }
        );
    }

    #[test]
    fn parses_rmc_on_leap_day_with_fraction() {
        let line = sentence("GPRMC,000000.25,A,,,,,,,290228,,");
        assert_eq!(rmc(&line).unix_nanos, Some(1_835_395_200_250_000_000));
    }

    #[test]
    fn parses_rmc_without_fix() {
        assert_eq!(
            rmc("$GPRMC,,V,,,,,,,,,,N*53\r\n"),
            Rmc {
                valid: false,
                unix_nanos: None,
            }
        );
    }

    #[test]
    fn rejects_bad_rmc_date() {
        let line = sentence("GPRMC,120000,A,,,,,,,321326,,");
        assert!(matches!(
            parse(line.as_bytes()),
            Err(NmeaError::Field {
                field: "rmc date",
                ..
            })
        ));
    }

    #[test]
    fn parses_multi_constellation_fix() {
        let line = sentence("GNGGA,123519.50,4530.0000,N,07330.0000,W,1,14,1.7,64.45,M,-18.82,M,,");
        let fix = gga(&line);
        assert_eq!(fix.quality, FixQuality::Autonomous);
        assert_eq!(fix.satellites, 14);
        assert_eq!(fix.hdop, Some(1.7));
        assert_eq!(
            fix.utc_seconds_of_day,
            Some(12.0 * 3600.0 + 35.0 * 60.0 + 19.5)
        );
        let position = fix.position.expect("fix has a position");
        assert!((position.latitude - 45.5).abs() < 1e-12);
        assert!((position.longitude + 73.5).abs() < 1e-12);
        assert_eq!(position.altitude_msl_m, Some(64.45));
    }

    #[test]
    fn parses_southern_and_eastern_hemispheres() {
        let line = sentence("GPGGA,000000,3345.0000,S,15112.0000,E,4,20,0.6,10.0,M,,M,,");
        let position = gga(&line).position.expect("rtk fix has a position");
        assert!((position.latitude + 33.75).abs() < 1e-12);
        assert!((position.longitude - 151.2).abs() < 1e-12);
    }

    #[test]
    fn parses_receiver_without_fix() {
        let fix = gga("$GPGGA,,,,,,0,00,99.99,,,,,,*48\r\n");
        assert_eq!(fix.quality, FixQuality::NoFix);
        assert_eq!(fix.satellites, 0);
        assert_eq!(fix.hdop, Some(99.99));
        assert_eq!(fix.utc_seconds_of_day, None);
        assert_eq!(fix.position, None);
    }

    #[test]
    fn parses_receiver_command_replies() {
        assert_eq!(
            parse(b"$command,GPGGA 1,response: OK*19\r\n"),
            Ok(Some(Sentence::CommandReply { accepted: true }))
        );
        let rejected = "command,GPXXX 1,response: PARSING FAILD NO MATCHING FUNC  GPXXX";
        let sum = checksum_of(rejected) ^ b'$';
        assert_eq!(
            parse(format!("${rejected}*{sum:02X}").as_bytes()),
            Ok(Some(Sentence::CommandReply { accepted: false }))
        );
    }

    #[test]
    fn other_sentences_are_skipped() {
        let line = sentence("GPGSA,A,3,,,,,,,,,,,,,1.0,0.9,0.5");
        assert_eq!(parse(line.as_bytes()), Ok(None));
    }

    #[test]
    fn rejects_bad_checksum() {
        assert_eq!(
            parse(b"$GPGGA,,,,,,0,00,99.99,,,,,,*49"),
            Err(NmeaError::Checksum {
                expected: 0x49,
                computed: 0x48
            })
        );
    }

    #[test]
    fn rejects_missing_framing() {
        assert_eq!(
            parse(b"GPGGA,,,,,,0,00,99.99,,,,,,*48"),
            Err(NmeaError::MissingStart)
        );
        assert_eq!(
            parse(b"$GPGGA,,,,,,0,00,99.99,,,,,,"),
            Err(NmeaError::MissingChecksum)
        );
    }

    #[test]
    fn rejects_fix_without_coordinates() {
        let line = sentence("GPGGA,120000,,,,,1,05,2.0,,,,,,");
        assert!(matches!(
            parse(line.as_bytes()),
            Err(NmeaError::Field {
                field: "position",
                ..
            })
        ));
    }

    #[test]
    fn rejects_out_of_range_values() {
        let minutes = sentence("GPGGA,120000,4560.0000,N,07330.0000,W,1,05,2.0,,,,,,");
        assert!(matches!(
            parse(minutes.as_bytes()),
            Err(NmeaError::Field {
                field: "latitude",
                ..
            })
        ));
        let latitude = sentence("GPGGA,120000,9100.0000,N,07330.0000,W,1,05,2.0,,,,,,");
        assert!(matches!(
            parse(latitude.as_bytes()),
            Err(NmeaError::Field {
                field: "latitude",
                ..
            })
        ));
        let time = sentence("GPGGA,246000,4530.0000,N,07330.0000,W,1,05,2.0,,,,,,");
        assert!(matches!(
            parse(time.as_bytes()),
            Err(NmeaError::Field {
                field: "utc time",
                ..
            })
        ));
    }

    #[test]
    fn rejects_wrong_field_count() {
        let line = sentence("GPGGA,120000,4530.0000,N");
        assert_eq!(parse(line.as_bytes()), Err(NmeaError::FieldCount(3)));
    }
}
