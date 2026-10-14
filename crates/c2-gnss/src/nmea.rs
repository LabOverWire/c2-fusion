const GGA_FIELDS: usize = 14;
const RMC_MIN_FIELDS: usize = 11;
const NANOS_PER_SECOND: f64 = 1e9;
const SECONDS_PER_DAY: i64 = 86_400;
const LEAP_SECOND: f64 = 60.0;
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
    let expected = match checksum.len() == 2 && checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        true => u8::from_str_radix(checksum, 16).map_err(|_| invalid("checksum", checksum))?,
        false => return Err(invalid("checksum", checksum)),
    };
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
        satellites: match sats {
            "" => 0,
            value => digits(value)
                .and_then(|count| u8::try_from(count).ok())
                .ok_or_else(|| invalid("satellites", value))?,
        },
        hdop: match hdop {
            "" => None,
            value => Some(decimal(value).ok_or_else(|| invalid("hdop", value))? as f32),
        },
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
    let unix_nanos = match (clock(time)?, day_number(date)?) {
        (Some(clock), _) if clock.second >= LEAP_SECOND => None,
        (Some(clock), Some(days)) => Some(
            days * SECONDS_PER_DAY * 1_000_000_000
                + (clock.seconds_of_day() * NANOS_PER_SECOND).round() as i64,
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
            .and_then(digits)
            .map(i64::from)
            .ok_or_else(|| invalid("rmc date", value))
    };
    let (day, month, year) = (part(0..2)?, part(2..4)?, 2000 + part(4..6)?);
    if value.len() != 6
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
    {
        return Err(invalid("rmc date", value));
    }
    Ok(Some(days_from_civil(year, month, day)))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
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

fn digits(value: &str) -> Option<u32> {
    match !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        true => value.parse().ok(),
        false => None,
    }
}

fn decimal(value: &str) -> Option<f64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, "0"));
    let plain = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    match plain(whole) && plain(fraction) {
        true => value.parse().ok(),
        false => None,
    }
}

fn signed_decimal(value: &str) -> Option<f64> {
    match value.strip_prefix('-') {
        Some(magnitude) => decimal(magnitude).map(|m| -m),
        None => decimal(value),
    }
}

struct Clock {
    hour: u32,
    minute: u32,
    second: f64,
}

impl Clock {
    fn seconds_of_day(&self) -> f64 {
        f64::from(self.hour * 3600 + self.minute * 60) + self.second
    }
}

fn time_of_day(value: &str) -> Result<Option<f64>, NmeaError> {
    Ok(clock(value)?.map(|clock| clock.seconds_of_day()))
}

fn clock(value: &str) -> Result<Option<Clock>, NmeaError> {
    if value.is_empty() {
        return Ok(None);
    }
    let hour = value
        .get(0..2)
        .and_then(digits)
        .ok_or_else(|| invalid("utc time", value))?;
    let minute = value
        .get(2..4)
        .and_then(digits)
        .ok_or_else(|| invalid("utc time", value))?;
    let second = value
        .get(4..)
        .filter(|s| s.split('.').next().is_some_and(|whole| whole.len() == 2))
        .and_then(decimal)
        .ok_or_else(|| invalid("utc time", value))?;
    match (hour < 24, minute < 60, (0.0..61.0).contains(&second)) {
        (true, true, true) => Ok(Some(Clock {
            hour,
            minute,
            second,
        })),
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
    let degrees = value
        .get(..degree_digits)
        .and_then(digits)
        .map(f64::from)
        .ok_or_else(|| invalid(field, value))?;
    let minutes = value
        .get(degree_digits..)
        .and_then(decimal)
        .ok_or_else(|| invalid(field, value))?;
    if !(0.0..60.0).contains(&minutes) {
        return Err(invalid(field, value));
    }
    let magnitude = degrees + minutes / 60.0;
    if magnitude > limit {
        return Err(invalid(field, value));
    }
    let mut letters = hemisphere.chars();
    match (letters.next(), letters.next()) {
        (Some(h), None) if h == positive => Ok(Some(magnitude)),
        (Some(h), None) if h == negative => Ok(Some(-magnitude)),
        _ => Err(invalid(field, hemisphere)),
    }
}

fn altitude(value: &str, unit: &str) -> Result<Option<f64>, NmeaError> {
    match (value, unit) {
        ("", _) => Ok(None),
        (value, "M") => signed_decimal(value)
            .map(Some)
            .ok_or_else(|| invalid("altitude", value)),
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

    const FIX: &str = "GNGGA,123519.50,4530.0000,N,07330.0000,W,1,14,1.7,64.45,M,-18.82,M,,";

    fn fix_with(from: &str, to: &str) -> Result<Option<Sentence>, NmeaError> {
        assert!(FIX.contains(from), "{from}");
        parse(sentence(&FIX.replacen(from, to, 1)).as_bytes())
    }

    fn refused_field(result: Result<Option<Sentence>, NmeaError>) -> &'static str {
        match result {
            Err(NmeaError::Field { field, .. }) => field,
            other => panic!("expected a field error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_numeric_and_signed_coordinates() {
        assert_eq!(
            refused_field(fix_with("07330.0000", "nan30.0000")),
            "longitude"
        );
        assert_eq!(
            refused_field(fix_with("07330.0000", "07330.0e00")),
            "longitude"
        );
        assert_eq!(
            refused_field(fix_with("4530.0000,N", "-930.0000,N")),
            "latitude"
        );
        assert_eq!(
            refused_field(fix_with("4530.0000,N", "+930.0000,N")),
            "latitude"
        );
        assert_eq!(
            refused_field(fix_with("4530.0000,N", "45-1.0000,N")),
            "latitude"
        );
    }

    #[test]
    fn rejects_non_finite_altitude_and_hdop() {
        assert_eq!(refused_field(fix_with(",64.45,M", ",NaN,M")), "altitude");
        assert_eq!(refused_field(fix_with(",64.45,M", ",inf,M")), "altitude");
        assert_eq!(refused_field(fix_with(",1.7,", ",NaN,")), "hdop");
        assert_eq!(refused_field(fix_with(",1.7,", ",inf,")), "hdop");
        assert_eq!(refused_field(fix_with(",1.7,", ",-1.7,")), "hdop");
    }

    #[test]
    fn keeps_a_negative_altitude() {
        let fix = gga(&sentence(&FIX.replacen(",64.45,M", ",-12.5,M", 1)));
        assert_eq!(fix.position.expect("position").altitude_msl_m, Some(-12.5));
    }

    #[test]
    fn hemisphere_time_satellites_and_checksum_must_be_exact() {
        assert_eq!(
            refused_field(fix_with("07330.0000,W", "07330.0000,Wx")),
            "longitude"
        );
        assert_eq!(refused_field(fix_with("123519.50", "+10000")), "utc time");
        assert_eq!(refused_field(fix_with("123519.50", "12351.50")), "utc time");
        assert_eq!(refused_field(fix_with(",14,", ",+4,")), "satellites");
        assert_eq!(refused_field(fix_with(",14,", ",300,")), "satellites");
        for checksum in ["+48", "4", "048", "4G"] {
            let line = format!("$GPGGA,,,,,,0,00,99.99,,,,,,*{checksum}\r\n");
            assert_eq!(
                refused_field(parse(line.as_bytes())),
                "checksum",
                "{checksum}"
            );
        }
    }

    #[test]
    fn rejects_days_past_the_end_of_the_month() {
        for date in ["310226", "300226", "290226", "310426", "310926"] {
            let line = sentence(&format!("GPRMC,120000,A,,,,,,,{date},,"));
            assert_eq!(refused_field(parse(line.as_bytes())), "rmc date", "{date}");
        }
        let leap_day = rmc(&sentence("GPRMC,120000,A,,,,,,,290228,,"));
        assert!(leap_day.unix_nanos.is_some());
    }

    #[test]
    fn leap_second_has_no_unix_time() {
        let leap = rmc(&sentence("GPRMC,235960,A,,,,,,,311226,,"));
        assert!(leap.valid);
        assert_eq!(leap.unix_nanos, None);
        let midnight = rmc(&sentence("GPRMC,000000,A,,,,,,,010127,,"));
        assert!(midnight.unix_nanos.is_some());
    }
}
