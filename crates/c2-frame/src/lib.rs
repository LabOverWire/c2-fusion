#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

use bebytes::BeBytes;

pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 3;

const DATA: u8 = 0;
const ACK: u8 = 1;
const DATAGRAM: u8 = 2;

#[derive(BeBytes, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    #[bits(4)]
    pub version: u8,
    #[bits(4)]
    pub kind: u8,
    pub sequence: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame<'a> {
    Data { sequence: u16, payload: &'a [u8] },
    Ack { sequence: u16 },
    Datagram { sequence: u16, payload: &'a [u8] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    Header,
    Version(u8),
    Kind(u8),
    EmptyData,
    TrailingAck(usize),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Header => write!(f, "frame header is truncated or invalid"),
            FrameError::Version(version) => write!(f, "unsupported frame version {version}"),
            FrameError::Kind(kind) => write!(f, "unknown frame kind {kind}"),
            FrameError::EmptyData => write!(f, "data frame has no payload"),
            FrameError::TrailingAck(extra) => write!(f, "ack frame has {extra} trailing bytes"),
        }
    }
}

impl core::error::Error for FrameError {}

impl<'a> Frame<'a> {
    pub fn sequence(&self) -> u16 {
        match self {
            Frame::Data { sequence, .. }
            | Frame::Ack { sequence }
            | Frame::Datagram { sequence, .. } => *sequence,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Frame::Data { payload, .. } => (DATA, *payload),
            Frame::Ack { .. } => (ACK, &[][..]),
            Frame::Datagram { payload, .. } => (DATAGRAM, *payload),
        };
        let header = Header {
            version: VERSION,
            kind,
            sequence: self.sequence(),
        };
        let mut bytes = header.to_be_bytes();
        bytes.extend_from_slice(payload);
        bytes
    }

    pub fn decode(bytes: &'a [u8]) -> Result<Self, FrameError> {
        let (header, consumed) =
            Header::try_from_be_bytes(bytes).map_err(|_| FrameError::Header)?;
        if header.version != VERSION {
            return Err(FrameError::Version(header.version));
        }
        let rest = &bytes[consumed..];
        match (header.kind, rest.len()) {
            (DATA, 0) => Err(FrameError::EmptyData),
            (DATA, _) => Ok(Frame::Data {
                sequence: header.sequence,
                payload: rest,
            }),
            (ACK, 0) => Ok(Frame::Ack {
                sequence: header.sequence,
            }),
            (ACK, extra) => Err(FrameError::TrailingAck(extra)),
            (DATAGRAM, 0) => Err(FrameError::EmptyData),
            (DATAGRAM, _) => Ok(Frame::Datagram {
                sequence: header.sequence,
                payload: rest,
            }),
            (kind, _) => Err(FrameError::Kind(kind)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_packs_version_and_kind_into_one_byte() {
        let bytes = Frame::Ack { sequence: 0x0102 }.encode();
        assert_eq!(bytes, [0x11, 0x01, 0x02]);
        assert_eq!(bytes.len(), HEADER_LEN);
        let data = Frame::Data {
            sequence: 7,
            payload: b"xy",
        }
        .encode();
        assert_eq!(data, [0x10, 0x00, 0x07, b'x', b'y']);
    }

    #[test]
    fn frames_round_trip() {
        let payload = [0u8, 1, 2, 0xff, b'\r', b'\n'];
        for frame in [
            Frame::Data {
                sequence: 65535,
                payload: &payload,
            },
            Frame::Ack { sequence: 0 },
            Frame::Datagram {
                sequence: 9,
                payload: &payload,
            },
        ] {
            assert_eq!(Frame::decode(&frame.encode()), Ok(frame));
        }
    }

    #[test]
    fn rejects_malformed_frames() {
        assert_eq!(Frame::decode(&[]), Err(FrameError::Header));
        assert_eq!(Frame::decode(&[0x10]), Err(FrameError::Header));
        assert_eq!(Frame::decode(&[0x10, 0x00]), Err(FrameError::Header));
        assert_eq!(
            Frame::decode(&[0x20, 0x00, 0x01, 1]),
            Err(FrameError::Version(2))
        );
        assert_eq!(
            Frame::decode(&[0x10, 0x00, 0x01]),
            Err(FrameError::EmptyData)
        );
        assert_eq!(
            Frame::decode(&[0x11, 0x00, 0x01, 9]),
            Err(FrameError::TrailingAck(1))
        );
        assert_eq!(Frame::decode(&[0x15, 0x00, 0x01]), Err(FrameError::Kind(5)));
        assert_eq!(
            Frame::decode(&[0x12, 0x00, 0x01]),
            Err(FrameError::EmptyData)
        );
    }
}
