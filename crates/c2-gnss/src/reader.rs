use bytes::BytesMut;
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::codec::{Decoder, FramedRead};

use crate::nmea::{self, NmeaError, Sentence};

pub const MAX_LINE_BYTES: usize = 512;

const FIX_OUTPUT_REQUEST: &[u8] = b"GPGGA 1\r\nGPRMC 1\r\n";

#[derive(Debug, thiserror::Error)]
pub enum GnssError {
    #[error(transparent)]
    Nmea(#[from] NmeaError),
    #[error("line exceeded {MAX_LINE_BYTES} bytes")]
    LineTooLong,
    #[error("read failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot open serial port {path}: {source}")]
    Serial {
        path: String,
        source: tokio_serial::Error,
    },
    #[error("serial port closed")]
    Closed,
    #[error("no GGA or RMC for {0:?}")]
    Silent(std::time::Duration),
    #[cfg(target_os = "linux")]
    #[error("pps line: {0}")]
    Gpio(#[from] gpio_cdev::Error),
    #[cfg(target_os = "linux")]
    #[error("pps line closed")]
    PpsClosed,
    #[cfg(target_os = "linux")]
    #[error("system clock: {0}")]
    Clock(#[from] nix::errno::Errno),
}

enum Chunk {
    Line(BytesMut),
    Overflow,
}

#[derive(Default)]
struct BoundedLines {
    discarding: bool,
}

impl Decoder for BoundedLines {
    type Item = Chunk;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Chunk>, std::io::Error> {
        match src.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                let line = src.split_to(end + 1);
                let overflowed =
                    std::mem::take(&mut self.discarding) || line.len() > MAX_LINE_BYTES;
                Ok(Some(if overflowed {
                    Chunk::Overflow
                } else {
                    Chunk::Line(line)
                }))
            }
            None if src.len() > MAX_LINE_BYTES => {
                src.clear();
                self.discarding = true;
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Chunk>, std::io::Error> {
        match self.decode(src)? {
            Some(chunk) => Ok(Some(chunk)),
            None if std::mem::take(&mut self.discarding) => {
                src.clear();
                Ok(Some(Chunk::Overflow))
            }
            None if src.is_empty() => Ok(None),
            None => Ok(Some(Chunk::Line(src.split()))),
        }
    }
}

pub fn open_serial(path: &str, baud_rate: u32) -> Result<SerialStream, GnssError> {
    tokio_serial::new(path, baud_rate)
        .open_native_async()
        .map_err(|source| GnssError::Serial {
            path: path.to_string(),
            source,
        })
}

pub async fn request_fix_output<W: AsyncWrite + Unpin>(port: &mut W) -> Result<(), GnssError> {
    port.write_all(FIX_OUTPUT_REQUEST).await?;
    port.flush().await?;
    Ok(())
}

pub struct NmeaReader<R> {
    lines: FramedRead<R, BoundedLines>,
}

impl<R: AsyncRead + Unpin> NmeaReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            lines: FramedRead::new(reader, BoundedLines::default()),
        }
    }

    pub async fn next_sentence(&mut self) -> Option<Result<Sentence, GnssError>> {
        loop {
            let line = match self.lines.next().await? {
                Ok(Chunk::Line(line)) => line,
                Ok(Chunk::Overflow) => return Some(Err(GnssError::LineTooLong)),
                Err(error) => return Some(Err(error.into())),
            };
            if !line.starts_with(b"$") {
                continue;
            }
            match nmea::parse(&line) {
                Ok(Some(sentence)) => return Some(Ok(sentence)),
                Ok(None) => continue,
                Err(error) => return Some(Err(error.into())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmea::{checksum_of, FixQuality, Gga, Rmc};

    fn sentence(payload: &str) -> String {
        let sum = checksum_of(payload);
        format!("${payload}*{sum:02X}\r\n")
    }

    async fn next_gga<R: AsyncRead + Unpin>(reader: &mut NmeaReader<R>) -> Gga {
        match reader.next_sentence().await {
            Some(Ok(Sentence::Gga(gga))) => gga,
            other => panic!("expected GGA, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn requests_gga_and_rmc_output() {
        let mut written = Vec::new();
        request_fix_output(&mut written).await.expect("write");
        assert_eq!(written, b"GPGGA 1\r\nGPRMC 1\r\n");
    }

    #[tokio::test]
    async fn yields_fixes_and_skips_other_traffic() {
        let fix = sentence("GNGGA,120000.00,4530.0000,N,07330.0000,W,1,12,0.9,20.0,M,,M,,");
        let input = [
            "#VERSIONA,97,GPS,FINE;\"UM980\"*ef69137f\r\n".to_string(),
            "$command,GPGGA 1,response: OK*19\r\n".to_string(),
            "$GPRMC,,V,,,,,,,,,,N*53\r\n".to_string(),
            "\r\n".to_string(),
            fix,
            "$GPGGA,,,,,,0,00,99.99,,,,,,*48\r\n".to_string(),
        ]
        .concat();
        let mut reader = NmeaReader::new(input.as_bytes());

        assert!(matches!(
            reader.next_sentence().await,
            Some(Ok(Sentence::CommandReply { accepted: true }))
        ));
        assert!(matches!(
            reader.next_sentence().await,
            Some(Ok(Sentence::Rmc(Rmc { valid: false, .. })))
        ));
        let first = next_gga(&mut reader).await;
        assert_eq!(first.quality, FixQuality::Autonomous);
        assert_eq!(first.satellites, 12);
        assert_eq!(next_gga(&mut reader).await.quality, FixQuality::NoFix);
        assert!(reader.next_sentence().await.is_none());
    }

    #[test]
    fn discards_oversized_line_arriving_in_pieces() {
        let mut decoder = BoundedLines::default();
        let mut buffer = BytesMut::from("A".repeat(MAX_LINE_BYTES + 1).as_str());
        assert!(decoder.decode(&mut buffer).expect("decode").is_none());
        assert!(buffer.is_empty());

        buffer.extend_from_slice(b"tail of the long line\r\n$GPGGA,,,,,,0,00,99.99,,,,,,*48\r\n");
        assert!(matches!(
            decoder.decode(&mut buffer),
            Ok(Some(Chunk::Overflow))
        ));
        match decoder.decode(&mut buffer) {
            Ok(Some(Chunk::Line(line))) => assert!(line.starts_with(b"$GPGGA")),
            _ => panic!("expected the next line after the overflow"),
        }
        assert!(decoder.decode(&mut buffer).expect("decode").is_none());
    }

    #[test]
    fn keeps_final_line_without_newline() {
        let mut decoder = BoundedLines::default();
        let mut buffer = BytesMut::from("$GPGGA,,,,,,0,00,99.99,,,,,,*48");
        assert!(matches!(
            decoder.decode_eof(&mut buffer),
            Ok(Some(Chunk::Line(_)))
        ));
        assert!(decoder.decode_eof(&mut buffer).expect("decode").is_none());
    }

    #[tokio::test]
    async fn reports_errors_and_keeps_reading() {
        let oversized = format!("${}\r\n", "A".repeat(MAX_LINE_BYTES * 2));
        let input = [
            "$GPGGA,,,,,,0,00,99.99,,,,,,*49\r\n".to_string(),
            oversized,
            "$GPGGA,,,,,,0,00,99.99,,,,,,*48\r\n".to_string(),
        ]
        .concat();
        let mut reader = NmeaReader::new(input.as_bytes());

        assert!(matches!(
            reader.next_sentence().await,
            Some(Err(GnssError::Nmea(NmeaError::Checksum { .. })))
        ));
        assert!(matches!(
            reader.next_sentence().await,
            Some(Err(GnssError::LineTooLong))
        ));
        assert_eq!(next_gga(&mut reader).await.quality, FixQuality::NoFix);
        assert!(reader.next_sentence().await.is_none());
    }
}
