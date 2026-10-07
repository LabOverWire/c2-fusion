# Firmware

Firmware for the microcontroller-side components of the RF bearer. Each component is its own Cargo project with its own target and toolchain settings, kept out of the host workspace at the repository root.

## Components

| Component | Hardware | Firmware | Status |
|---|---|---|---|
| `lora-modem` | ESP32-C3 (RISC-V) + Waveshare Core1262-HF (SX1262, 850-930 MHz) | Rust, `no_std`, esp-hal + Embassy | Boots and logs; radio driver not yet added |
| REYAX RYLR890 | SX1276 + STM32, UART AT commands | Vendor firmware, driven from the host | No firmware here |
| Heltec WiFi LoRa 32 V2 | ESP32 (Xtensa) + SX1276 | Not started | Optional second modem path |
| GNSS (UM980, GT-U7) | UART NMEA and PPS into the host | None, read by the host node | No firmware here |

The host side (the Linux node that runs `c2-node`) talks to a modem over USB serial. The frame format between host and modem belongs in a shared `no_std` crate so both sides use the same definition.

## lora-modem

Target `riscv32imc-unknown-none-elf` on stable Rust; `rust-toolchain.toml` pins the channel, the target and `rust-src`.

```
cd firmware/lora-modem
cargo build --release
cargo run --release
```

`cargo run` flashes and opens the serial monitor through `espflash`, which must be version 3.3.0 or newer.

Wiring to the Core1262-HF uses SPI (SCK, MOSI, MISO, NSS) plus BUSY, DIO1, RESET, 3.3V and GND. Pin assignments depend on the exact ESP32-C3 board and are recorded here once fixed. Always connect a 915 MHz antenna before transmitting.
