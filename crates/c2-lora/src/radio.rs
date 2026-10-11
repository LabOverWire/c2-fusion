use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::time::{sleep, timeout, Instant};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::codec::FramedRead;
use tracing::{debug, warn};

use crate::at::{AtCodec, Event, Frame, MAX_LINE, MAX_PAYLOAD};

pub const BAUD_RATE: u32 = 115_200;
pub const PENDING_FRAMES: usize = 16;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
const SEND_TIMEOUT: Duration = Duration::from_secs(12);
const READY_TIMEOUT: Duration = Duration::from_secs(3);
const RESET_PULSE: Duration = Duration::from_millis(100);
pub const TURNAROUND: Duration = Duration::from_millis(100);
pub const WAKE_AFTER: Duration = Duration::from_secs(3);
const WAKE_SETTLE: Duration = Duration::from_millis(200);
const MAX_REQUEST: usize = 20 + MAX_PAYLOAD;

#[derive(Debug, thiserror::Error)]
pub enum LoraError {
    #[error("radio I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot open serial port {path}: {source}")]
    Serial {
        path: String,
        source: tokio_serial::Error,
    },
    #[error("radio did not answer {0} in time")]
    Timeout(&'static str),
    #[error("radio rejected the command with +ERR={0}")]
    Rejected(u8),
    #[error("radio answered {got}, expected {expected}")]
    Unexpected { got: String, expected: &'static str },
    #[error("payload of {0} bytes is outside 1..={MAX_PAYLOAD}")]
    PayloadSize(usize),
    #[error("radio has no reset line")]
    NoResetLine,
    #[error("serial port closed")]
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    Khz125,
    Khz250,
    Khz500,
}

impl Bandwidth {
    fn code(self) -> u8 {
        match self {
            Bandwidth::Khz125 => 7,
            Bandwidth::Khz250 => 8,
            Bandwidth::Khz500 => 9,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modulation {
    pub spreading_factor: u8,
    pub bandwidth: Bandwidth,
    pub coding_rate: u8,
    pub preamble: u8,
}

impl Modulation {
    fn setting(self) -> String {
        format!(
            "{},{},{},{}",
            self.spreading_factor,
            self.bandwidth.code(),
            self.coding_rate,
            self.preamble
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub address: u16,
    pub network: u8,
    pub band_hz: u32,
    pub power_dbm: u8,
    pub modulation: Modulation,
}

enum Reply {
    Ok,
    Value(heapless::String<MAX_LINE>),
}

impl Reply {
    fn unexpected(self, expected: &'static str) -> LoraError {
        let got = match self {
            Reply::Ok => "+OK".to_string(),
            Reply::Value(value) => format!("+{value}"),
        };
        LoraError::Unexpected { got, expected }
    }
}

pub struct ResetLine {
    value: PathBuf,
}

impl ResetLine {
    pub async fn open(gpio: u32) -> Result<Self, LoraError> {
        let base = PathBuf::from(format!("/sys/class/gpio/gpio{gpio}"));
        if !tokio::fs::try_exists(&base).await? {
            tokio::fs::write("/sys/class/gpio/export", gpio.to_string()).await?;
        }
        tokio::fs::write(base.join("direction"), "high").await?;
        Ok(Self {
            value: base.join("value"),
        })
    }

    async fn pulse(&self) -> Result<(), std::io::Error> {
        tokio::fs::write(&self.value, "0").await?;
        sleep(RESET_PULSE).await;
        tokio::fs::write(&self.value, "1").await
    }
}

pub fn open_serial(path: &str) -> Result<SerialStream, LoraError> {
    tokio_serial::new(path, BAUD_RATE)
        .open_native_async()
        .map_err(|source| LoraError::Serial {
            path: path.to_string(),
            source,
        })
}

pub struct Radio<P> {
    port: FramedRead<P, AtCodec>,
    reset: Option<ResetLine>,
    pending: heapless::Deque<Frame, PENDING_FRAMES>,
    dropped: u64,
    last_heard: Option<Instant>,
    last_command: Option<Instant>,
    power_dbm: Option<u8>,
}

impl<P: AsyncRead + AsyncWrite + Unpin> Radio<P> {
    pub fn new(port: P, reset: Option<ResetLine>) -> Self {
        Self {
            port: FramedRead::new(port, AtCodec::default()),
            reset,
            pending: heapless::Deque::new(),
            dropped: 0,
            last_heard: None,
            last_command: None,
            power_dbm: None,
        }
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped
    }

    pub async fn ping(&mut self) -> Result<Duration, LoraError> {
        let started = Instant::now();
        match self.exchange(b"AT\r\n", COMMAND_TIMEOUT, "AT").await? {
            Reply::Ok => Ok(started.elapsed()),
            other => Err(other.unexpected("+OK")),
        }
    }

    pub async fn query(&mut self, setting: &str) -> Result<String, LoraError> {
        let request = format!("AT+{setting}?\r\n");
        let reply = self
            .exchange(request.as_bytes(), COMMAND_TIMEOUT, "a query")
            .await?;
        let current = match &reply {
            Reply::Value(value) => value
                .strip_prefix(setting)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_string),
            Reply::Ok => None,
        };
        current.ok_or_else(|| reply.unexpected("the queried setting"))
    }

    pub async fn configure(&mut self, config: &Config) -> Result<usize, LoraError> {
        let desired = [
            ("ADDRESS", config.address.to_string()),
            ("NETWORKID", config.network.to_string()),
            ("BAND", config.band_hz.to_string()),
            ("CRFOP", config.power_dbm.to_string()),
            ("PARAMETER", config.modulation.setting()),
        ];
        let mut changed = 0;
        for (setting, value) in desired {
            if self.query(setting).await? == value {
                continue;
            }
            let request = format!("AT+{setting}={value}\r\n");
            match self
                .exchange(request.as_bytes(), COMMAND_TIMEOUT, "a setting")
                .await?
            {
                Reply::Ok => changed += 1,
                other => return Err(other.unexpected("+OK")),
            }
        }
        self.power_dbm = Some(config.power_dbm);
        Ok(changed)
    }

    pub async fn refresh(&mut self) -> Result<Duration, LoraError> {
        let Some(power_dbm) = self.power_dbm else {
            return self.ping().await;
        };
        let started = Instant::now();
        let request = format!("AT+CRFOP={power_dbm}\r\n");
        match self
            .exchange(request.as_bytes(), COMMAND_TIMEOUT, "a refresh")
            .await?
        {
            Reply::Ok => Ok(started.elapsed()),
            other => Err(other.unexpected("+OK")),
        }
    }

    pub async fn send(&mut self, destination: u16, payload: &[u8]) -> Result<Duration, LoraError> {
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return Err(LoraError::PayloadSize(payload.len()));
        }
        let idle = self
            .last_command
            .is_none_or(|command| command.elapsed() >= WAKE_AFTER);
        if idle && self.power_dbm.is_some() {
            self.refresh().await?;
            sleep(WAKE_SETTLE).await;
        }
        if let Some(wait) = self
            .last_heard
            .and_then(|heard| TURNAROUND.checked_sub(heard.elapsed()))
        {
            sleep(wait).await;
        }
        let header = format!("AT+SEND={destination},{},", payload.len());
        let mut request = heapless::Vec::<u8, MAX_REQUEST>::new();
        request
            .extend_from_slice(header.as_bytes())
            .and_then(|()| request.extend_from_slice(payload))
            .and_then(|()| request.extend_from_slice(b"\r\n"))
            .map_err(|_| LoraError::PayloadSize(payload.len()))?;
        let started = Instant::now();
        match self.exchange(&request, SEND_TIMEOUT, "a send").await? {
            Reply::Ok => Ok(started.elapsed()),
            other => Err(other.unexpected("+OK")),
        }
    }

    pub async fn recv(&mut self) -> Result<Frame, LoraError> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(frame);
        }
        loop {
            match self.next_event().await? {
                Event::Received(frame) => return Ok(frame),
                other => self.note_unsolicited(other),
            }
        }
    }

    pub async fn reset(&mut self) -> Result<Duration, LoraError> {
        let started = Instant::now();
        match &self.reset {
            Some(line) => line.pulse().await?,
            None => return Err(LoraError::NoResetLine),
        }
        timeout(READY_TIMEOUT, self.await_ready())
            .await
            .map_err(|_| LoraError::Timeout("+READY"))??;
        Ok(started.elapsed())
    }

    async fn await_ready(&mut self) -> Result<(), LoraError> {
        loop {
            match self.next_event().await? {
                Event::Ready => return Ok(()),
                Event::Received(frame) => self.hold(frame),
                other => debug!(event = ?other, "ignored radio output while waiting for +READY"),
            }
        }
    }

    async fn exchange(
        &mut self,
        request: &[u8],
        limit: Duration,
        what: &'static str,
    ) -> Result<Reply, LoraError> {
        let port = self.port.get_mut();
        port.write_all(request).await?;
        port.flush().await?;
        let reply = timeout(limit, self.await_reply())
            .await
            .map_err(|_| LoraError::Timeout(what))?;
        self.last_command = Some(Instant::now());
        reply
    }

    async fn await_reply(&mut self) -> Result<Reply, LoraError> {
        loop {
            match self.next_event().await? {
                Event::Ok => return Ok(Reply::Ok),
                Event::Value(value) => return Ok(Reply::Value(value)),
                Event::Rejected(code) => return Err(LoraError::Rejected(code)),
                Event::Received(frame) => self.hold(frame),
                other => self.note_unsolicited(other),
            }
        }
    }

    async fn next_event(&mut self) -> Result<Event, LoraError> {
        let event = match self.port.next().await {
            Some(event) => event?,
            None => return Err(LoraError::Closed),
        };
        if matches!(event, Event::Received(_)) {
            self.last_heard = Some(Instant::now());
        }
        Ok(event)
    }

    fn hold(&mut self, frame: Frame) {
        if self.pending.is_full() {
            self.pending.pop_front();
            self.dropped += 1;
            warn!(
                dropped = self.dropped,
                "pending frame queue full, dropped the oldest frame"
            );
        }
        if self.pending.push_back(frame).is_err() {
            self.dropped += 1;
        }
    }

    fn note_unsolicited(&self, event: Event) {
        match event {
            Event::Ready => warn!("radio restarted on its own"),
            Event::Garbled(start) => warn!(
                start = %start.escape_ascii(),
                "discarded unparseable radio output"
            ),
            other => debug!(event = ?other, "ignored unsolicited radio output"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, DuplexStream};

    async fn expect_request(device: &mut DuplexStream, expected: &[u8]) {
        let mut request = vec![0; expected.len()];
        device.read_exact(&mut request).await.expect("request");
        assert_eq!(request, expected);
    }

    async fn reply(device: &mut DuplexStream, output: &[u8]) {
        device.write_all(output).await.expect("reply");
    }

    #[tokio::test]
    async fn configure_writes_only_changed_settings() {
        let (host, mut device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        let config = Config {
            address: 1,
            network: 6,
            band_hz: 915_000_000,
            power_dbm: 0,
            modulation: Modulation {
                spreading_factor: 9,
                bandwidth: Bandwidth::Khz125,
                coding_rate: 1,
                preamble: 4,
            },
        };
        let device_script = async {
            expect_request(&mut device, b"AT+ADDRESS?\r\n").await;
            reply(&mut device, b"+ADDRESS=1\r\n").await;
            expect_request(&mut device, b"AT+NETWORKID?\r\n").await;
            reply(&mut device, b"+NETWORKID=0\r\n").await;
            expect_request(&mut device, b"AT+NETWORKID=6\r\n").await;
            reply(&mut device, b"+OK\r\n").await;
            expect_request(&mut device, b"AT+BAND?\r\n").await;
            reply(&mut device, b"+BAND=915000000\r\n").await;
            expect_request(&mut device, b"AT+CRFOP?\r\n").await;
            reply(&mut device, b"+CRFOP=15\r\n").await;
            expect_request(&mut device, b"AT+CRFOP=0\r\n").await;
            reply(&mut device, b"+OK\r\n").await;
            expect_request(&mut device, b"AT+PARAMETER?\r\n").await;
            reply(&mut device, b"+PARAMETER=12,7,1,4\r\n").await;
            expect_request(&mut device, b"AT+PARAMETER=9,7,1,4\r\n").await;
            reply(&mut device, b"+OK\r\n").await;
        };
        let (changed, ()) = tokio::join!(radio.configure(&config), device_script);
        assert_eq!(changed.expect("configure"), 3);
    }

    #[tokio::test]
    async fn rejected_setting_is_an_error() {
        let (host, mut device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        let device_script = async {
            expect_request(&mut device, b"AT+ADDRESS?\r\n").await;
            reply(&mut device, b"+ERR=4\r\n").await;
        };
        let (result, ()) = tokio::join!(radio.query("ADDRESS"), device_script);
        assert!(matches!(result, Err(LoraError::Rejected(4))));
    }

    #[tokio::test]
    async fn frames_arriving_during_a_send_are_kept() {
        let (host, mut device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        let device_script = async {
            expect_request(&mut device, b"AT+SEND=2,4,a,\r\n\r\n").await;
            reply(&mut device, b"+RCV=2,2,hi,-40,42\r\n+OK\r\n").await;
        };
        let (sent, ()) = tokio::join!(radio.send(2, b"a,\r\n"), device_script);
        assert!(sent.is_ok());
        let frame = radio.recv().await.expect("held frame");
        assert_eq!(frame.source, 2);
        assert_eq!(frame.payload.as_slice(), b"hi");
        assert_eq!(frame.rssi_dbm, -40);
    }

    #[tokio::test]
    async fn payload_size_is_checked_before_sending() {
        let (host, _device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        assert!(matches!(
            radio.send(2, b"").await,
            Err(LoraError::PayloadSize(0))
        ));
        let oversized = [0u8; MAX_PAYLOAD + 1];
        assert!(matches!(
            radio.send(2, &oversized).await,
            Err(LoraError::PayloadSize(241))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn silent_radio_times_out() {
        let (host, _device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        assert!(matches!(radio.ping().await, Err(LoraError::Timeout("AT"))));
    }

    #[tokio::test(start_paused = true)]
    async fn send_waits_out_the_peer_turnaround() {
        let (host, mut device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        reply(&mut device, b"+RCV=2,2,hi,-40,42\r\n").await;
        radio.recv().await.expect("frame");
        let heard = Instant::now();
        let device_script = async {
            expect_request(&mut device, b"AT+SEND=2,2,ok\r\n").await;
            let waited = heard.elapsed();
            reply(&mut device, b"+OK\r\n").await;
            waited
        };
        let (sent, waited) = tokio::join!(radio.send(2, b"ok"), device_script);
        assert!(sent.is_ok());
        assert!(waited >= TURNAROUND);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_radio_is_woken_before_sending() {
        let (host, mut device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        radio.power_dbm = Some(0);
        radio.last_command = Some(Instant::now());
        let quick = async {
            expect_request(&mut device, b"AT+SEND=0,1,a\r\n").await;
            reply(&mut device, b"+OK\r\n").await;
        };
        let (sent, ()) = tokio::join!(radio.send(0, b"a"), quick);
        assert!(sent.is_ok());

        sleep(WAKE_AFTER).await;
        let after_idle = async {
            expect_request(&mut device, b"AT+CRFOP=0\r\n").await;
            reply(&mut device, b"+OK\r\n").await;
            let woke = Instant::now();
            expect_request(&mut device, b"AT+SEND=0,1,b\r\n").await;
            let settled = woke.elapsed();
            reply(&mut device, b"+OK\r\n").await;
            settled
        };
        let (sent, settled) = tokio::join!(radio.send(0, b"b"), after_idle);
        assert!(sent.is_ok());
        assert!(settled >= WAKE_SETTLE);
    }

    #[tokio::test]
    async fn reset_needs_a_reset_line() {
        let (host, _device) = duplex(1024);
        let mut radio = Radio::new(host, None);
        assert!(matches!(radio.reset().await, Err(LoraError::NoResetLine)));
    }
}
