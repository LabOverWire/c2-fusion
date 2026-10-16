use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use c2_exchange::radio::{split, Hello, RadioMessage, Reassembler, Received};
use c2_lora::{
    open_serial, Config, Due, Frame, Inbound, Link, LoraError, Outgoing, Radio, ResetLine, MAX_DATA,
};
use c2_model::C2Message;
use stitch_p2p::{PeerId, RecordOutcome, SyncNode, SyncState, WriteFrame};
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
    let self_id = node.state().lock().await.self_id();
    let mut exchange = Exchange::new(self_id, seed(settings.config.address) as u16);
    loop {
        let Err(error) = session(
            &settings,
            &mut link,
            &node,
            &name,
            &mut exchange,
            &mut writes,
        )
        .await;
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
    self_id: PeerId,
    queue: BTreeMap<(PeerId, u64), WriteFrame>,
    wire: VecDeque<(Vec<u8>, String)>,
    staged: Option<(PeerId, u64)>,
    reassembler: Reassembler,
    fragment_id: u16,
    held: BTreeMap<(PeerId, u64), WriteFrame>,
    behind: bool,
    dropped_local: u64,
    hello_due: bool,
    hello_start: Option<PeerId>,
    last_hello: Option<Instant>,
}

impl Exchange {
    fn new(self_id: PeerId, fragment_id: u16) -> Self {
        Self {
            self_id,
            queue: BTreeMap::new(),
            wire: VecDeque::new(),
            staged: None,
            reassembler: Reassembler::default(),
            fragment_id,
            held: BTreeMap::new(),
            behind: false,
            dropped_local: 0,
            hello_due: false,
            hello_start: None,
            last_hello: None,
        }
    }

    fn enqueue(&mut self, frame: WriteFrame) {
        self.queue.insert((frame.stamp.peer, frame.seq), frame);
        while self.queue.len() > QUEUE_LIMIT {
            if let Some(((origin, _), _)) = self.queue.pop_last() {
                self.behind |= origin == self.self_id;
            }
        }
    }

    fn accept_local(&mut self, frame: WriteFrame) {
        match self.behind {
            true => self.dropped_local += 1,
            false => self.enqueue(frame),
        }
    }

    fn merge_delta(&mut self, hello: &Hello, delta: Vec<WriteFrame>, link_busy: bool) {
        if hello.covers(&self.self_id) {
            self.behind = false;
        }
        let sending = self.staged.filter(|_| link_busy || !self.wire.is_empty());
        for frame in delta {
            if sending != Some((frame.stamp.peer, frame.seq)) {
                self.enqueue(frame);
            }
        }
    }

    fn settle_held(&mut self, guard: &mut SyncState) -> usize {
        let mut released = 0;
        loop {
            let cursors = guard.cursors();
            let delivered = |origin: &PeerId| cursors.get(origin).copied().unwrap_or(0);
            self.held
                .retain(|(origin, seq), _| *seq > delivered(origin));
            let Some(next) = self
                .held
                .keys()
                .find(|(origin, seq)| *seq == delivered(origin) + 1)
                .copied()
            else {
                return released;
            };
            if let Some(frame) = self.held.remove(&next) {
                if guard.receive(frame, now_millis()) == RecordOutcome::Appended {
                    released += 1;
                }
            }
        }
    }

    fn request_hello(&mut self) {
        self.hello_due = true;
    }

    fn request_hello_after_gap(&mut self, origin: PeerId) {
        let spaced = self
            .last_hello
            .is_none_or(|sent| sent.elapsed() >= GAP_HELLO_SPACING);
        if spaced {
            self.hello_due = true;
            self.hello_start = Some(origin);
        }
    }
}

async fn session(
    settings: &LoraSettings,
    link: &mut Link,
    node: &SyncNode,
    name: &str,
    exchange: &mut Exchange,
    writes: &mut mpsc::UnboundedReceiver<WriteFrame>,
) -> Result<Infallible, LoraError> {
    let reset = ResetLine::open(settings.reset_gpio).await?;
    let mut radio = Radio::new(open_serial(&settings.port)?, Some(reset));
    let ready = radio.reset().await?;
    let changed = radio.configure(&settings.config).await?;
    println!(
        "LORA name={name} ready address={} spreading_factor={} bandwidth={:?} reset_ms={} changed={changed}",
        settings.config.address,
        settings.config.modulation.spreading_factor,
        settings.config.modulation.bandwidth,
        ready.as_millis()
    );

    let mut health = interval(HEALTH_INTERVAL);
    health.set_missed_tick_behavior(MissedTickBehavior::Delay);
    health.tick().await;
    let mut hello = interval(HELLO_INTERVAL);
    hello.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        let due = link.next_due().map(Instant::from_std);
        let ready_to_send = exchange.hello_due
            || (link.has_capacity() && !(exchange.wire.is_empty() && exchange.queue.is_empty()));
        tokio::select! {
            biased;
            frame = radio.recv() => {
                receive(&mut radio, link, node, name, exchange, frame?).await?;
            }
            _ = wait_until(due) => retry(&mut radio, link, name).await?,
            _ = std::future::ready(()), if ready_to_send => {
                send_next(&mut radio, link, node, name, exchange).await?;
            }
            Some(frame) = writes.recv() => exchange.accept_local(frame),
            _ = hello.tick() => exchange.request_hello(),
            _ = health.tick() => {
                let round_trip = radio.refresh().await?;
                let released = exchange.settle_held(&mut *node.state().lock().await);
                println!(
                    "LORA name={name} health ping_ms={} dropped_frames={} queued={} held={} released={released} behind={} dropped_local={}",
                    round_trip.as_millis(),
                    radio.dropped_frames(),
                    exchange.queue.len(),
                    exchange.held.len(),
                    exchange.behind,
                    exchange.dropped_local
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
    if exchange.hello_due {
        exchange.hello_due = false;
        exchange.last_hello = Some(Instant::now());
        let (from, ours) = {
            let state = node.state();
            let guard = state.lock().await;
            (guard.self_id(), guard.cursors())
        };
        let hello = Hello::window(from, &ours, exchange.hello_start);
        exchange.hello_start = hello.next_start();
        let label = format!("hello cursors={}/{}", hello.cursors.len(), ours.len());
        return match RadioMessage::Hello(hello).encode() {
            Ok(payload) => send(radio, link, name, &payload, &label, Delivery::Datagram).await,
            Err(error) => {
                eprintln!("LORA name={name} skip {label} error={error}");
                Ok(())
            }
        };
    }
    if exchange.wire.is_empty() {
        let Some((key, frame)) = exchange.queue.pop_first() else {
            return Ok(());
        };
        exchange.staged = Some(key);
        stage(name, exchange, frame);
    }
    match exchange.wire.pop_front() {
        Some((payload, label)) => send(radio, link, name, &payload, &label, Delivery::Acked).await,
        None => Ok(()),
    }
}

fn stage(name: &str, exchange: &mut Exchange, frame: WriteFrame) {
    let label = format!(
        "write id={} origin={} origin_seq={}",
        frame.id,
        short(&frame.stamp.peer),
        frame.seq
    );
    let payload = match RadioMessage::Write(frame).encode() {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("LORA name={name} skip {label} error={error}");
            return;
        }
    };
    let id = exchange.fragment_id;
    exchange.fragment_id = id.wrapping_add(1);
    match split(&payload, MAX_DATA, id) {
        Ok(parts) if parts.len() == 1 => exchange
            .wire
            .extend(parts.into_iter().map(|part| (part, label.clone()))),
        Ok(parts) => {
            let count = parts.len();
            exchange
                .wire
                .extend(parts.into_iter().enumerate().map(|(index, part)| {
                    (
                        part,
                        format!("{label} fragment={}/{count} fragment_id={id}", index + 1),
                    )
                }));
        }
        Err(error) => eprintln!(
            "LORA name={name} skip {label} len={} error={error}",
            payload.len()
        ),
    }
}

enum Delivery {
    Acked,
    Datagram,
}

async fn send(
    radio: &mut Radio<SerialStream>,
    link: &mut Link,
    name: &str,
    payload: &[u8],
    label: &str,
    delivery: Delivery,
) -> Result<(), LoraError> {
    let now = std::time::Instant::now();
    let sent = match delivery {
        Delivery::Acked => link.originate(payload, now),
        Delivery::Datagram => link.datagram(payload, now),
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
    frame: Frame,
) -> Result<(), LoraError> {
    let source = frame.source;
    let rssi = frame.rssi_dbm;
    let link_busy = !link.has_capacity();
    match link.receive(source, &frame.payload, std::time::Instant::now()) {
        Inbound::Deliver {
            sequence,
            payload,
            ack,
        } => {
            transmit(radio, &ack).await?;
            dispatch(node, name, exchange, link_busy, &frame, sequence, payload).await;
        }
        Inbound::Datagram { sequence, payload } => {
            dispatch(node, name, exchange, link_busy, &frame, sequence, payload).await;
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
    link_busy: bool,
    frame: &Frame,
    sequence: u16,
    payload: &[u8],
) {
    let source = frame.source;
    let rssi = frame.rssi_dbm;
    match exchange
        .reassembler
        .accept(source, payload, std::time::Instant::now())
    {
        Ok(Received::Complete(RadioMessage::Write(write))) => {
            apply(node, name, exchange, source, rssi, frame.snr, write).await;
        }
        Ok(Received::Complete(RadioMessage::Hello(hello))) => {
            answer_hello(node, name, exchange, link_busy, hello).await;
        }
        Ok(Received::Partial { id, have, count }) => println!(
            "LORA name={name} rx fragment from={source} fragment_id={id} have={have}/{count} rssi_dbm={rssi}"
        ),
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
    let label = serde_json::from_slice::<C2Message>(&write.data)
        .ok()
        .and_then(|msg| msg.label().map(|label| label.classification.clone()))
        .unwrap_or_else(|| "none".to_string());
    let origin = write.stamp.peer;
    let origin_seq = write.seq;
    let state = node.state();
    let mut guard = state.lock().await;
    let outcome = guard.receive(write.clone(), now_millis());
    let mut released = exchange.settle_held(&mut guard);
    if let RecordOutcome::Gap { .. } = outcome {
        if exchange.held.len() < QUEUE_LIMIT {
            exchange.held.insert((origin, origin_seq), write);
        }
        exchange.request_hello_after_gap(origin);
        released += exchange.settle_held(&mut guard);
    }
    println!(
        "LORA name={name} rx write id={id} label={label} origin={} origin_seq={origin_seq} outcome={outcome:?} released={released} held={} from={source} rssi_dbm={rssi} snr={snr}",
        short(&origin),
        exchange.held.len()
    );
}

async fn answer_hello(
    node: &SyncNode,
    name: &str,
    exchange: &mut Exchange,
    link_busy: bool,
    hello: Hello,
) {
    let state = node.state();
    let mut guard = state.lock().await;
    if hello.is_complete() {
        guard.note_peer_cursors(hello.from, hello.cursors.clone().into_iter().collect());
    }
    let delta = guard.delta_since(&hello.their_cursors(&guard.cursors()));
    let missing = delta.len();
    exchange.merge_delta(&hello, delta, link_busy);
    println!(
        "LORA name={name} rx hello from={} cursors={} missing={missing} queued={} behind={}",
        short(&hello.from),
        hello.cursors.len(),
        exchange.queue.len(),
        exchange.behind
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

#[cfg(test)]
mod tests {
    use super::*;
    use stitch_p2p::{Hlc, Op, Stamp};

    const SELF: u8 = 200;

    fn origin(n: u8) -> PeerId {
        [n; 32]
    }

    fn write(from: u8, seq: u64) -> WriteFrame {
        WriteFrame {
            stamp: Stamp {
                hlc: Hlc {
                    physical: 1_791_671_100_000 + seq,
                    logical: 0,
                },
                peer: origin(from),
            },
            seq,
            op: Op::Insert,
            entity: "notes".to_string(),
            id: format!("n-{from}-{seq}"),
            data: b"{}".to_vec(),
            signature: None,
        }
    }

    async fn node_with(origins: u8, writes_each: u64) -> SyncNode {
        let node = SyncNode::new(origin(SELF));
        {
            let state = node.state();
            let mut guard = state.lock().await;
            for from in 1..=origins {
                for seq in 1..=writes_each {
                    assert_eq!(
                        guard.receive(write(from, seq), now_millis()),
                        RecordOutcome::Appended
                    );
                }
            }
        }
        node
    }

    fn exchange() -> Exchange {
        Exchange::new(origin(SELF), 1)
    }

    fn hello(from: u8, cursors: &[(u8, u64)], from_start: bool, to_end: bool) -> Hello {
        Hello {
            from: origin(from),
            cursors: cursors.iter().map(|(n, seq)| (origin(*n), *seq)).collect(),
            from_start,
            to_end,
        }
    }

    fn queued(exchange: &Exchange) -> Vec<(PeerId, u64)> {
        exchange.queue.keys().copied().collect()
    }

    #[tokio::test]
    async fn every_hello_window_fits_a_datagram_and_a_cycle_covers_every_origin() {
        let node = node_with(12, 1).await;
        let ours = node.state().lock().await.cursors();
        let mut link = Link::new(7);
        let mut start = None;
        let mut carried = std::collections::BTreeSet::new();
        for sent in 1.. {
            let hello = Hello::window(origin(SELF), &ours, start);
            let payload = RadioMessage::Hello(hello.clone()).encode().expect("encode");
            assert!(
                link.datagram(&payload, std::time::Instant::now()).is_ok(),
                "hello {sent}"
            );
            carried.extend(hello.cursors.keys().copied());
            start = hello.next_start();
            if start.is_none() {
                assert_eq!(sent, 3);
                break;
            }
        }
        assert!(ours.keys().all(|key| carried.contains(key)));
    }

    #[tokio::test]
    async fn partial_hello_queues_only_what_its_window_is_missing() {
        let node = node_with(8, 3).await;
        let mut exchange = exchange();
        let partial = hello(100, &[(1, 3), (2, 1), (3, 3), (4, 3), (5, 3)], true, false);
        answer_hello(&node, "test", &mut exchange, false, partial).await;
        assert_eq!(queued(&exchange), vec![(origin(2), 2), (origin(2), 3)]);
    }

    #[tokio::test]
    async fn complete_hello_still_queues_origins_the_peer_lacks() {
        let node = node_with(2, 2).await;
        let mut exchange = exchange();
        answer_hello(
            &node,
            "test",
            &mut exchange,
            false,
            hello(100, &[(1, 2)], true, true),
        )
        .await;
        assert_eq!(queued(&exchange), vec![(origin(2), 1), (origin(2), 2)]);
    }

    #[tokio::test]
    async fn hellos_from_two_peers_merge_into_one_queue() {
        let node = node_with(2, 2).await;
        let mut exchange = exchange();
        answer_hello(
            &node,
            "test",
            &mut exchange,
            false,
            hello(100, &[(1, 0), (2, 2)], true, true),
        )
        .await;
        answer_hello(
            &node,
            "test",
            &mut exchange,
            false,
            hello(101, &[(1, 2), (2, 1)], true, true),
        )
        .await;
        assert_eq!(
            queued(&exchange),
            vec![(origin(1), 1), (origin(1), 2), (origin(2), 2)]
        );
    }

    #[tokio::test]
    async fn frame_on_the_air_is_not_queued_again() {
        let node = node_with(1, 2).await;
        let mut exchange = exchange();
        exchange.staged = Some((origin(1), 1));
        let behind = hello(100, &[(1, 0)], true, true);
        answer_hello(&node, "test", &mut exchange, true, behind.clone()).await;
        assert_eq!(queued(&exchange), vec![(origin(1), 2)]);
        answer_hello(&node, "test", &mut exchange, false, behind).await;
        assert_eq!(queued(&exchange), vec![(origin(1), 1), (origin(1), 2)]);
    }

    #[test]
    fn local_writes_beyond_the_queue_are_dropped_until_a_hello_resends_them() {
        let mut exchange = exchange();
        for seq in 1..=1000 {
            exchange.accept_local(write(SELF, seq));
        }
        assert_eq!(exchange.queue.len(), QUEUE_LIMIT);
        assert!(exchange.behind);
        assert_eq!(exchange.dropped_local, 1000 - QUEUE_LIMIT as u64 - 1);
        assert_eq!(
            queued(&exchange).last(),
            Some(&(origin(SELF), QUEUE_LIMIT as u64))
        );
    }

    #[tokio::test]
    async fn hello_covering_this_node_clears_behind_and_requeues_its_writes() {
        let node = SyncNode::new(origin(SELF));
        let mut exchange = exchange();
        for seq in 1..=40 {
            let frame = node
                .local_write(Op::Insert, "notes", format!("n-{seq}"), b"{}".to_vec())
                .await
                .expect("local write");
            exchange.accept_local(frame);
        }
        assert!(exchange.behind);
        while exchange.queue.pop_first().is_some() {}
        answer_hello(
            &node,
            "test",
            &mut exchange,
            false,
            hello(100, &[(1, 4)], true, false),
        )
        .await;
        assert!(exchange.behind);
        assert!(exchange.queue.is_empty());
        answer_hello(
            &node,
            "test",
            &mut exchange,
            false,
            hello(100, &[(SELF, 32)], false, true),
        )
        .await;
        assert!(!exchange.behind);
        assert_eq!(
            queued(&exchange),
            (33..=40).map(|seq| (origin(SELF), seq)).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn held_frames_settle_when_the_gap_fills_by_another_path() {
        let node = node_with(1, 1).await;
        let state = node.state();
        let mut exchange = exchange();
        for seq in [3, 4, 6] {
            let mut guard = state.lock().await;
            assert!(matches!(
                guard.receive(write(1, seq), now_millis()),
                RecordOutcome::Gap { .. }
            ));
            exchange.held.insert((origin(1), seq), write(1, seq));
        }
        let mut guard = state.lock().await;
        assert_eq!(
            guard.receive(write(1, 2), now_millis()),
            RecordOutcome::Appended
        );
        assert_eq!(exchange.settle_held(&mut guard), 2);
        assert_eq!(
            exchange.held.keys().copied().collect::<Vec<_>>(),
            vec![(origin(1), 6)]
        );
        assert_eq!(
            guard.receive(write(1, 5), now_millis()),
            RecordOutcome::Appended
        );
        assert_eq!(exchange.settle_held(&mut guard), 1);
        assert!(exchange.held.is_empty());
        assert_eq!(guard.cursors().get(&origin(1)), Some(&6));
    }

    #[tokio::test]
    async fn held_frames_already_delivered_are_pruned() {
        let node = node_with(1, 1).await;
        let state = node.state();
        let mut exchange = exchange();
        exchange.held.insert((origin(1), 3), write(1, 3));
        let mut guard = state.lock().await;
        for seq in 2..=4 {
            assert_eq!(
                guard.receive(write(1, seq), now_millis()),
                RecordOutcome::Appended
            );
        }
        assert_eq!(exchange.settle_held(&mut guard), 0);
        assert!(exchange.held.is_empty());
    }

    #[test]
    fn gap_hello_starts_its_window_at_the_gapped_origin() {
        let mut exchange = exchange();
        exchange.request_hello_after_gap(origin(9));
        assert!(exchange.hello_due);
        assert_eq!(exchange.hello_start, Some(origin(9)));
        exchange.hello_due = false;
        exchange.hello_start = None;
        exchange.last_hello = Some(Instant::now());
        exchange.request_hello_after_gap(origin(3));
        assert!(!exchange.hello_due);
        assert_eq!(exchange.hello_start, None);
    }
}
