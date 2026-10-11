use futures_util::StreamExt;
use gpio_cdev::{AsyncLineEventHandle, Chip, EventRequestFlags, LineRequestFlags};
use nix::sys::time::TimeSpec;
use nix::time::{clock_gettime, clock_settime, ClockId};

use crate::reader::GnssError;
use crate::timing::PpsEdge;

const CONSUMER: &str = "c2-pps";
const NANOS_PER_SECOND: i64 = 1_000_000_000;
const PLL_TIME_CONSTANT: i64 = 0;

pub struct PpsLine {
    events: AsyncLineEventHandle,
}

impl PpsLine {
    pub fn open(chip: &str, line: u32) -> Result<Self, GnssError> {
        let events = Chip::new(chip)?.get_line(line)?.async_events(
            LineRequestFlags::INPUT,
            EventRequestFlags::RISING_EDGE,
            CONSUMER,
        )?;
        Ok(Self { events })
    }

    pub async fn next_edge(&mut self) -> Result<PpsEdge, GnssError> {
        let event = self.events.next().await.ok_or(GnssError::PpsClosed)??;
        let monotonic_now = read_nanos(ClockId::CLOCK_MONOTONIC)?;
        let system_now = read_nanos(ClockId::CLOCK_REALTIME)?;
        let edge_monotonic = i64::try_from(event.timestamp()).map_err(|_| out_of_range())?;
        Ok(PpsEdge {
            monotonic_nanos: event.timestamp(),
            system_nanos: system_now - (monotonic_now - edge_monotonic),
        })
    }
}

pub fn monotonic_nanos() -> Result<u64, GnssError> {
    let nanos = read_nanos(ClockId::CLOCK_MONOTONIC)?;
    u64::try_from(nanos).map_err(|_| out_of_range())
}

fn out_of_range() -> GnssError {
    GnssError::Clock(nix::errno::Errno::ERANGE)
}

pub fn step_clock(offset_nanos: i64) -> Result<(), GnssError> {
    let target = read_nanos(ClockId::CLOCK_REALTIME)? + offset_nanos;
    let time = TimeSpec::new(
        target.div_euclid(NANOS_PER_SECOND),
        target.rem_euclid(NANOS_PER_SECOND),
    );
    clock_settime(ClockId::CLOCK_REALTIME, time)?;
    Ok(())
}

pub fn discipline_clock(offset_nanos: i64) -> Result<(), GnssError> {
    let mut adjustment = libc::timex {
        modes: libc::ADJ_OFFSET | libc::ADJ_STATUS | libc::ADJ_NANO | libc::ADJ_TIMECONST,
        offset: offset_nanos,
        status: libc::STA_PLL,
        constant: PLL_TIME_CONSTANT,
        ..unsafe { std::mem::zeroed() }
    };
    match unsafe { libc::adjtimex(&mut adjustment) } {
        -1 => Err(GnssError::Clock(nix::errno::Errno::last())),
        _ => Ok(()),
    }
}

fn read_nanos(clock: ClockId) -> Result<i64, GnssError> {
    let time = clock_gettime(clock)?;
    Ok(time.tv_sec() * NANOS_PER_SECOND + time.tv_nsec())
}
