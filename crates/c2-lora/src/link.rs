use std::time::{Duration, Instant};

use c2_frame::{Frame, FrameError, HEADER_LEN};

use crate::at::MAX_PAYLOAD;

pub const MAX_DATA: usize = MAX_PAYLOAD - HEADER_LEN;
pub const MAX_PEERS: usize = 8;
pub const MAX_IN_FLIGHT: usize = 1;
pub const BROADCAST: u16 = 0;

const RECENT: usize = 32;
const MAX_ATTEMPTS: u8 = 4;
const RETRY_BASE: Duration = Duration::from_secs(3);
const SEND_SPACING_MS: u64 = 800;
const RETRY_JITTER_MS: u64 = 2000;
const ACK_JITTER_MS: u64 = 300;
const PEER_EXPIRY: Duration = Duration::from_secs(300);

pub type Addresses = heapless::Vec<u16, MAX_PEERS>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub destination: u16,
    pub sequence: u16,
    pub bytes: Vec<u8>,
    pub not_before: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound<'a> {
    Deliver {
        sequence: u16,
        payload: &'a [u8],
        ack: Outgoing,
    },
    Duplicate {
        sequence: u16,
        ack: Outgoing,
    },
    Acked {
        sequence: u16,
        remaining: usize,
    },
    UnknownAck {
        sequence: u16,
    },
    Datagram {
        sequence: u16,
        payload: &'a [u8],
    },
    Invalid(FrameError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Due {
    Retry {
        sequence: u16,
        attempt: u8,
        outgoing: Outgoing,
    },
    GaveUp {
        sequence: u16,
        missing: Addresses,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("payload of {0} bytes is outside 1..={MAX_DATA}")]
    PayloadSize(usize),
    #[error("{MAX_IN_FLIGHT} messages are already awaiting acknowledgement")]
    Busy,
}

struct Peer {
    address: u16,
    last_heard: Instant,
}

struct InFlight {
    sequence: u16,
    frame: Vec<u8>,
    attempts: u8,
    due: Instant,
    awaiting: Addresses,
}

pub struct Link {
    next_sequence: u16,
    peers: heapless::Vec<Peer, MAX_PEERS>,
    in_flight: heapless::Vec<InFlight, MAX_IN_FLIGHT>,
    recent: heapless::Deque<(u16, u16), RECENT>,
    rng: u64,
}

impl Link {
    pub fn new(seed: u64) -> Self {
        let mut link = Self {
            next_sequence: 0,
            peers: heapless::Vec::new(),
            in_flight: heapless::Vec::new(),
            recent: heapless::Deque::new(),
            rng: seed | 1,
        };
        link.next_sequence = (link.random() & u64::from(u16::MAX)) as u16;
        link
    }

    pub fn has_capacity(&self) -> bool {
        !self.in_flight.is_full()
    }

    pub fn originate(&mut self, payload: &[u8], now: Instant) -> Result<Outgoing, LinkError> {
        if payload.is_empty() || payload.len() > MAX_DATA {
            return Err(LinkError::PayloadSize(payload.len()));
        }
        if !self.has_capacity() {
            return Err(LinkError::Busy);
        }
        let sequence = self.next_sequence;
        self.next_sequence = sequence.wrapping_add(1);
        let frame = Frame::Data { sequence, payload }.encode();
        let spacing = Duration::from_millis(self.random() % SEND_SPACING_MS);
        let awaiting = self.live_peers(now);
        let entry = InFlight {
            sequence,
            frame: frame.clone(),
            attempts: 1,
            due: now + spacing + self.backoff(1),
            awaiting,
        };
        if self.in_flight.push(entry).is_err() {
            return Err(LinkError::Busy);
        }
        Ok(Outgoing {
            destination: BROADCAST,
            sequence,
            bytes: frame,
            not_before: now + spacing,
        })
    }

    pub fn datagram(&mut self, payload: &[u8], now: Instant) -> Result<Outgoing, LinkError> {
        if payload.is_empty() || payload.len() > MAX_DATA {
            return Err(LinkError::PayloadSize(payload.len()));
        }
        let sequence = self.next_sequence;
        self.next_sequence = sequence.wrapping_add(1);
        Ok(Outgoing {
            destination: BROADCAST,
            sequence,
            bytes: Frame::Datagram { sequence, payload }.encode(),
            not_before: now,
        })
    }

    pub fn receive<'a>(&mut self, source: u16, bytes: &'a [u8], now: Instant) -> Inbound<'a> {
        let frame = match Frame::decode(bytes) {
            Ok(frame) => frame,
            Err(error) => return Inbound::Invalid(error),
        };
        self.note_peer(source, now);
        match frame {
            Frame::Data { sequence, payload } => {
                let ack = Outgoing {
                    destination: source,
                    sequence,
                    bytes: Frame::Ack { sequence }.encode(),
                    not_before: now + Duration::from_millis(self.random() % ACK_JITTER_MS),
                };
                match self.recent.iter().any(|seen| *seen == (source, sequence)) {
                    true => Inbound::Duplicate { sequence, ack },
                    false => {
                        if self.recent.is_full() {
                            self.recent.pop_front();
                        }
                        if self.recent.push_back((source, sequence)).is_err() {
                            self.recent.clear();
                        }
                        Inbound::Deliver {
                            sequence,
                            payload,
                            ack,
                        }
                    }
                }
            }
            Frame::Ack { sequence } => self.acknowledge(source, sequence),
            Frame::Datagram { sequence, payload } => Inbound::Datagram { sequence, payload },
        }
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.in_flight.iter().map(|entry| entry.due).min()
    }

    pub fn poll(&mut self, now: Instant) -> Option<Due> {
        let index = self
            .in_flight
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.due <= now)
            .min_by_key(|(_, entry)| entry.due)
            .map(|(index, _)| index)?;
        if self.in_flight[index].attempts >= MAX_ATTEMPTS {
            let entry = self.in_flight.swap_remove(index);
            return Some(Due::GaveUp {
                sequence: entry.sequence,
                missing: entry.awaiting,
            });
        }
        let attempt = self.in_flight[index].attempts + 1;
        let due = now + self.backoff(attempt);
        let entry = &mut self.in_flight[index];
        entry.attempts = attempt;
        entry.due = due;
        Some(Due::Retry {
            sequence: entry.sequence,
            attempt,
            outgoing: Outgoing {
                destination: BROADCAST,
                sequence: entry.sequence,
                bytes: entry.frame.clone(),
                not_before: now,
            },
        })
    }

    fn acknowledge(&mut self, source: u16, sequence: u16) -> Inbound<'static> {
        let Some(index) = self
            .in_flight
            .iter()
            .position(|entry| entry.sequence == sequence)
        else {
            return Inbound::UnknownAck { sequence };
        };
        let entry = &mut self.in_flight[index];
        entry.awaiting.retain(|address| *address != source);
        let remaining = entry.awaiting.len();
        if remaining == 0 {
            self.in_flight.swap_remove(index);
        }
        Inbound::Acked {
            sequence,
            remaining,
        }
    }

    fn note_peer(&mut self, address: u16, now: Instant) {
        if let Some(peer) = self.peers.iter_mut().find(|peer| peer.address == address) {
            peer.last_heard = now;
            return;
        }
        if self.peers.is_full() {
            if let Some(oldest) = self
                .peers
                .iter()
                .enumerate()
                .min_by_key(|(_, peer)| peer.last_heard)
                .map(|(index, _)| index)
            {
                self.peers.swap_remove(oldest);
            }
        }
        if self
            .peers
            .push(Peer {
                address,
                last_heard: now,
            })
            .is_err()
        {
            self.peers.clear();
        }
    }

    fn live_peers(&self, now: Instant) -> Addresses {
        self.peers
            .iter()
            .filter(|peer| now.duration_since(peer.last_heard) < PEER_EXPIRY)
            .map(|peer| peer.address)
            .collect()
    }

    fn backoff(&mut self, attempt: u8) -> Duration {
        RETRY_BASE * (1u32 << attempt.saturating_sub(1).min(7))
            + Duration::from_millis(self.random() % RETRY_JITTER_MS)
    }

    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_from(sequence: u16, payload: &[u8]) -> Vec<u8> {
        Frame::Data { sequence, payload }.encode()
    }

    fn ack_for(sequence: u16) -> Vec<u8> {
        Frame::Ack { sequence }.encode()
    }

    fn sequence_of(outgoing: &Outgoing) -> u16 {
        let encoded = Frame::decode(&outgoing.bytes)
            .expect("link emits valid frames")
            .sequence();
        assert_eq!(encoded, outgoing.sequence);
        encoded
    }

    #[test]
    fn delivery_completes_when_every_known_peer_acks() {
        let start = Instant::now();
        let mut link = Link::new(7);
        link.receive(2, &data_from(40, b"hello"), start);
        link.receive(3, &data_from(90, b"hello"), start);

        let sent = link.originate(b"report", start).expect("originate");
        assert_eq!(sent.destination, BROADCAST);
        let sequence = sequence_of(&sent);

        assert_eq!(
            link.receive(2, &ack_for(sequence), start),
            Inbound::Acked {
                sequence,
                remaining: 1
            }
        );
        assert_eq!(
            link.receive(3, &ack_for(sequence), start),
            Inbound::Acked {
                sequence,
                remaining: 0
            }
        );
        assert_eq!(link.next_due(), None);
    }

    #[test]
    fn unacknowledged_frame_is_retried_then_abandoned() {
        let start = Instant::now();
        let mut link = Link::new(11);
        link.receive(2, &data_from(1, b"x"), start);
        let sent = link.originate(b"report", start).expect("originate");
        let sequence = sequence_of(&sent);

        let mut now;
        let mut retries = 0;
        loop {
            now = link.next_due().expect("pending retry");
            match link.poll(now) {
                Some(Due::Retry {
                    outgoing, attempt, ..
                }) => {
                    retries += 1;
                    assert_eq!(outgoing.bytes, sent.bytes);
                    assert_eq!(attempt, retries + 1);
                }
                Some(Due::GaveUp {
                    sequence: abandoned,
                    missing,
                }) => {
                    assert_eq!(abandoned, sequence);
                    assert_eq!(missing.as_slice(), &[2]);
                    break;
                }
                None => panic!("due entry was not returned"),
            }
        }
        assert_eq!(retries, MAX_ATTEMPTS - 1);
        assert!(now >= start + RETRY_BASE * 15);
        assert_eq!(link.next_due(), None);
    }

    #[test]
    fn without_known_peers_a_frame_still_gets_every_attempt() {
        let start = Instant::now();
        let mut link = Link::new(3);
        link.originate(b"report", start).expect("originate");
        for attempt in 2..=MAX_ATTEMPTS {
            let due = link.next_due().expect("retry scheduled");
            assert!(matches!(link.poll(due), Some(Due::Retry { attempt: a, .. }) if a == attempt));
        }
        let due = link.next_due().expect("give-up scheduled");
        assert!(matches!(link.poll(due), Some(Due::GaveUp { missing, .. }) if missing.is_empty()));
    }

    #[test]
    fn any_ack_completes_a_frame_sent_without_known_peers() {
        let start = Instant::now();
        let mut link = Link::new(19);
        let sent = link.originate(b"report", start).expect("originate");
        let sequence = sequence_of(&sent);
        assert_eq!(
            link.receive(5, &ack_for(sequence), start),
            Inbound::Acked {
                sequence,
                remaining: 0
            }
        );
        assert_eq!(link.next_due(), None);
    }

    #[test]
    fn duplicate_data_is_acked_but_not_delivered_twice() {
        let start = Instant::now();
        let mut link = Link::new(5);
        let frame = data_from(9, b"payload");
        let Inbound::Deliver { payload, ack, .. } = link.receive(4, &frame, start) else {
            panic!("first copy should be delivered");
        };
        assert_eq!(payload, b"payload");
        assert_eq!(ack.destination, 4);
        assert_eq!(ack.bytes, ack_for(9));
        assert!(ack.not_before < start + Duration::from_millis(ACK_JITTER_MS));
        assert!(matches!(
            link.receive(4, &frame, start),
            Inbound::Duplicate { sequence: 9, .. }
        ));
        assert!(matches!(
            link.receive(6, &frame, start),
            Inbound::Deliver { .. }
        ));
    }

    #[test]
    fn in_flight_limit_applies_back_pressure() {
        let start = Instant::now();
        let mut link = Link::new(13);
        for _ in 0..MAX_IN_FLIGHT {
            link.originate(b"m", start).expect("capacity");
        }
        assert!(!link.has_capacity());
        assert_eq!(link.originate(b"m", start), Err(LinkError::Busy));
    }

    #[test]
    fn rejects_bad_payloads_and_frames() {
        let start = Instant::now();
        let mut link = Link::new(17);
        assert_eq!(link.originate(b"", start), Err(LinkError::PayloadSize(0)));
        assert_eq!(
            link.originate(&[0; MAX_DATA + 1], start),
            Err(LinkError::PayloadSize(MAX_DATA + 1))
        );
        assert!(matches!(
            link.receive(2, b"\x20\x00\x01x", start),
            Inbound::Invalid(FrameError::Version(2))
        ));
        assert!(matches!(
            link.receive(2, &ack_for(500), start),
            Inbound::UnknownAck { sequence: 500 }
        ));
    }

    #[test]
    fn datagrams_are_neither_tracked_nor_acknowledged() {
        let start = Instant::now();
        let mut link = Link::new(5);
        link.receive(2, &data_from(1, b"x"), start);
        let sent = link.originate(b"a", start).expect("originate");
        assert!(!link.has_capacity());
        let due_before = link.next_due();

        let datagram = link.datagram(b"hello", start).expect("datagram");
        assert_eq!(datagram.destination, BROADCAST);
        assert_eq!(sequence_of(&datagram), sent.sequence.wrapping_add(1));
        assert_eq!(link.next_due(), due_before);
        assert!(!link.has_capacity());

        let incoming = Frame::Datagram {
            sequence: 77,
            payload: b"hi",
        }
        .encode();
        assert_eq!(
            link.receive(3, &incoming, start),
            Inbound::Datagram {
                sequence: 77,
                payload: b"hi",
            }
        );
        assert_eq!(link.datagram(&[], start), Err(LinkError::PayloadSize(0)));
    }

    #[test]
    fn sequences_start_at_a_seeded_random_point() {
        let start = Instant::now();
        let first = sequence_of(&Link::new(1).originate(b"m", start).expect("originate"));
        let second = sequence_of(&Link::new(2).originate(b"m", start).expect("originate"));
        assert_ne!(first, second);
    }
}
