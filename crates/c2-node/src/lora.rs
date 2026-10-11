use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use c2_exchange::radio::RadioMessage;
use c2_lora::{
    open_serial, Config, Due, Frame, Inbound, Link, LoraError, Outgoing, Radio, ResetLine,
    MAX_IN_FLIGHT,
};
use stitch_p2p::{Cursors, PeerId, RecordOutcome, SyncNode, WriteFrame};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, sleep_until, Instant, MissedTickBehavior};
use tokio_serial::SerialStream;

const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const HELLO_INTERVAL: Duration = Duration::from_secs(30);
const GAP_HELLO_SPACING: Duration = Duration::from_secs(15);
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

struct Exchange {
    queue: VecDeque<WriteFrame>,
    held: BTreeMap<(PeerId, u64), WriteFrame>,
    hello_due: bool,
    last_hello: Option<Instant>,
}

impl Exchange {
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
    let mut exchange = Exchange {
        queue: VecDeque::new(),
        held: BTreeMap::new(),
        hello_due: false,
        last_hello: None,
    };

    loop {
        let due = link.next_due().map(Instant::from_std);
        let ready_to_send =
            exchange.hello_due || (link.has_capacity() && !exchange.queue.is_empty());
        tokio::select! {
            biased;
            frame = radio.recv() => {
                receive(&mut radio, link, node, name, &mut exchange, writes, frame?).await?;
            }
            _ = wait_until(due) => retry(&mut radio, link, name).await?,
            _ = std::future::ready(()), if ready_to_send => {
                send_next(&mut radio, link, node, name, &mut exchange).await?;
            }
            Some(frame) = writes.recv(), if exchange.queue.len() < QUEUE_LIMIT => {
                exchange.queue.push_back(frame);
            }
            _ = hello.tick() => exchange.request_hello(),
            _ = health.tick() => {
                let round_trip = radio.refresh().await?;
                println!(
                    "LORA name={name} health ping_ms={} dropped_frames={} queued={}",
                    round_trip.as_millis(),
                    radio.dropped_frames(),
                    exchange.queue.len()
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
    exchange: &mut Exchange,
) -> Result<(), LoraError> {
    let sending_hello = exchange.hello_due;
    let (message, label) = match sending_hello {
        true => {
            exchange.hello_due = false;
            exchange.last_hello = Some(Instant::now());
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
        false => match exchange.queue.pop_front() {
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
    let now = std::time::Instant::now();
    let sent = match sending_hello {
        true => link.datagram(&payload, now),
        false => link.originate(&payload, now),
    };
    let outgoing = match sent {
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
    exchange: &mut Exchange,
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
            dispatch(node, name, exchange, writes, &frame, sequence, payload).await;
        }
        Inbound::Datagram { sequence, payload } => {
            dispatch(node, name, exchange, writes, &frame, sequence, payload).await;
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

async fn dispatch(
    node: &SyncNode,
    name: &str,
    exchange: &mut Exchange,
    writes: &mut mpsc::UnboundedReceiver<WriteFrame>,
    frame: &Frame,
    sequence: u16,
    payload: &[u8],
) {
    let source = frame.source;
    let rssi = frame.rssi_dbm;
    match RadioMessage::decode(payload) {
        Ok(RadioMessage::Write(write)) => {
            apply(node, name, exchange, source, rssi, frame.snr, write).await;
        }
        Ok(RadioMessage::Hello { from, cursors }) => {
            answer_hello(node, name, exchange, writes, from, cursors).await;
        }
        Err(error) => eprintln!(
            "LORA name={name} rx-undecodable from={source} seq={sequence} rssi_dbm={rssi} error={error}"
        ),
    }
}

async fn apply(
    node: &SyncNode,
    name: &str,
    exchange: &mut Exchange,
    source: u16,
    rssi: i16,
    snr: i16,
    write: WriteFrame,
) {
    let id = write.id.clone();
    let origin = write.stamp.peer;
    let origin_seq = write.seq;
    let state = node.state();
    let mut guard = state.lock().await;
    let outcome = guard.receive(write.clone(), now_millis());
    let mut released = 0;
    match outcome {
        RecordOutcome::Gap { .. } => {
            if exchange.held.len() < QUEUE_LIMIT {
                exchange.held.insert((origin, origin_seq), write);
            }
            exchange.request_hello_after_gap();
        }
        RecordOutcome::Appended => {
            let mut next = origin_seq + 1;
            while let Some(held) = exchange.held.remove(&(origin, next)) {
                if guard.receive(held, now_millis()) != RecordOutcome::Appended {
                    break;
                }
                released += 1;
                next += 1;
            }
            exchange
                .held
                .retain(|(held_origin, seq), _| *held_origin != origin || *seq > next);
        }
        _ => {}
    }
    println!(
        "LORA name={name} rx write id={id} origin={} origin_seq={origin_seq} outcome={outcome:?} released={released} held={} from={source} rssi_dbm={rssi} snr={snr}",
        short(&origin),
        exchange.held.len()
    );
}

async fn answer_hello(
    node: &SyncNode,
    name: &str,
    exchange: &mut Exchange,
    writes: &mut mpsc::UnboundedReceiver<WriteFrame>,
    from: PeerId,
    cursors: Cursors,
) {
    let state = node.state();
    let mut guard = state.lock().await;
    guard.note_peer_cursors(from, cursors.clone());
    if !queue_is_stale(&exchange.queue, &cursors) && !exchange.queue.is_empty() {
        println!(
            "LORA name={name} rx hello from={} deferred queued={}",
            short(&from),
            exchange.queue.len()
        );
        return;
    }
    while writes.try_recv().is_ok() {}
    let delta = guard.delta_since(&cursors);
    let missing = delta.len();
    exchange.queue = delta.into_iter().take(QUEUE_LIMIT).collect();
    println!(
        "LORA name={name} rx hello from={} missing={missing} queued={}",
        short(&from),
        exchange.queue.len()
    );
}

fn queue_is_stale(queue: &VecDeque<WriteFrame>, cursors: &Cursors) -> bool {
    let mut origins: Vec<PeerId> = Vec::new();
    for frame in queue {
        if origins.contains(&frame.stamp.peer) {
            continue;
        }
        origins.push(frame.stamp.peer);
        let delivered = cursors.get(&frame.stamp.peer).copied().unwrap_or(0);
        let in_window = delivered < frame.seq && frame.seq <= delivered + 1 + MAX_IN_FLIGHT as u64;
        if !in_window {
            return true;
        }
    }
    false
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
