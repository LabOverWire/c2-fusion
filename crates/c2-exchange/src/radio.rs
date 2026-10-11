use std::time::{Duration, Instant};

use bebytes::BeBytes;
use c2_codec::compact::CompactCodec;
use c2_codec::Codec;
use c2_model::C2Message;
use stitch_p2p::{Cursors, Hlc, Op, PeerId, Stamp, WriteFrame};

use crate::ENTITY;

const WRITE: u8 = 0;
const HELLO: u8 = 1;
const FRAGMENT: u8 = 2;

pub const MAX_FRAGMENTS: usize = 16;
const MAX_PARTIALS: usize = 4;
const PARTIAL_EXPIRY: Duration = Duration::from_secs(60);
const FRAGMENT_HEADER_LEN: usize = 1 + 2 + 1 + 1;

const BODY_JSON: u8 = 0;
const BODY_COMPACT: u8 = 1;

const OP_INSERT: u8 = 0;
const OP_UPDATE: u8 = 1;
const OP_DELETE: u8 = 2;

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
    #[error("a {0}-byte payload limit leaves no room for fragment data")]
    FragmentLimit(usize),
    #[error("message needs {0} fragments, more than {MAX_FRAGMENTS}")]
    TooManyFragments(usize),
    #[error("fragment {index} of {count} is invalid")]
    BadFragment { index: u8, count: u8 },
    #[error("hello declares {declared} cursors but carries {carried}")]
    CursorCount { declared: u8, carried: usize },
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
struct FragmentWire {
    id: u16,
    index: u8,
    count: u8,
    chunk: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Complete(RadioMessage),
    Partial { id: u16, have: usize, count: u8 },
}

struct Partial {
    source: u16,
    id: u16,
    count: u8,
    parts: [Option<Vec<u8>>; MAX_FRAGMENTS],
    started: Instant,
}

pub struct Reassembler {
    partials: [Option<Partial>; MAX_PARTIALS],
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

pub fn split(message: &[u8], limit: usize, id: u16) -> Result<Vec<Vec<u8>>, RadioError> {
    if message.len() <= limit {
        return Ok(vec![message.to_vec()]);
    }
    let chunk_len = limit
        .checked_sub(FRAGMENT_HEADER_LEN)
        .filter(|len| *len > 0)
        .ok_or(RadioError::FragmentLimit(limit))?;
    let needed = message.len().div_ceil(chunk_len);
    let count = u8::try_from(needed)
        .ok()
        .filter(|count| usize::from(*count) <= MAX_FRAGMENTS)
        .ok_or(RadioError::TooManyFragments(needed))?;
    Ok(message
        .chunks(chunk_len)
        .zip(0..count)
        .map(|(chunk, index)| {
            let mut bytes = vec![FRAGMENT];
            bytes.extend_from_slice(
                &FragmentWire {
                    id,
                    index,
                    count,
                    chunk: chunk.to_vec(),
                }
                .to_be_bytes(),
            );
            bytes
        })
        .collect())
}

impl Default for Reassembler {
    fn default() -> Self {
        Self {
            partials: std::array::from_fn(|_| None),
        }
    }
}

impl Reassembler {
    pub fn accept(
        &mut self,
        source: u16,
        payload: &[u8],
        now: Instant,
    ) -> Result<Received, RadioError> {
        match payload.split_first() {
            Some((&FRAGMENT, body)) => self.fragment(source, body, now),
            _ => RadioMessage::decode(payload).map(Received::Complete),
        }
    }

    fn fragment(&mut self, source: u16, body: &[u8], now: Instant) -> Result<Received, RadioError> {
        let wire: FragmentWire = whole(body)?;
        let valid = (2..=MAX_FRAGMENTS).contains(&usize::from(wire.count))
            && wire.index < wire.count
            && !wire.chunk.is_empty();
        if !valid {
            return Err(RadioError::BadFragment {
                index: wire.index,
                count: wire.count,
            });
        }
        for slot in &mut self.partials {
            if slot
                .as_ref()
                .is_some_and(|partial| now.duration_since(partial.started) > PARTIAL_EXPIRY)
            {
                *slot = None;
            }
        }
        let slot = self.slot_for(source, wire.id, wire.count);
        let partial = slot.get_or_insert_with(|| Partial {
            source,
            id: wire.id,
            count: wire.count,
            parts: std::array::from_fn(|_| None),
            started: now,
        });
        partial.parts[usize::from(wire.index)].get_or_insert(wire.chunk);
        let have = partial.parts.iter().flatten().count();
        if have < usize::from(partial.count) {
            return Ok(Received::Partial {
                id: wire.id,
                have,
                count: wire.count,
            });
        }
        let message: Vec<u8> = partial.parts.iter().flatten().flatten().copied().collect();
        *slot = None;
        RadioMessage::decode(&message).map(Received::Complete)
    }

    fn slot_for(&mut self, source: u16, id: u16, count: u8) -> &mut Option<Partial> {
        let matching = self.partials.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|partial| partial.source == source && partial.id == id)
        });
        let index = matching
            .or_else(|| self.partials.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                self.partials
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.as_ref().map(|partial| partial.started))
                    .map_or(0, |(index, _)| index)
            });
        let slot = &mut self.partials[index];
        let reusable = slot.as_ref().is_some_and(|partial| {
            partial.source == source && partial.id == id && partial.count == count
        });
        if !reusable {
            *slot = None;
        }
        slot
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
    let wire: HelloWire = whole(bytes)?;
    if wire.cursors.len() != usize::from(wire.count) {
        return Err(RadioError::CursorCount {
            declared: wire.count,
            carried: wire.cursors.len(),
        });
    }
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
    use c2_model::{ContactReport, Domain, FunctionalService, PositionReport};

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
    fn gnss_derived_coordinates_always_travel_compact() {
        for step in 0..2000u32 {
            let minutes = f64::from(step) * 0.000_137;
            let msg = C2Message::PositionReport(PositionReport {
                id: "POS-node-a".to_string(),
                reporting_unit: "node-a".to_string(),
                domain: Domain::Land,
                service: FunctionalService::CommandAndControl,
                reported_at: "2026-10-11T00:45:00Z".to_string(),
                latitude: 45.0 + (30.0 + minutes) / 60.0,
                longitude: -(73.0 + (34.0 + minutes) / 60.0),
            });
            let bytes = RadioMessage::Write(frame(ENTITY, stored(&msg)))
                .encode()
                .expect("encode");
            assert_eq!(bytes.len(), 129, "step {step}");
        }
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

    fn long_contact() -> C2Message {
        C2Message::ContactReport(ContactReport {
            id: "C-77".to_string(),
            reporting_unit: "UAV-7".to_string(),
            domain: Domain::Air,
            service: FunctionalService::Intelligence,
            observed_at: "2026-10-11T03:20:00Z".to_string(),
            latitude: 45.5017,
            longitude: -73.5673,
            description: "Convoy of six vehicles moving north along the river road. ".repeat(4),
        })
    }

    fn oversized_write() -> (RadioMessage, Vec<u8>) {
        let message = RadioMessage::Write(frame(ENTITY, stored(&long_contact())));
        let bytes = message.encode().expect("encode");
        (message, bytes)
    }

    const LIMIT: usize = 237;

    #[test]
    fn small_messages_are_not_split() {
        let bytes = RadioMessage::Write(frame(ENTITY, stored(&position())))
            .encode()
            .expect("encode");
        assert_eq!(split(&bytes, LIMIT, 1), Ok(vec![bytes.clone()]));
        assert_eq!(
            Reassembler::default().accept(2, &bytes, Instant::now()),
            Ok(Received::Complete(
                RadioMessage::decode(&bytes).expect("decode")
            ))
        );
    }

    #[test]
    fn oversized_write_reassembles_from_fragments_in_any_order() {
        let (message, bytes) = oversized_write();
        assert!(bytes.len() > LIMIT);
        let fragments = split(&bytes, LIMIT, 9).expect("split");
        assert_eq!(fragments.len(), 2);
        assert!(fragments.iter().all(|fragment| fragment.len() <= LIMIT));
        let now = Instant::now();
        let mut reassembler = Reassembler::default();
        assert_eq!(
            reassembler.accept(2, &fragments[1], now),
            Ok(Received::Partial {
                id: 9,
                have: 1,
                count: 2
            })
        );
        assert_eq!(
            reassembler.accept(2, &fragments[1], now),
            Ok(Received::Partial {
                id: 9,
                have: 1,
                count: 2
            })
        );
        assert_eq!(
            reassembler.accept(2, &fragments[0], now),
            Ok(Received::Complete(message))
        );
    }

    #[test]
    fn fragments_from_different_senders_reassemble_independently() {
        let (message, bytes) = oversized_write();
        let fragments = split(&bytes, LIMIT, 4).expect("split");
        let now = Instant::now();
        let mut reassembler = Reassembler::default();
        assert!(matches!(
            reassembler.accept(2, &fragments[0], now),
            Ok(Received::Partial { .. })
        ));
        assert!(matches!(
            reassembler.accept(3, &fragments[1], now),
            Ok(Received::Partial { .. })
        ));
        assert_eq!(
            reassembler.accept(2, &fragments[1], now),
            Ok(Received::Complete(message.clone()))
        );
        assert_eq!(
            reassembler.accept(3, &fragments[0], now),
            Ok(Received::Complete(message))
        );
    }

    #[test]
    fn incomplete_messages_expire() {
        let (_, bytes) = oversized_write();
        let fragments = split(&bytes, LIMIT, 5).expect("split");
        let start = Instant::now();
        let mut reassembler = Reassembler::default();
        assert!(matches!(
            reassembler.accept(2, &fragments[0], start),
            Ok(Received::Partial { have: 1, .. })
        ));
        assert!(matches!(
            reassembler.accept(
                2,
                &fragments[1],
                start + PARTIAL_EXPIRY + Duration::from_secs(1)
            ),
            Ok(Received::Partial { have: 1, .. })
        ));
    }

    #[test]
    fn rejects_invalid_fragments() {
        let (_, bytes) = oversized_write();
        assert_eq!(
            split(&bytes, FRAGMENT_HEADER_LEN, 1),
            Err(RadioError::FragmentLimit(FRAGMENT_HEADER_LEN))
        );
        assert!(matches!(
            split(&vec![0; 40 * MAX_FRAGMENTS], 40, 1),
            Err(RadioError::TooManyFragments(_))
        ));
        let now = Instant::now();
        let mut reassembler = Reassembler::default();
        for (index, count) in [(0, 1), (2, 2), (0, 17)] {
            let fragment = [FRAGMENT, 0, 1, index, count, 0xAA];
            assert_eq!(
                reassembler.accept(2, &fragment, now),
                Err(RadioError::BadFragment { index, count })
            );
        }
        let fragments = split(&bytes, LIMIT, 6).expect("split");
        for fragment in &fragments {
            for cut in 0..fragment.len() {
                let result = Reassembler::default().accept(2, &fragment[..cut], now);
                assert!(!matches!(result, Ok(Received::Complete(_))), "cut at {cut}");
            }
        }
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
