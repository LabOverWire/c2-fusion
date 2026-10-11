use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use c2_codec::compact::CompactCodec;
use c2_codec::Codec;
use c2_exchange::ingest;
use c2_lora::{
    open_serial, Config, Due, Frame, Inbound, Link, LoraError, Outgoing, Radio, ResetLine,
};
use c2_model::C2Message;
use stitch_p2p::Store;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, sleep_until, Instant, MissedTickBehavior};
use tokio_serial::SerialStream;

pub const OUTBOX_DEPTH: usize = 32;

const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_BACKOFF: Duration = Duration::from_secs(5);

pub struct LoraSettings {
    pub port: String,
    pub reset_gpio: u32,
    pub config: Config,
}

pub async fn run(
    settings: LoraSettings,
    store: Arc<Store>,
    name: String,
    mut outbox: mpsc::Receiver<C2Message>,
) {
    let mut link = Link::new(seed(settings.config.address));
    loop {
        let Err(error) = session(&settings, &mut link, &store, &name, &mut outbox).await;
        eprintln!("LORA name={name} error={error} recovering");
        sleep(RECOVERY_BACKOFF).await;
    }
}

fn seed(address: u16) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    (nanos as u64) ^ (u64::from(address) << 48)
}

async fn session(
    settings: &LoraSettings,
    link: &mut Link,
    store: &Store,
    name: &str,
    outbox: &mut mpsc::Receiver<C2Message>,
) -> Result<Infallible, LoraError> {
    let reset = ResetLine::open(settings.reset_gpio).await?;
    let mut radio = Radio::new(open_serial(&settings.port)?, Some(reset));
    let ready = radio.reset().await?;
    let changed = radio.configure(&settings.config).await?;
    println!(
        "LORA name={name} ready address={} spreading_factor={} reset_ms={} changed={changed}",
        settings.config.address,
        settings.config.modulation.spreading_factor,
        ready.as_millis()
    );

    let mut health = interval(HEALTH_INTERVAL);
    health.set_missed_tick_behavior(MissedTickBehavior::Delay);
    health.tick().await;

    loop {
        let due = link.next_due().map(Instant::from_std);
        tokio::select! {
            biased;
            frame = radio.recv() => receive(&mut radio, link, store, name, frame?).await?,
            _ = wait_until(due) => retry(&mut radio, link, name).await?,
            Some(msg) = outbox.recv(), if link.has_capacity() => {
                originate(&mut radio, link, name, &msg).await?;
            }
            _ = health.tick() => {
                let round_trip = radio.refresh().await?;
                println!(
                    "LORA name={name} health ping_ms={} dropped_frames={}",
                    round_trip.as_millis(),
                    radio.dropped_frames()
                );
            }
        }
    }
}

async fn wait_until(due: Option<Instant>) {
    match due {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn transmit(
    radio: &mut Radio<SerialStream>,
    outgoing: &Outgoing,
) -> Result<Duration, LoraError> {
    sleep_until(Instant::from_std(outgoing.not_before)).await;
    radio.send(outgoing.destination, &outgoing.bytes).await
}

async fn originate(
    radio: &mut Radio<SerialStream>,
    link: &mut Link,
    name: &str,
    msg: &C2Message,
) -> Result<(), LoraError> {
    let id = msg.id();
    let payload = match CompactCodec.encode(msg) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("LORA name={name} skip id={id} error={error}");
            return Ok(());
        }
    };
    let outgoing = match link.originate(&payload, std::time::Instant::now()) {
        Ok(outgoing) => outgoing,
        Err(error) => {
            eprintln!("LORA name={name} skip id={id} error={error}");
            return Ok(());
        }
    };
    let airtime = transmit(radio, &outgoing).await?;
    println!(
        "LORA name={name} tx id={id} seq={} len={} send_ms={}",
        outgoing.sequence,
        outgoing.bytes.len(),
        airtime.as_millis()
    );
    Ok(())
}

async fn receive(
    radio: &mut Radio<SerialStream>,
    link: &mut Link,
    store: &Store,
    name: &str,
    frame: Frame,
) -> Result<(), LoraError> {
    let source = frame.source;
    let rssi = frame.rssi_dbm;
    match link.receive(source, &frame.payload, std::time::Instant::now()) {
        Inbound::Deliver {
            sequence,
            payload,
            ack,
        } => {
            deliver(store, name, source, sequence, rssi, frame.snr, payload).await;
            transmit(radio, &ack).await?;
        }
        Inbound::Duplicate { sequence, ack } => {
            println!("LORA name={name} duplicate from={source} seq={sequence}");
            transmit(radio, &ack).await?;
        }
        Inbound::Acked {
            sequence,
            remaining,
        } => println!("LORA name={name} ack from={source} seq={sequence} remaining={remaining}"),
        Inbound::UnknownAck { sequence } => {
            println!("LORA name={name} late-ack from={source} seq={sequence}")
        }
        Inbound::Invalid(error) => {
            eprintln!("LORA name={name} invalid-frame from={source} rssi_dbm={rssi} error={error}")
        }
    }
    Ok(())
}

async fn deliver(
    store: &Store,
    name: &str,
    source: u16,
    sequence: u16,
    rssi: i16,
    snr: i16,
    payload: &[u8],
) {
    let msg = match CompactCodec.decode(payload) {
        Ok(msg) => msg,
        Err(error) => {
            eprintln!(
                "LORA name={name} rx-undecodable from={source} seq={sequence} rssi_dbm={rssi} error={error}"
            );
            return;
        }
    };
    match ingest(store, &msg).await {
        Ok(()) => println!(
            "LORA name={name} rx id={} from={source} seq={sequence} len={} rssi_dbm={rssi} snr={snr}",
            msg.id(),
            payload.len()
        ),
        Err(error) => eprintln!("LORA name={name} rx id={} ingest-error={error}", msg.id()),
    }
}

async fn retry(
    radio: &mut Radio<SerialStream>,
    link: &mut Link,
    name: &str,
) -> Result<(), LoraError> {
    match link.poll(std::time::Instant::now()) {
        Some(Due::Retry {
            sequence,
            attempt,
            outgoing,
        }) => {
            let airtime = transmit(radio, &outgoing).await?;
            println!(
                "LORA name={name} retry seq={sequence} attempt={attempt} send_ms={}",
                airtime.as_millis()
            );
        }
        Some(Due::GaveUp { sequence, missing }) => {
            eprintln!("LORA name={name} gave-up seq={sequence} missing={missing:?}");
        }
        None => {}
    }
    Ok(())
}
