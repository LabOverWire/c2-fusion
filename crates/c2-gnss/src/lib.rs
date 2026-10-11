pub mod nmea;
#[cfg(target_os = "linux")]
pub mod pps;
pub mod reader;
pub mod timing;

pub use nmea::{FixQuality, Gga, NmeaError, Position, Rmc, Sentence};
#[cfg(target_os = "linux")]
pub use pps::{discipline_clock, monotonic_nanos, step_clock, PpsLine};
pub use reader::{open_serial, request_fix_output, GnssError, NmeaReader, MAX_LINE_BYTES};
pub use timing::{clock_offset, PpsEdge, PAIRING_WINDOW};
