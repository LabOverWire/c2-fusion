use bebytes::BeBytes;
use c2_codec::compact::CompactCodec;
use c2_codec::Codec;
use c2_model::C2Message;
use stitch_p2p::{Cursors, Hlc, Op, PeerId, Stamp, WriteFrame};

use crate::ENTITY;

const WRITE: u8 = 0;
const HELLO: u8 = 1;

const BODY_JSON: u8 = 0;
const BODY_COMPACT: u8 = 1;

const OP_INSERT: u8 = 0;
const OP_UPDATE: u8 = 1;
const OP_DELETE: u8 = 2;

const WRITE_FIXED_LEN: usize = 8 + 4 + 32 + 8 + 1 + 1;
const HELLO_FIXED_LEN: usize = 32 + 1;
const CURSOR_LEN: usize = 32 + 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioMessage {
    Write(WriteFrame),
    Hello { from: PeerId, cursors: Cursors },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RadioError {
    #[error("signed frames are not carried over radio")]
    Signed,
    #[error("{0} exceeds 255 bytes")]
    TooLong(&'static str),
    #[error("{0} cursors exceed 255")]
    TooManyCursors(usize),
    #[error("empty radio message")]
    Empty,
    #[error("unknown radio message kind {0}")]
    Kind(u8),
    #[error("unknown op {0}")]
    Op(u8),
    #[error("unknown body kind {0}")]
    Body(u8),
    #[error("malformed radio message: {0}")]
    Malformed(String),
    #[error("{0} trailing bytes")]
    Trailing(usize),
    #[error("truncated radio message: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct WriteWire {
    physical: u64,
    logical: u32,
    origin: [u8; 32],
    seq: u64,
    op: u8,
    entity_len: u8,
    #[FromField(entity_len)]
    entity: String,
    id_len: u8,
    #[FromField(id_len)]
    id: String,
    body_kind: u8,
    body: Vec<u8>,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct CursorWire {
    origin: [u8; 32],
    seq: u64,
}

#[derive(BeBytes, Debug, Clone, PartialEq)]
struct HelloWire {
    from: [u8; 32],
    count: u8,
    #[FromField(count)]
    cursors: Vec<CursorWire>,
}

impl RadioMessage {
    pub fn encode(&self) -> Result<Vec<u8>, RadioError> {
        let (kind, body) = match self {
            RadioMessage::Write(frame) => (WRITE, encode_write(frame)?),
            RadioMessage::Hello { from, cursors } => (HELLO, encode_hello(from, cursors)?),
        };
        let mut bytes = vec![kind];
        bytes.extend_from_slice(&body);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RadioError> {
        let (kind, body) = bytes.split_first().ok_or(RadioError::Empty)?;
        match *kind {
            WRITE => decode_write(body).map(RadioMessage::Write),
            HELLO => decode_hello(body),
            other => Err(RadioError::Kind(other)),
        }
    }
}

fn encode_write(frame: &WriteFrame) -> Result<Vec<u8>, RadioError> {
    if frame.signature.is_some() {
        return Err(RadioError::Signed);
    }
    let (body_kind, body) = match compact_body(&frame.entity, &frame.data) {
        Some(compact) => (BODY_COMPACT, compact),
        None => (BODY_JSON, frame.data.clone()),
    };
    Ok(WriteWire {
        physical: frame.stamp.hlc.physical,
        logical: frame.stamp.hlc.logical,
        origin: frame.stamp.peer,
        seq: frame.seq,
        op: op_code(frame.op),
        entity_len: short_len(&frame.entity, "entity")?,
        entity: frame.entity.clone(),
        id_len: short_len(&frame.id, "id")?,
        id: frame.id.clone(),
        body_kind,
        body,
    }
    .to_be_bytes())
}

fn decode_write(bytes: &[u8]) -> Result<WriteFrame, RadioError> {
    at_least(bytes, WRITE_FIXED_LEN)?;
    let wire: WriteWire = whole(bytes)?;
    let data = match wire.body_kind {
        BODY_JSON => wire.body,
        BODY_COMPACT => record_bytes(&wire.body)?,
        other => return Err(RadioError::Body(other)),
    };
    Ok(WriteFrame {
        stamp: Stamp {
            hlc: Hlc {
                physical: wire.physical,
                logical: wire.logical,
            },
            peer: wire.origin,
        },
        seq: wire.seq,
        op: op_from(wire.op)?,
        entity: wire.entity,
        id: wire.id,
        data,
        signature: None,
    })
}

fn encode_hello(from: &PeerId, cursors: &Cursors) -> Result<Vec<u8>, RadioError> {
    let count =
        u8::try_from(cursors.len()).map_err(|_| RadioError::TooManyCursors(cursors.len()))?;
    let mut entries: Vec<CursorWire> = cursors
        .iter()
        .map(|(origin, seq)| CursorWire {
            origin: *origin,
            seq: *seq,
        })
        .collect();
    entries.sort_by_key(|entry| entry.origin);
    Ok(HelloWire {
        from: *from,
        count,
        cursors: entries,
    }
    .to_be_bytes())
}

fn decode_hello(bytes: &[u8]) -> Result<RadioMessage, RadioError> {
    at_least(bytes, HELLO_FIXED_LEN)?;
    at_least(bytes, HELLO_FIXED_LEN + usize::from(bytes[32]) * CURSOR_LEN)?;
    let wire: HelloWire = whole(bytes)?;
    Ok(RadioMessage::Hello {
        from: wire.from,
        cursors: wire
            .cursors
            .into_iter()
            .map(|entry| (entry.origin, entry.seq))
            .collect(),
    })
}

fn compact_body(entity: &str, data: &[u8]) -> Option<Vec<u8>> {
    if entity != ENTITY {
        return None;
    }
    let msg: C2Message = serde_json::from_slice(data).ok()?;
    let compact = CompactCodec.encode(&msg).ok()?;
    match record_bytes(&compact) {
        Ok(rebuilt) if rebuilt == data => Some(compact),
        _ => None,
    }
}

fn record_bytes(compact: &[u8]) -> Result<Vec<u8>, RadioError> {
    let msg = CompactCodec
        .decode(compact)
        .map_err(|error| RadioError::Malformed(error.to_string()))?;
    serde_json::to_value(&msg)
        .and_then(|value| serde_json::to_vec(&value))
        .map_err(|error| RadioError::Malformed(error.to_string()))
}

fn whole<T: BeBytes>(bytes: &[u8]) -> Result<T, RadioError> {
    let (wire, consumed) =
        T::try_from_be_bytes(bytes).map_err(|error| RadioError::Malformed(error.to_string()))?;
    match bytes.len() - consumed {
        0 => Ok(wire),
        extra => Err(RadioError::Trailing(extra)),
    }
}

fn at_least(bytes: &[u8], need: usize) -> Result<(), RadioError> {
    match bytes.len() >= need {
        true => Ok(()),
        false => Err(RadioError::Truncated {
            need,
            have: bytes.len(),
        }),
    }
}

fn short_len(value: &str, field: &'static str) -> Result<u8, RadioError> {
    u8::try_from(value.len()).map_err(|_| RadioError::TooLong(field))
}

fn op_code(op: Op) -> u8 {
    match op {
        Op::Insert => OP_INSERT,
        Op::Update => OP_UPDATE,
        Op::Delete => OP_DELETE,
    }
}

fn op_from(code: u8) -> Result<Op, RadioError> {
    match code {
        OP_INSERT => Ok(Op::Insert),
        OP_UPDATE => Ok(Op::Update),
        OP_DELETE => Ok(Op::Delete),
        other => Err(RadioError::Op(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_model::{Domain, FunctionalService, PositionReport};

    fn peer(n: u8) -> PeerId {
        [n; 32]
    }

    fn position() -> C2Message {
        C2Message::PositionReport(PositionReport {
            id: "POS-node-a".to_string(),
            reporting_unit: "node-a".to_string(),
            domain: Domain::Land,
            service: FunctionalService::CommandAndControl,
            reported_at: "2026-10-11T00:45:00Z".to_string(),
            latitude: 45.5017,
            longitude: -73.5673,
        })
    }

    fn frame(entity: &str, data: Vec<u8>) -> WriteFrame {
        WriteFrame {
            stamp: Stamp {
                hlc: Hlc {
                    physical: 1_791_671_100_000,
                    logical: 3,
                },
                peer: peer(7),
            },
            seq: 42,
            op: Op::Insert,
            entity: entity.to_string(),
            id: "POS-node-a".to_string(),
            data,
            signature: None,
        }
    }

    fn stored(msg: &C2Message) -> Vec<u8> {
        serde_json::to_vec(&serde_json::to_value(msg).expect("value")).expect("bytes")
    }

    #[test]
    fn c2_write_travels_compact_and_rebuilds_identical_bytes() {
        let original = frame(ENTITY, stored(&position()));
        let bytes = RadioMessage::Write(original.clone())
            .encode()
            .expect("encode");
        assert_eq!(bytes.len(), 129);
        assert_eq!(
            RadioMessage::decode(&bytes).expect("decode"),
            RadioMessage::Write(original)
        );
    }

    #[test]
    fn other_entities_travel_as_raw_bytes() {
        let original = frame("notes", b"{\"text\":\"hello\"}".to_vec());
        let bytes = RadioMessage::Write(original.clone())
            .encode()
            .expect("encode");
        assert_eq!(
            RadioMessage::decode(&bytes).expect("decode"),
            RadioMessage::Write(original)
        );
    }

    #[test]
    fn non_canonical_json_is_not_recompressed() {
        let original = frame(ENTITY, b"{ \"message_type\" : \"Sitrep\" }".to_vec());
        let bytes = RadioMessage::Write(original.clone())
            .encode()
            .expect("encode");
        assert_eq!(
            RadioMessage::decode(&bytes).expect("decode"),
            RadioMessage::Write(original)
        );
    }

    #[test]
    fn hello_round_trips_cursors() {
        let hello = RadioMessage::Hello {
            from: peer(1),
            cursors: [(peer(1), 12), (peer(2), 7)].into_iter().collect(),
        };
        let bytes = hello.encode().expect("encode");
        assert_eq!(bytes.len(), 1 + 32 + 1 + 2 * 40);
        assert_eq!(RadioMessage::decode(&bytes).expect("decode"), hello);
    }

    #[test]
    fn rejects_signed_and_malformed_input() {
        let mut signed = frame(ENTITY, stored(&position()));
        signed.signature = Some([0; 64]);
        assert_eq!(
            RadioMessage::Write(signed).encode(),
            Err(RadioError::Signed)
        );
        assert_eq!(RadioMessage::decode(&[]), Err(RadioError::Empty));
        assert_eq!(RadioMessage::decode(&[9]), Err(RadioError::Kind(9)));
        let write = RadioMessage::Write(frame(ENTITY, stored(&position())))
            .encode()
            .expect("encode");
        let hello = RadioMessage::Hello {
            from: peer(1),
            cursors: [(peer(1), 12), (peer(2), 7)].into_iter().collect(),
        }
        .encode()
        .expect("encode");
        for bytes in [&write, &hello] {
            for cut in 1..bytes.len() {
                assert!(RadioMessage::decode(&bytes[..cut]).is_err(), "cut at {cut}");
            }
        }
    }
}
