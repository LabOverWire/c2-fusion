use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use c2_exchange::radio::RadioMessage;
use c2_lora::{
    open_serial, Config, Due, Frame, Inbound, Link, LoraError, Outgoing, Radio, ResetLine,
};
use stitch_p2p::{Cursors, PeerId, RecordOutcome, SyncNode, WriteFrame};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, sleep_until, Instant, MissedTickBehavior};
use tokio_serial::SerialStream;

const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const HELLO_INTERVAL: Duration = Duration::from_secs(30);
const GAP_HELLO_SPACING: Duration = Duration::from_secs(5);
const RECOVERY_BACKOFF: Duration = Duration::from_secs(5);
const QUEUE_LIMIT: usize = 32;

pub struct LoraSettings {
    pub port: String,
    pub reset_gpio: u32,
    pub config: Config,
}

pub async fn run(settings: LoraSettings, node: SyncNode, name: String) {
    let mut link = Link::new(seed(settings.config.address));
    let mut writes = node.register_session();
    loop {
        let Err(error) = session(&settings, &mut link, &node, &name, &mut writes).await;
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

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn short(peer: &PeerId) -> String {
    peer[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Outbound {
    queue: VecDeque<WriteFrame>,
    hello_due: bool,
    last_hello: Option<Instant>,
}

impl Outbound {
    fn request_hello(&mut self) {
        self.hello_due = true;
    }

    fn request_hello_after_gap(&mut self) {
        let spaced = self
            .last_hello
            .is_none_or(|sent| sent.elapsed() >= GAP_HELLO_SPACING);
        self.hello_due |= spaced;
    }
}

async fn session(
    settings: &LoraSettings,
    link: &mut Link,
    node: &SyncNode,
    name: &str,
    writes: &mut mpsc::UnboundedReceiver<WriteFrame>,
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
    let mut hello = interval(HELLO_INTERVAL);
    hello.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut outbound = Outbound {
        queue: VecDeque::new(),
        hello_due: false,
        last_hello: None,
    };

    loop {
        let due = link.next_due().map(Instant::from_std);
        let ready_to_send =
            link.has_capacity() && (outbound.hello_due || !outbound.queue.is_empty());
        tokio::select! {
            biased;
            frame = radio.recv() => {
                receive(&mut radio, link, node, name, &mut outbound, writes, frame?).await?;
            }
            _ = wait_until(due) => retry(&mut radio, link, name).await?,
            _ = std::future::ready(()), if ready_to_send => {
                send_next(&mut radio, link, node, name, &mut outbound).await?;
            }
            Some(frame) = writes.recv(), if outbound.queue.len() < QUEUE_LIMIT => {
                outbound.queue.push_back(frame);
            }
            _ = hello.tick() => outbound.request_hello(),
            _ = health.tick() => {
                let round_trip = radio.refresh().await?;
                println!(
                    "LORA name={name} health ping_ms={} dropped_frames={} queued={}",
                    round_trip.as_millis(),
                    radio.dropped_frames(),
                    outbound.queue.len()
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

async fn send_next(
    radio: &mut Radio<SerialStream>,
    link: &mut Link,
    node: &SyncNode,
    name: &str,
    outbound: &mut Outbound,
) -> Result<(), LoraError> {
    let (message, label) = match outbound.hello_due {
        true => {
            outbound.hello_due = false;
            outbound.last_hello = Some(Instant::now());
            let state = node.state();
            let guard = state.lock().await;
            let cursors = guard.cursors();
            let label = format!("hello cursors={}", cursors.len());
            (
                RadioMessage::Hello {
                    from: guard.self_id(),
                    cursors,
                },
                label,
            )
        }
        false => match outbound.queue.pop_front() {
            Some(frame) => {
                let label = format!(
                    "write id={} origin={} origin_seq={}",
                    frame.id,
                    short(&frame.stamp.peer),
                    frame.seq
                );
                (RadioMessage::Write(frame), label)
            }
            None => return Ok(()),
        },
    };
    let payload = match message.encode() {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("LORA name={name} skip {label} error={error}");
            return Ok(());
        }
    };
    let outgoing = match link.originate(&payload, std::time::Instant::now()) {
        Ok(outgoing) => outgoing,
        Err(error) => {
            eprintln!(
                "LORA name={name} skip {label} len={} error={error}",
                payload.len()
            );
            return Ok(());
        }
    };
    let airtime = transmit(radio, &outgoing).await?;
    println!(
        "LORA name={name} tx {label} seq={} len={} send_ms={}",
        outgoing.sequence,
        outgoing.bytes.len(),
        airtime.as_millis()
    );
    Ok(())
}

async fn receive(
    radio: &mut Radio<SerialStream>,
    link: &mut Link,
    node: &SyncNode,
    name: &str,
    outbound: &mut Outbound,
    writes: &mut mpsc::UnboundedReceiver<WriteFrame>,
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
            transmit(radio, &ack).await?;
            match RadioMessage::decode(payload) {
                Ok(RadioMessage::Write(write)) => {
                    apply(node, name, outbound, source, rssi, frame.snr, write).await;
                }
                Ok(RadioMessage::Hello { from, cursors }) => {
                    answer_hello(node, name, outbound, writes, from, cursors).await;
                }
                Err(error) => eprintln!(
                    "LORA name={name} rx-undecodable from={source} seq={sequence} rssi_dbm={rssi} error={error}"
                ),
            }
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

async fn apply(
    node: &SyncNode,
    name: &str,
    outbound: &mut Outbound,
    source: u16,
    rssi: i16,
    snr: i16,
    write: WriteFrame,
) {
    let id = write.id.clone();
    let origin = short(&write.stamp.peer);
    let origin_seq = write.seq;
    let outcome = node.state().lock().await.receive(write, now_millis());
    if let RecordOutcome::Gap { .. } = outcome {
        outbound.request_hello_after_gap();
    }
    println!(
        "LORA name={name} rx write id={id} origin={origin} origin_seq={origin_seq} outcome={outcome:?} from={source} rssi_dbm={rssi} snr={snr}"
    );
}

async fn answer_hello(
    node: &SyncNode,
    name: &str,
    outbound: &mut Outbound,
    writes: &mut mpsc::UnboundedReceiver<WriteFrame>,
    from: PeerId,
    cursors: Cursors,
) {
    while writes.try_recv().is_ok() {}
    let delta = {
        let state = node.state();
        let mut guard = state.lock().await;
        guard.note_peer_cursors(from, cursors.clone());
        guard.delta_since(&cursors)
    };
    let missing = delta.len();
    outbound.queue = delta.into_iter().take(QUEUE_LIMIT).collect();
    println!(
        "LORA name={name} rx hello from={} missing={missing} queued={}",
        short(&from),
        outbound.queue.len()
    );
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
