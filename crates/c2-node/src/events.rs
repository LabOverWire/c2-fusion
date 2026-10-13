use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use c2_exchange::ENTITY;
use c2_model::C2Message;
use serde::Serialize;
use serde_json::value::RawValue;
use stitch_p2p::{MutationEvent, Op, PeerId, Store, WriteOrigin};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Semaphore};

use crate::Result;

pub const SCHEMA: u32 = 1;
const MAX_CONSUMERS: usize = 8;

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

fn record(data: &[u8]) -> Result<Box<RawValue>> {
    let message: C2Message = serde_json::from_slice(data)?;
    Ok(serde_json::value::to_raw_value(&message)?)
}

fn change(event: &MutationEvent) -> Result<Option<Line<'_>>> {
    if event.entity != ENTITY {
        return Ok(None);
    }
    Ok(Some(Line::Change {
        op: event.op.into(),
        id: &event.id,
        origin: event.origin.into(),
        at: now(),
        record: event.data.as_deref().map(record).transpose()?,
    }))
}

struct Consumer {
    writer: BufWriter<TcpStream>,
    seq: u64,
}

impl Consumer {
    async fn send(&mut self, line: Line<'_>) -> Result<()> {
        let mut bytes = serde_json::to_vec(&Envelope {
            seq: self.seq,
            line,
        })?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await?;
        self.seq += 1;
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
            self.send(Line::Snapshot {
                id,
                origin,
                record: record(data)?,
            })
            .await?;
        }
        self.send(Line::Ready {
            count: entries.len(),
            at: now(),
        })
        .await?;
        self.writer.flush().await?;
        Ok(())
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
                    if let Some(line) = change(&event)? {
                        self.send(line).await?;
                        self.writer.flush().await?;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
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
) {
    println!("EVENTS name={name} consumer={address} connected");
    let events = store.subscribe().await;
    let consumer = Consumer {
        writer: BufWriter::new(socket),
        seq: 0,
    };
    match consumer.run(events, &store, &name, own).await {
        Ok(()) => println!("EVENTS name={name} consumer={address} closed"),
        Err(error) => println!("EVENTS name={name} consumer={address} disconnected: {error}"),
    }
}

pub async fn serve(listener: TcpListener, store: Arc<Store>, name: String, own: PeerId) {
    let name: Arc<str> = name.into();
    let slots = Arc::new(Semaphore::new(MAX_CONSUMERS));
    loop {
        let (socket, address) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("EVENTS name={name} accept-error={error}");
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
            serve_consumer(socket, address, store, name, own).await;
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

    async fn connect(store: Arc<Store>, own: PeerId) -> Lines<BufReader<TcpStream>> {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(serve(listener, store, "test".to_string(), own));
        let stream = TcpStream::connect(address).await.expect("connect");
        BufReader::new(stream).lines()
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

        store.delete(ENTITY, "C-1").await;
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
        assert!(change(&event).expect("no error").is_none());
    }
}
