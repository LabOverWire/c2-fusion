use std::error::Error;
use std::process::ExitCode;
use std::time::Duration;

use c2_lora::{open_serial, Bandwidth, Config, Modulation, Radio, ResetLine};
use tokio::time::{sleep, timeout, Instant};
use tokio_serial::SerialStream;

const USAGE: &str = "usage: lora_link <serial-path> <reset-gpio> <address> <network> <power-dbm> <spreading-factor> (listen | echo | send <destination> <text> | burst <destination> <count> <gap-ms> | rtt <destination> <count> <size>)";
const BAND_HZ: u32 = 915_000_000;
const ECHO_WAIT: Duration = Duration::from_secs(30);

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> Result<()> {
    let [path, gpio, address, network, power, spreading_factor, mode @ ..] = args else {
        return Err(USAGE.into());
    };
    let config = Config {
        address: address.parse()?,
        network: network.parse()?,
        band_hz: BAND_HZ,
        power_dbm: power.parse()?,
        modulation: Modulation {
            spreading_factor: spreading_factor.parse()?,
            bandwidth: Bandwidth::Khz125,
            coding_rate: 1,
            preamble: 4,
        },
    };
    let reset = ResetLine::open(gpio.parse()?).await?;
    let mut radio = Radio::new(open_serial(path)?, Some(reset));

    let ready = radio.reset().await?;
    println!("RESET ready_ms={}", ready.as_millis());
    let changed = radio.configure(&config).await?;
    println!(
        "CONFIG address={} network={} power_dbm={} spreading_factor={} changed={changed}",
        config.address, config.network, config.power_dbm, config.modulation.spreading_factor
    );

    match mode {
        [command] if command == "listen" => listen(&mut radio, false).await,
        [command] if command == "echo" => listen(&mut radio, true).await,
        [command, destination, text] if command == "send" => {
            let airtime = radio.send(destination.parse()?, text.as_bytes()).await?;
            println!(
                "TX to={destination} len={} send_ms={}",
                text.len(),
                airtime.as_millis()
            );
            Ok(())
        }
        [command, destination, count, gap] if command == "burst" => {
            let gap = Duration::from_millis(gap.parse()?);
            burst(&mut radio, destination.parse()?, count.parse()?, gap).await
        }
        [command, destination, count, size] if command == "rtt" => {
            round_trips(
                &mut radio,
                destination.parse()?,
                count.parse()?,
                size.parse()?,
            )
            .await
        }
        _ => Err(USAGE.into()),
    }
}

async fn listen(radio: &mut Radio<SerialStream>, echo: bool) -> Result<()> {
    loop {
        let frame = radio.recv().await?;
        println!(
            "RX from={} len={} rssi_dbm={} snr={} payload={}",
            frame.source,
            frame.payload.len(),
            frame.rssi_dbm,
            frame.snr,
            frame.payload.escape_ascii()
        );
        if echo {
            let airtime = radio.send(frame.source, &frame.payload).await?;
            println!("ECHO to={} send_ms={}", frame.source, airtime.as_millis());
        }
    }
}

async fn burst(
    radio: &mut Radio<SerialStream>,
    destination: u16,
    count: u32,
    gap: Duration,
) -> Result<()> {
    for sequence in 0..count {
        let payload = format!("burst-{sequence:03}-padding-to-a-longer-frame-xxxxxxxxxxxxxxxx");
        let airtime = radio.send(destination, payload.as_bytes()).await?;
        println!(
            "TX seq={sequence} to={destination} len={} send_ms={}",
            payload.len(),
            airtime.as_millis()
        );
        sleep(gap).await;
    }
    Ok(())
}

async fn round_trips(
    radio: &mut Radio<SerialStream>,
    destination: u16,
    count: u32,
    size: usize,
) -> Result<()> {
    let mut answered = 0;
    for sequence in 0..count {
        let payload = format!("{:x<size$}", format!("rtt-{sequence:03}-"));
        let started = Instant::now();
        let airtime = radio.send(destination, payload.as_bytes()).await?;
        match timeout(ECHO_WAIT, radio.recv()).await {
            Ok(frame) => {
                let frame = frame?;
                let matched = frame.payload.as_slice() == payload.as_bytes();
                answered += u32::from(matched);
                println!(
                    "RTT seq={sequence} matched={matched} total_ms={} send_ms={} rssi_dbm={} snr={}",
                    started.elapsed().as_millis(),
                    airtime.as_millis(),
                    frame.rssi_dbm,
                    frame.snr
                );
            }
            Err(_) => println!("RTT seq={sequence} lost after_ms={}", ECHO_WAIT.as_millis()),
        }
    }
    println!(
        "SUMMARY sent={count} answered={answered} dropped_frames={}",
        radio.dropped_frames()
    );
    Ok(())
}
