use std::convert::Infallible;
use std::time::Duration;

use c2_gnss::{
    clock_offset, discipline_clock, monotonic_nanos, open_serial, request_fix_output, step_clock,
    Gga, GnssError, NmeaReader, PpsEdge, PpsLine, Rmc, Sentence,
};
use tokio::time::{interval, sleep, sleep_until, Instant, MissedTickBehavior};

const REPORT_INTERVAL: Duration = Duration::from_secs(60);
const RECOVERY_BACKOFF: Duration = Duration::from_secs(5);
const SILENCE_LIMIT: Duration = Duration::from_secs(5);
const STEP_THRESHOLD_NANOS: i64 = 500_000_000;

pub struct GnssSettings {
    pub port: String,
    pub baud_rate: u32,
    pub pps_chip: String,
    pub pps_line: u32,
}

pub async fn run(settings: GnssSettings, name: String) {
    loop {
        let Err(error) = session(&settings, &name).await;
        eprintln!("GNSS name={name} error={error} recovering");
        sleep(RECOVERY_BACKOFF).await;
    }
}

#[derive(Default)]
struct Status {
    fix: Option<Gga>,
    offset_nanos: Option<i64>,
    worst_nanos: i64,
    edges: u32,
    paired: u32,
}

async fn session(settings: &GnssSettings, name: &str) -> Result<Infallible, GnssError> {
    let mut port = open_serial(&settings.port, settings.baud_rate)?;
    request_fix_output(&mut port).await?;
    let mut reader = NmeaReader::new(port);
    let mut pps = PpsLine::open(&settings.pps_chip, settings.pps_line)?;
    println!(
        "GNSS name={name} ready port={} pps={}:{}",
        settings.port, settings.pps_chip, settings.pps_line
    );

    let mut report = interval(REPORT_INTERVAL);
    report.set_missed_tick_behavior(MissedTickBehavior::Delay);
    report.tick().await;
    let mut last_edge: Option<PpsEdge> = None;
    let mut status = Status::default();
    let mut heard = Instant::now();

    loop {
        tokio::select! {
            edge = pps.next_edge() => {
                last_edge = Some(edge?);
                status.edges += 1;
            }
            sentence = reader.next_sentence() => match sentence.ok_or(GnssError::Closed)? {
                Ok(Sentence::Gga(gga)) => {
                    heard = Instant::now();
                    status.fix = Some(gga);
                }
                Ok(Sentence::Rmc(rmc)) => {
                    heard = Instant::now();
                    if let Some(offset) = paired_offset(last_edge, rmc)? {
                        status.paired += 1;
                        status.offset_nanos = Some(offset);
                        status.worst_nanos = status.worst_nanos.max(offset.abs());
                        if offset.abs() >= STEP_THRESHOLD_NANOS {
                            step_clock(offset)?;
                            println!("GNSS name={name} stepped-clock offset_ms={}", offset / 1_000_000);
                            last_edge = None;
                        } else {
                            discipline_clock(offset)?;
                        }
                    }
                }
                Ok(Sentence::CommandReply { accepted: true }) => {}
                Ok(Sentence::CommandReply { accepted: false }) => {
                    eprintln!("GNSS name={name} receiver rejected an output command");
                }
                Err(error) => eprintln!("GNSS name={name} skip error={error}"),
            },
            _ = sleep_until(heard + SILENCE_LIMIT) => return Err(GnssError::Silent(SILENCE_LIMIT)),
            _ = report.tick() => {
                log_status(name, &status);
                status = Status { fix: status.fix, ..Status::default() };
            }
        }
    }
}

fn paired_offset(edge: Option<PpsEdge>, rmc: Rmc) -> Result<Option<i64>, GnssError> {
    match edge {
        Some(edge) => Ok(clock_offset(edge, rmc, monotonic_nanos()?)),
        None => Ok(None),
    }
}

fn log_status(name: &str, status: &Status) {
    let Status {
        fix,
        offset_nanos,
        worst_nanos,
        edges,
        paired,
    } = status;
    let offset_us = offset_nanos.map(|nanos| nanos / 1_000);
    let worst_us = worst_nanos / 1_000;
    match fix {
        Some(Gga {
            quality,
            satellites,
            hdop,
            ..
        }) => println!(
            "GNSS name={name} fix={quality:?} sats={satellites} hdop={hdop:?} pps_edges={edges} paired={paired} offset_us={offset_us:?} worst_us={worst_us}"
        ),
        None => println!(
            "GNSS name={name} fix=none pps_edges={edges} paired={paired} offset_us={offset_us:?} worst_us={worst_us}"
        ),
    }
}
