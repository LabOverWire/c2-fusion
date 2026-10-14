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
    let current = adjtimex(libc::timex {
        modes: 0,
        ..unsafe { std::mem::zeroed() }
    })?;
    adjtimex(libc::timex {
        modes: libc::ADJ_OFFSET | libc::ADJ_STATUS | libc::ADJ_NANO | libc::ADJ_TIMECONST,
        offset: offset_nanos,
        status: pll_status(current.status),
        constant: PLL_TIME_CONSTANT,
        ..unsafe { std::mem::zeroed() }
    })?;
    Ok(())
}

fn pll_status(current: libc::c_int) -> libc::c_int {
    (current & !libc::STA_UNSYNC & !libc::STA_RONLY) | libc::STA_PLL
}

fn adjtimex(mut request: libc::timex) -> Result<libc::timex, GnssError> {
    match unsafe { libc::adjtimex(&mut request) } {
        -1 => Err(GnssError::Clock(nix::errno::Errno::last())),
        _ => Ok(request),
    }
}

fn read_nanos(clock: ClockId) -> Result<i64, GnssError> {
    let time = clock_gettime(clock)?;
    Ok(time.tv_sec() * NANOS_PER_SECOND + time.tv_nsec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pll_status_keeps_a_pending_leap_second_and_clears_unsync() {
        let status = pll_status(libc::STA_UNSYNC | libc::STA_INS | libc::STA_NANO);
        assert_eq!(status & libc::STA_PLL, libc::STA_PLL);
        assert_eq!(status & libc::STA_INS, libc::STA_INS);
        assert_eq!(status & libc::STA_UNSYNC, 0);
        assert_eq!(status & libc::STA_RONLY, 0);
    }
}
