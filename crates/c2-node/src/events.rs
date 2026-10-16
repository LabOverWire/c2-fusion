use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use c2_exchange::ENTITY;
use c2_model::C2Message;
use serde::Serialize;
use serde_json::value::RawValue;
use stitch_p2p::{MutationEvent, Op, PeerId, Store, WriteOrigin};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Semaphore};
use tokio::time::{sleep, timeout};

use crate::Result;

pub const SCHEMA: u32 = 2;
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONSUMERS: usize = 8;
const ACCEPT_RETRY: Duration = Duration::from_secs(1);

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Origin {
    Local,
    Remote,
}

impl From<WriteOrigin> for Origin {
    fn from(origin: WriteOrigin) -> Self {
        match origin {
            WriteOrigin::Local => Origin::Local,
            WriteOrigin::Remote => Origin::Remote,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Insert,
    Update,
    Delete,
}

impl From<Op> for Operation {
    fn from(op: Op) -> Self {
        match op {
            Op::Insert => Operation::Insert,
            Op::Update => Operation::Update,
            Op::Delete => Operation::Delete,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Line<'a> {
    Hello {
        node: &'a str,
        schema: u32,
    },
    Snapshot {
        id: &'a str,
        origin: Origin,
        record: Box<RawValue>,
    },
    Ready {
        count: usize,
        at: String,
    },
    Change {
        op: Operation,
        id: &'a str,
        origin: Origin,
        at: String,
        record: Option<Box<RawValue>>,
    },
    Resync {
        missed: u64,
    },
    Unreadable {
        id: &'a str,
        origin: Origin,
        error: String,
    },
}

#[derive(Serialize)]
struct Envelope<'a> {
    seq: u64,
    #[serde(flatten)]
    line: Line<'a>,
}

fn now() -> String {
    humantime::format_rfc3339_millis(SystemTime::now()).to_string()
}

fn record(data: &[u8]) -> serde_json::Result<Box<RawValue>> {
    let message: C2Message = serde_json::from_slice(data)?;
    serde_json::value::to_raw_value(&message)
}

fn change(event: &MutationEvent) -> Option<Line<'_>> {
    if event.entity != ENTITY {
        return None;
    }
    let id = &event.id;
    let origin = event.origin.into();
    Some(match event.data.as_deref().map(record).transpose() {
        Ok(record) => Line::Change {
            op: event.op.into(),
            id,
            origin,
            at: now(),
            record,
        },
        Err(error) => Line::Unreadable {
            id,
            origin,
            error: error.to_string(),
        },
    })
}

struct Consumer {
    writer: BufWriter<TcpStream>,
    seq: u64,
    write_timeout: Duration,
}

impl Consumer {
    async fn send(&mut self, line: Line<'_>) -> Result<()> {
        let mut bytes = serde_json::to_vec(&Envelope {
            seq: self.seq,
            line,
        })?;
        bytes.push(b'\n');
        timeout(self.write_timeout, self.writer.write_all(&bytes)).await??;
        self.seq += 1;
        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        timeout(self.write_timeout, self.writer.flush()).await??;
        Ok(())
    }

    async fn snapshot(&mut self, store: &Store, own: PeerId) -> Result<()> {
        let mut entries = store.node().entries_with_authors(ENTITY).await;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (id, data, author) in &entries {
            let origin = if *author == own {
                Origin::Local
            } else {
                Origin::Remote
            };
            let line = match record(data) {
                Ok(record) => Line::Snapshot { id, origin, record },
                Err(error) => Line::Unreadable {
                    id,
                    origin,
                    error: error.to_string(),
                },
            };
            self.send(line).await?;
        }
        self.send(Line::Ready {
            count: entries.len(),
            at: now(),
        })
        .await?;
        self.flush().await
    }

    async fn run(
        mut self,
        mut events: broadcast::Receiver<MutationEvent>,
        store: &Store,
        name: &str,
        own: PeerId,
    ) -> Result<()> {
        self.send(Line::Hello {
            node: name,
            schema: SCHEMA,
        })
        .await?;
        self.snapshot(store, own).await?;
        loop {
            match events.recv().await {
                Ok(event) => {
                    if let Some(line) = change(&event) {
                        self.send(line).await?;
                        self.flush().await?;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    events = store.subscribe().await;
                    self.send(Line::Resync { missed }).await?;
                    self.snapshot(store, own).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }
}

async fn serve_consumer(
    socket: TcpStream,
    address: SocketAddr,
    store: Arc<Store>,
    name: Arc<str>,
    own: PeerId,
    write_timeout: Duration,
) {
    println!("EVENTS name={name} consumer={address} connected");
    let events = store.subscribe().await;
    let consumer = Consumer {
        writer: BufWriter::new(socket),
        seq: 0,
        write_timeout,
    };
    match consumer.run(events, &store, &name, own).await {
        Ok(()) => println!("EVENTS name={name} consumer={address} closed"),
        Err(error) => println!("EVENTS name={name} consumer={address} disconnected: {error}"),
    }
}

pub async fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    name: String,
    own: PeerId,
    write_timeout: Duration,
) {
    let name: Arc<str> = name.into();
    let slots = Arc::new(Semaphore::new(MAX_CONSUMERS));
    loop {
        let (socket, address) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("EVENTS name={name} accept-error={error}");
                sleep(ACCEPT_RETRY).await;
                continue;
            }
        };
        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
            eprintln!(
                "EVENTS name={name} consumer={address} refused: {MAX_CONSUMERS} already connected"
            );
            continue;
        };
        let store = Arc::clone(&store);
        let name = Arc::clone(&name);
        tokio::spawn(async move {
            serve_consumer(socket, address, store, name, own, write_timeout).await;
            drop(slot);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_exchange::ingest;
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, BufReader, Lines};

    const CONTACT: &str = r#"{"message_type":"ContactReport","id":"C-1","reporting_unit":"UAV-7","domain":"Air","service":"Intelligence","observed_at":"2026-09-29T16:00:00Z","latitude":49.3,"longitude":-123.1,"description":"test contact"}"#;

    fn contact(id: &str) -> C2Message {
        let mut value: Value = serde_json::from_str(CONTACT).expect("fixture is json");
        value["id"] = Value::from(id);
        serde_json::from_value(value).expect("fixture is a C2 message")
    }

    async fn listen(store: Arc<Store>, own: PeerId, write_timeout: Duration) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(serve(
            listener,
            store,
            "test".to_string(),
            own,
            write_timeout,
        ));
        address
    }

    async fn open(address: SocketAddr) -> Lines<BufReader<TcpStream>> {
        let stream = TcpStream::connect(address).await.expect("connect");
        BufReader::new(stream).lines()
    }

    async fn connect(store: Arc<Store>, own: PeerId) -> Lines<BufReader<TcpStream>> {
        open(listen(store, own, WRITE_TIMEOUT).await).await
    }

    async fn drain(lines: &mut Lines<BufReader<TcpStream>>, quiet: Duration) -> Vec<Value> {
        let mut values = Vec::new();
        while let Ok(Ok(Some(line))) = timeout(quiet, lines.next_line()).await {
            values.push(serde_json::from_str(&line).expect("json line"));
        }
        values
    }

    fn description(value: &Value) -> u32 {
        value["record"]["description"]
            .as_str()
            .and_then(|text| text.parse().ok())
            .expect("numbered description")
    }

    fn described(id: &str, text: &str) -> C2Message {
        let mut value = serde_json::to_value(contact(id)).expect("model json");
        value["description"] = Value::from(text);
        serde_json::from_value(value).expect("fixture is a C2 message")
    }

    async fn next(lines: &mut Lines<BufReader<TcpStream>>) -> (String, Value) {
        let line = lines.next_line().await.expect("read").expect("line");
        let value = serde_json::from_str(&line).expect("json line");
        (line, value)
    }

    #[tokio::test]
    async fn snapshot_then_live_changes() {
        let own = [1; 32];
        let store = Arc::new(Store::new(own));
        ingest(&store, &contact("C-1")).await.expect("ingest");
        let mut lines = connect(Arc::clone(&store), own).await;

        let (_, hello) = next(&mut lines).await;
        assert_eq!(hello["kind"], "hello");
        assert_eq!(hello["node"], "test");
        assert_eq!(hello["schema"], SCHEMA);
        assert_eq!(hello["seq"], 0);

        let (text, snapshot) = next(&mut lines).await;
        assert_eq!(snapshot["kind"], "snapshot");
        assert_eq!(snapshot["id"], "C-1");
        assert_eq!(snapshot["origin"], "local");
        let canonical = serde_json::to_string(&contact("C-1")).expect("model json");
        assert!(text.ends_with(&format!(r#""record":{canonical}}}"#)));

        let (_, ready) = next(&mut lines).await;
        assert_eq!(ready["kind"], "ready");
        assert_eq!(ready["count"], 1);

        ingest(&store, &contact("C-2")).await.expect("ingest");
        let (_, inserted) = next(&mut lines).await;
        assert_eq!(inserted["kind"], "change");
        assert_eq!(inserted["op"], "insert");
        assert_eq!(inserted["id"], "C-2");
        assert_eq!(inserted["origin"], "local");
        assert_eq!(inserted["seq"], 3);
        assert_eq!(inserted["record"]["message_type"], "ContactReport");

        store.delete(ENTITY, "C-1").await.expect("delete");
        let (_, deleted) = next(&mut lines).await;
        assert_eq!(deleted["op"], "delete");
        assert_eq!(deleted["id"], "C-1");
        assert_eq!(deleted["record"], Value::Null);
    }

    #[test]
    fn other_entities_are_skipped() {
        let event = MutationEvent {
            op: Op::Insert,
            entity: "members".to_string(),
            id: "x".to_string(),
            data: Some(b"{}".to_vec()),
            origin: WriteOrigin::Remote,
        };
        assert!(change(&event).is_none());
    }

    #[tokio::test]
    async fn unreadable_records_are_reported_and_the_stream_continues() {
        let own = [1; 32];
        let store = Arc::new(Store::new(own));
        ingest(&store, &contact("C-1")).await.expect("ingest");
        store
            .create(ENTITY, "BAD", serde_json::json!({"foo": 1}))
            .await
            .expect("create");
        let mut lines = connect(Arc::clone(&store), own).await;

        let kinds: Vec<(String, String)> = {
            let mut kinds = Vec::new();
            for _ in 0..4 {
                let (_, value) = next(&mut lines).await;
                kinds.push((
                    value["kind"].as_str().unwrap_or_default().to_string(),
                    value["id"].as_str().unwrap_or_default().to_string(),
                ));
            }
            kinds
        };
        assert_eq!(
            kinds,
            [
                ("hello".to_string(), String::new()),
                ("unreadable".to_string(), "BAD".to_string()),
                ("snapshot".to_string(), "C-1".to_string()),
                ("ready".to_string(), String::new()),
            ]
        );

        store
            .node()
            .local_write(Op::Update, ENTITY, "C-1", b"not json".to_vec())
            .await
            .expect("local write");
        let (_, unreadable) = next(&mut lines).await;
        assert_eq!(unreadable["kind"], "unreadable");
        assert_eq!(unreadable["id"], "C-1");
        assert_eq!(unreadable["origin"], "local");

        ingest(&store, &contact("C-2")).await.expect("ingest");
        let (_, inserted) = next(&mut lines).await;
        assert_eq!(inserted["kind"], "change");
        assert_eq!(inserted["id"], "C-2");
        assert_eq!(inserted["seq"], 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn after_a_resync_no_change_is_older_than_the_snapshot() {
        let own = [1; 32];
        let store = Arc::new(Store::new(own));
        let mut lines = connect(Arc::clone(&store), own).await;
        next(&mut lines).await;
        next(&mut lines).await;
        let big = "b".repeat(256 * 1024);
        for i in 0..100 {
            ingest(&store, &described(&format!("BIG-{i}"), &big))
                .await
                .expect("ingest");
        }
        for version in 0..1100 {
            ingest(&store, &described("X", &version.to_string()))
                .await
                .expect("ingest");
        }
        let lines: Vec<Value> = drain(&mut lines, Duration::from_secs(3))
            .await
            .into_iter()
            .filter(|value| value["id"] == "X" || value["kind"] != "change")
            .filter(|value| value["id"] == "X" || value["kind"] != "snapshot")
            .collect();
        let resync = lines
            .iter()
            .position(|value| value["kind"] == "resync")
            .expect("consumer fell behind");
        let after = &lines[resync..];
        let snapshot = description(
            after
                .iter()
                .find(|value| value["kind"] == "snapshot")
                .expect("X in the new snapshot"),
        );
        assert_eq!(snapshot, 1099);
        let older: Vec<u32> = after
            .iter()
            .filter(|value| value["kind"] == "change")
            .map(description)
            .filter(|version| *version <= snapshot)
            .collect();
        assert!(
            older.is_empty(),
            "{} changes up to {:?} after snapshot {snapshot}",
            older.len(),
            older.last()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn consumers_that_stop_reading_give_up_their_slots() {
        let own = [1; 32];
        let store = Arc::new(Store::new(own));
        let address = listen(Arc::clone(&store), own, Duration::from_millis(300)).await;
        let mut stuck = Vec::new();
        for _ in 0..MAX_CONSUMERS {
            stuck.push(TcpStream::connect(address).await.expect("connect"));
        }
        sleep(Duration::from_millis(200)).await;
        let big = "b".repeat(256 * 1024);
        for i in 0..60 {
            ingest(&store, &described(&format!("BIG-{i}"), &big))
                .await
                .expect("ingest");
        }
        sleep(Duration::from_secs(1)).await;
        let mut lines = open(address).await;
        let (_, hello) = timeout(Duration::from_secs(2), next(&mut lines))
            .await
            .expect("a slot was freed");
        assert_eq!(hello["kind"], "hello");
        drop(stuck);
    }
}
