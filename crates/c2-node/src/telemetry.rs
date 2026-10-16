use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use c2_exchange::{picture, ENTITY};
use c2_model::C2Message;
use mqp2p::Peer;
use opentelemetry::metrics::{Counter, Gauge, Meter, MeterProvider};
use opentelemetry::KeyValue;
use opentelemetry_otlp::MetricExporter;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::Resource;
use stitch_p2p::{MutationEvent, Op, PeerId, Store, WriteOrigin};
use tokio::sync::{broadcast, watch};
use tokio::time::{interval, MissedTickBehavior};

use crate::{node_identity, picture_digest, Result};

const ENDPOINT_VARIABLES: [&str; 2] = [
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
];
const INTERVAL_VARIABLE: &str = "OTEL_METRIC_EXPORT_INTERVAL";
const DEFAULT_INTERVAL_MS: u64 = 60_000;
const SERVICE_NAME: &str = "c2-node";

pub type PeerHandle = watch::Receiver<Option<Arc<Peer>>>;

#[derive(Clone)]
pub struct Metrics {
    mutations: Counter<u64>,
    events_missed: Counter<u64>,
    broker_reachable: Gauge<u64>,
    peers_live: Gauge<u64>,
    peer_lease: Gauge<u64>,
    cursor: Gauge<u64>,
    picture_records: Gauge<u64>,
    picture_unreadable: Gauge<u64>,
    picture_digest: Gauge<u64>,
}

impl Metrics {
    fn new(meter: &Meter) -> Self {
        Self {
            mutations: meter
                .u64_counter("c2.mutations")
                .with_description(
                    "Changes to the visible picture, by origin, operation and message type",
                )
                .build(),
            events_missed: meter
                .u64_counter("c2.telemetry.events_missed")
                .with_description("Mutation events the exporter fell behind on and did not count")
                .build(),
            broker_reachable: meter
                .u64_gauge("c2.broker.reachable")
                .with_description(
                    "1 when the last presence lookup on the discovery broker succeeded",
                )
                .build(),
            peers_live: meter
                .u64_gauge("c2.peers.live")
                .with_description("Other nodes holding a live presence lease")
                .build(),
            peer_lease: meter
                .u64_gauge("c2.peer.lease_remaining")
                .with_description(
                    "Seconds left on each peer's presence lease, 0 once it has lapsed",
                )
                .with_unit("s")
                .build(),
            cursor: meter
                .u64_gauge("c2.replication.cursor")
                .with_description("Highest write sequence delivered from each origin node")
                .build(),
            picture_records: meter
                .u64_gauge("c2.picture.records")
                .with_description("Messages in this node's common operational picture")
                .build(),
            picture_unreadable: meter
                .u64_gauge("c2.picture.unreadable")
                .with_description(
                    "Records in the store that are not readable C2 messages and are left out of the picture",
                )
                .build(),
            picture_digest: meter
                .u64_gauge("c2.picture.digest")
                .with_description(
                    "First 32 bits of the picture's SHA-256; equal values mean equal pictures",
                )
                .build(),
        }
    }
}

pub struct Telemetry {
    provider: SdkMeterProvider,
    metrics: Metrics,
    interval: Duration,
}

impl Telemetry {
    pub fn from_env(name: &str) -> Result<Option<Self>> {
        if ENDPOINT_VARIABLES
            .iter()
            .all(|variable| std::env::var(variable).map_or(true, |value| value.is_empty()))
        {
            return Ok(None);
        }
        let interval_ms = match std::env::var(INTERVAL_VARIABLE) {
            Ok(value) => value
                .parse()
                .map_err(|error| format!("invalid {INTERVAL_VARIABLE}: {error}"))?,
            Err(_) => DEFAULT_INTERVAL_MS,
        };
        let exporter = MetricExporter::builder().with_http().build()?;
        let resource = Resource::builder()
            .with_service_name(SERVICE_NAME)
            .with_attribute(KeyValue::new("service.instance.id", name.to_string()))
            .build();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter)
            .with_resource(resource)
            .build();
        let metrics = Metrics::new(&provider.meter(SERVICE_NAME));
        Ok(Some(Self {
            provider,
            metrics,
            interval: Duration::from_millis(interval_ms),
        }))
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
    }

    pub fn shutdown(&self) -> Result<()> {
        self.provider.shutdown()?;
        Ok(())
    }
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Insert => "insert",
        Op::Update => "update",
        Op::Delete => "delete",
    }
}

fn origin_name(origin: WriteOrigin) -> &'static str {
    match origin {
        WriteOrigin::Local => "local",
        WriteOrigin::Remote => "remote",
    }
}

fn mutation_attributes(event: &MutationEvent) -> [KeyValue; 4] {
    let record = event
        .data
        .as_deref()
        .filter(|_| event.entity == ENTITY)
        .and_then(|data| serde_json::from_slice::<serde_json::Value>(data).ok());
    let message_type = record
        .as_ref()
        .and_then(|value| value.get("message_type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let labelled = record
        .as_ref()
        .is_some_and(|value| value.get("label").is_some());
    [
        KeyValue::new("origin", origin_name(event.origin)),
        KeyValue::new("op", op_name(event.op)),
        KeyValue::new("message_type", message_type),
        KeyValue::new("labelled", labelled),
    ]
}

pub async fn run_mutations(mut events: broadcast::Receiver<MutationEvent>, metrics: Metrics) {
    loop {
        match events.recv().await {
            Ok(event) => metrics.mutations.add(1, &mutation_attributes(&event)),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                metrics.events_missed.add(missed, &[])
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn short(peer: &PeerId) -> String {
    peer[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Sampler {
    store: Arc<Store>,
    peer: PeerHandle,
    metrics: Metrics,
    names: HashMap<PeerId, String>,
    seen: BTreeSet<String>,
}

impl Sampler {
    async fn presence(&mut self) {
        let Some(peer) = self.peer.borrow().clone() else {
            self.metrics.broker_reachable.record(0, &[]);
            return;
        };
        let live = match peer.discover_peers().await {
            Ok(list) => {
                self.metrics.broker_reachable.record(1, &[]);
                let own = peer.peer_id().unwrap_or_default().to_string();
                list.into_iter()
                    .filter(|info| info.id != own)
                    .collect::<Vec<_>>()
            }
            Err(error) => {
                eprintln!("TELEMETRY presence lookup failed: {error}");
                self.metrics.broker_reachable.record(0, &[]);
                Vec::new()
            }
        };
        self.metrics.peers_live.record(live.len() as u64, &[]);
        let now = unix_now();
        let mut current = BTreeSet::new();
        for info in live {
            let remaining = info
                .expires_at
                .map_or(0, |expires| expires.saturating_sub(now));
            self.metrics
                .peer_lease
                .record(remaining, &[KeyValue::new("peer", info.name.clone())]);
            self.names
                .insert(node_identity(&info.name), info.name.clone());
            current.insert(info.name);
        }
        for lapsed in self.seen.difference(&current) {
            self.metrics
                .peer_lease
                .record(0, &[KeyValue::new("peer", lapsed.clone())]);
        }
        self.seen.extend(current);
    }

    async fn replication(&self) {
        let cursors = self.store.node().state().lock().await.cursors();
        for (origin, sequence) in cursors {
            let label = self
                .names
                .get(&origin)
                .cloned()
                .unwrap_or_else(|| short(&origin));
            self.metrics
                .cursor
                .record(sequence, &[KeyValue::new("origin", label)]);
        }
    }

    async fn picture(&self) {
        let picture = picture(&self.store).await;
        let sorted: BTreeMap<&String, &C2Message> = picture.messages.iter().collect();
        self.metrics
            .picture_records
            .record(sorted.len() as u64, &[]);
        self.metrics
            .picture_unreadable
            .record(picture.unreadable.len() as u64, &[]);
        match picture_digest(&sorted) {
            Ok(digest) => self
                .metrics
                .picture_digest
                .record(u64::from(u32::from_be_bytes(digest)), &[]),
            Err(error) => eprintln!("TELEMETRY digest failed: {error}"),
        }
    }
}

pub async fn run_sampler(
    store: Arc<Store>,
    peer: PeerHandle,
    name: String,
    metrics: Metrics,
    every: Duration,
) {
    let mut sampler = Sampler {
        store,
        peer,
        metrics,
        names: HashMap::from([(node_identity(&name), name)]),
        seen: BTreeSet::new(),
    };
    let mut ticker = interval(every);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        sampler.presence().await;
        sampler.replication().await;
        sampler.picture().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(entity: &str, data: Option<&str>, origin: WriteOrigin) -> MutationEvent {
        MutationEvent {
            op: if data.is_some() {
                Op::Insert
            } else {
                Op::Delete
            },
            entity: entity.to_string(),
            id: "C-1".to_string(),
            data: data.map(|text| text.as_bytes().to_vec()),
            origin,
        }
    }

    fn value<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a opentelemetry::Value> {
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| &attribute.value)
    }

    #[test]
    fn labelled_contact_report_is_classified() {
        let attributes = mutation_attributes(&event(
            ENTITY,
            Some(
                r#"{"message_type":"ContactReport","id":"C-1","label":{"classification":"UNCLASSIFIED"}}"#,
            ),
            WriteOrigin::Remote,
        ));
        assert_eq!(value(&attributes, "origin"), Some(&"remote".into()));
        assert_eq!(value(&attributes, "op"), Some(&"insert".into()));
        assert_eq!(
            value(&attributes, "message_type"),
            Some(&"ContactReport".into())
        );
        assert_eq!(value(&attributes, "labelled"), Some(&true.into()));
    }

    #[test]
    fn unlabelled_local_write_is_counted_as_such() {
        let attributes = mutation_attributes(&event(
            ENTITY,
            Some(r#"{"message_type":"PositionReport","id":"P-1"}"#),
            WriteOrigin::Local,
        ));
        assert_eq!(value(&attributes, "origin"), Some(&"local".into()));
        assert_eq!(
            value(&attributes, "message_type"),
            Some(&"PositionReport".into())
        );
        assert_eq!(value(&attributes, "labelled"), Some(&false.into()));
    }

    #[test]
    fn deletes_and_other_entities_have_unknown_type() {
        let deleted = mutation_attributes(&event(ENTITY, None, WriteOrigin::Remote));
        assert_eq!(value(&deleted, "op"), Some(&"delete".into()));
        assert_eq!(value(&deleted, "message_type"), Some(&"unknown".into()));
        let other = mutation_attributes(&event(
            "members",
            Some(r#"{"message_type":"ContactReport"}"#),
            WriteOrigin::Remote,
        ));
        assert_eq!(value(&other, "message_type"), Some(&"unknown".into()));
    }
}
