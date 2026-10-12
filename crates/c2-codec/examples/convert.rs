use std::error::Error;
use std::io::{Read, Write};

use c2_codec::Registry;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [from, to] = args.as_slice() else {
        return Err("usage: convert <from-format> <to-format> < message".into());
    };
    let registry = Registry::with_defaults();
    let source = registry.get(from).ok_or_else(|| {
        format!(
            "unknown format {from}, expected one of {:?}",
            registry.wire_formats()
        )
    })?;
    let target = registry.get(to).ok_or_else(|| {
        format!(
            "unknown format {to}, expected one of {:?}",
            registry.wire_formats()
        )
    })?;
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let encoded = target.encode(&source.decode(&input)?)?;
    let mut stdout = std::io::stdout();
    match String::from_utf8(encoded) {
        Ok(text) => writeln!(stdout, "{}", text.trim_end())?,
        Err(binary) => {
            let bytes = binary.into_bytes();
            let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            writeln!(stdout, "{} bytes: {}", bytes.len(), hex.join(" "))?;
        }
    }
    Ok(())
}
