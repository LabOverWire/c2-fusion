use std::process::ExitCode;

use c2_gnss::{open_serial, request_fix_output, Gga, NmeaReader, Rmc, Sentence};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, baud] = args.as_slice() else {
        eprintln!("usage: gnss_tail <serial-path> <baud>");
        return ExitCode::FAILURE;
    };
    let Ok(baud) = baud.parse::<u32>() else {
        eprintln!("invalid baud rate: {baud}");
        return ExitCode::FAILURE;
    };
    let mut port = match open_serial(path, baud) {
        Ok(port) => port,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = request_fix_output(&mut port).await {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    let mut reader = NmeaReader::new(port);
    while let Some(next) = reader.next_sentence().await {
        match next {
            Ok(Sentence::Gga(gga)) => report_gga(&gga),
            Ok(Sentence::Rmc(Rmc { valid, unix_nanos })) => {
                println!("TIME valid={valid} unix_nanos={unix_nanos:?}")
            }
            Ok(Sentence::CommandReply { accepted }) => println!("REPLY accepted={accepted}"),
            Err(error) => eprintln!("SKIP error={error}"),
        }
    }
    ExitCode::SUCCESS
}

fn report_gga(gga: &Gga) {
    let Gga {
        utc_seconds_of_day,
        quality,
        satellites,
        hdop,
        position,
    } = gga;
    match position {
        Some(position) => {
            let latitude = position.latitude;
            let longitude = position.longitude;
            let altitude = position.altitude_msl_m;
            println!(
                "FIX utc_s={utc_seconds_of_day:?} quality={quality:?} sats={satellites} hdop={hdop:?} lat={latitude:.8} lon={longitude:.8} alt_msl_m={altitude:?}"
            );
        }
        None => println!("NOFIX utc_s={utc_seconds_of_day:?} quality={quality:?} sats={satellites} hdop={hdop:?}"),
    }
}
