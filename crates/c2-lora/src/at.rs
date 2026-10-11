use bytes::{Buf, BytesMut};
use tokio_util::codec::Decoder;

pub const MAX_PAYLOAD: usize = 240;
pub const MAX_LINE: usize = 64;
pub const SNIPPET: usize = 48;

pub type Snippet = heapless::Vec<u8, SNIPPET>;

const RECEIVE_PREFIX: &[u8] = b"+RCV=";
const MAX_HEADER: usize = 10;
const MAX_TRAILER: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub source: u16,
    pub payload: heapless::Vec<u8, MAX_PAYLOAD>,
    pub rssi_dbm: i16,
    pub snr: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Ok,
    Rejected(u8),
    Ready,
    Value(heapless::String<MAX_LINE>),
    Received(Frame),
    Garbled(Snippet),
}

#[derive(Default)]
pub struct AtCodec {
    discarded: Option<Snippet>,
}

struct InvalidFrame;

impl Decoder for AtCodec {
    type Item = Event;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Event>, std::io::Error> {
        let filler = src
            .iter()
            .take_while(|byte| matches!(byte, 0 | b'\r' | b'\n'))
            .count();
        src.advance(filler);
        if src.is_empty() {
            return Ok(None);
        }
        if self.discarded.is_none() && src.starts_with(RECEIVE_PREFIX) {
            return Ok(self.decode_frame(src));
        }
        if self.discarded.is_none() && RECEIVE_PREFIX.starts_with(src) {
            return Ok(None);
        }
        Ok(self.decode_line(src))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Event>, std::io::Error> {
        match self.decode(src)? {
            Some(event) => Ok(Some(event)),
            None => {
                let leftover = self
                    .discarded
                    .take()
                    .or_else(|| (!src.is_empty()).then(|| snippet(src)));
                src.clear();
                Ok(leftover.map(Event::Garbled))
            }
        }
    }
}

impl AtCodec {
    fn decode_line(&mut self, src: &mut BytesMut) -> Option<Event> {
        match src.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                let line = src.split_to(end + 1);
                match (self.discarded.take(), line.len() > MAX_LINE) {
                    (Some(start), _) => Some(Event::Garbled(start)),
                    (None, true) => Some(Event::Garbled(snippet(&line))),
                    (None, false) => Some(parse_line(trim_line(&line))),
                }
            }
            None if src.len() > MAX_LINE => {
                if self.discarded.is_none() {
                    self.discarded = Some(snippet(src));
                }
                src.clear();
                None
            }
            None => None,
        }
    }

    fn decode_frame(&mut self, src: &mut BytesMut) -> Option<Event> {
        match parse_frame(src) {
            Ok(Some((frame, consumed))) => {
                src.advance(consumed);
                Some(Event::Received(frame))
            }
            Ok(None) => None,
            Err(InvalidFrame) => {
                self.discarded = Some(snippet(src));
                src.advance(RECEIVE_PREFIX.len());
                self.decode_line(src)
            }
        }
    }
}

fn snippet(bytes: &[u8]) -> Snippet {
    bytes.iter().take(SNIPPET).copied().collect()
}

fn trim_line(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|byte| !matches!(byte, b'\r' | b'\n'))
        .map_or(0, |last| last + 1);
    &line[..end]
}

fn parse_line(line: &[u8]) -> Event {
    match line {
        b"+OK" => Event::Ok,
        b"+READY" => Event::Ready,
        _ => match (line.strip_prefix(b"+ERR="), line.strip_prefix(b"+")) {
            (Some(code), _) => {
                parse_number(code).map_or_else(|| Event::Garbled(snippet(line)), Event::Rejected)
            }
            (None, Some(value)) => std::str::from_utf8(value)
                .ok()
                .and_then(|text| heapless::String::try_from(text).ok())
                .map_or_else(|| Event::Garbled(snippet(line)), Event::Value),
            (None, None) => Event::Garbled(snippet(line)),
        },
    }
}

fn parse_number<T: std::str::FromStr>(digits: &[u8]) -> Option<T> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

fn parse_frame(src: &[u8]) -> Result<Option<(Frame, usize)>, InvalidFrame> {
    let body = &src[RECEIVE_PREFIX.len()..];
    let header_len = body.len().min(MAX_HEADER);
    let mut commas = body[..header_len]
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b',')
        .map(|(index, _)| index);
    let (first, second) = match (commas.next(), commas.next()) {
        (Some(first), Some(second)) => (first, second),
        _ if body.len() < MAX_HEADER => return Ok(None),
        _ => return Err(InvalidFrame),
    };
    let (Some(source), Some(length)) = (
        parse_number::<u16>(&body[..first]),
        parse_number::<usize>(&body[first + 1..second]),
    ) else {
        return Err(InvalidFrame);
    };
    if length == 0 || length > MAX_PAYLOAD {
        return Err(InvalidFrame);
    }
    let payload_start = second + 1;
    let payload_end = payload_start + length;
    if body.len() < payload_end {
        return Ok(None);
    }
    let trailer = &body[payload_end..];
    let Some(line_end) = trailer.iter().position(|byte| *byte == b'\n') else {
        return match trailer.len() < MAX_TRAILER {
            true => Ok(None),
            false => Err(InvalidFrame),
        };
    };
    let Some(signal) = trim_line(&trailer[..line_end]).strip_prefix(b",") else {
        return Err(InvalidFrame);
    };
    let Some(comma) = signal.iter().position(|byte| *byte == b',') else {
        return Err(InvalidFrame);
    };
    let (Some(rssi_dbm), Some(snr), Ok(payload)) = (
        parse_number(&signal[..comma]),
        parse_number(&signal[comma + 1..]),
        heapless::Vec::from_slice(&body[payload_start..payload_end]),
    ) else {
        return Err(InvalidFrame);
    };
    let frame = Frame {
        source,
        payload,
        rssi_dbm,
        snr,
    };
    Ok(Some((
        frame,
        RECEIVE_PREFIX.len() + payload_end + line_end + 1,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(input: &[u8]) -> Vec<Event> {
        let mut codec = AtCodec::default();
        let mut buffer = BytesMut::from(input);
        let mut events = Vec::new();
        while let Ok(Some(event)) = codec.decode_eof(&mut buffer) {
            events.push(event);
        }
        events
    }

    fn value(text: &str) -> Event {
        Event::Value(heapless::String::try_from(text).expect("short value"))
    }

    fn garbled(bytes: &[u8]) -> Event {
        Event::Garbled(snippet(bytes))
    }

    fn frame(source: u16, payload: &[u8], rssi_dbm: i16, snr: i16) -> Event {
        Event::Received(Frame {
            source,
            payload: heapless::Vec::from_slice(payload).expect("short payload"),
            rssi_dbm,
            snr,
        })
    }

    #[test]
    fn parses_replies() {
        assert_eq!(
            decode_all(b"+OK\r\n+ERR=4\r\n+ADDRESS=1\r\n+VER=RYLR89C_V1.2.7\r\n"),
            vec![
                Event::Ok,
                Event::Rejected(4),
                value("ADDRESS=1"),
                value("VER=RYLR89C_V1.2.7"),
            ]
        );
    }

    #[test]
    fn skips_nul_before_ready() {
        assert_eq!(decode_all(b"\0+READY\r\n"), vec![Event::Ready]);
    }

    #[test]
    fn frame_payload_is_delimited_by_length() {
        assert_eq!(
            decode_all(b"+RCV=1,8,a,b\r\n\0\xffz,-44,41\r\n+OK\r\n"),
            vec![frame(1, b"a,b\r\n\0\xffz", -44, 41), Event::Ok]
        );
    }

    #[test]
    fn frame_arriving_in_pieces() {
        let mut codec = AtCodec::default();
        let mut buffer = BytesMut::new();
        for piece in [&b"+RC"[..], b"V=2,11,hello", b"-lora1,-40", b",42\r\n"] {
            assert_eq!(codec.decode(&mut buffer).expect("decode"), None);
            buffer.extend_from_slice(piece);
        }
        assert_eq!(
            codec.decode(&mut buffer).expect("decode"),
            Some(frame(2, b"hello-lora1", -40, 42))
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn malformed_frame_is_garbled_and_decoding_continues() {
        assert_eq!(
            decode_all(b"+RCV=1,999,xyz\r\n+OK\r\n"),
            vec![garbled(b"+RCV=1,999,xyz\r\n+OK\r\n"), Event::Ok]
        );
        assert_eq!(
            decode_all(b"+RCV=1,3,abc;-40;42\r\n+OK\r\n"),
            vec![garbled(b"+RCV=1,3,abc;-40;42\r\n+OK\r\n"), Event::Ok]
        );
    }

    #[test]
    fn overlong_line_is_garbled_and_decoding_continues() {
        let mut input = vec![b'x'; MAX_LINE * 2];
        input.extend_from_slice(b"\r\n+OK\r\n");
        let mut codec = AtCodec::default();
        let mut buffer = BytesMut::from(&input[..MAX_LINE + 1]);
        assert_eq!(codec.decode(&mut buffer).expect("decode"), None);
        buffer.extend_from_slice(&input[MAX_LINE + 1..]);
        assert_eq!(
            codec.decode(&mut buffer).expect("decode"),
            Some(garbled(&input[..SNIPPET]))
        );
        assert_eq!(codec.decode(&mut buffer).expect("decode"), Some(Event::Ok));
    }

    #[test]
    fn unknown_output_is_garbled() {
        assert_eq!(
            decode_all(b"hello\r\n+ERR=x\r\n"),
            vec![garbled(b"hello"), garbled(b"+ERR=x")]
        );
    }
}
