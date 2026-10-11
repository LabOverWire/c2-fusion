# c2-node on Milk-V Duo S

Runs `c2-node` as a supervised service on a Milk-V Duo S (SG2000, RISC-V) with the official Buildroot v2.0.1 musl image on eMMC.

## Install

Build the static RISC-V binary, then install it on each board:

```
cargo zigbuild --release --target riscv64gc-unknown-linux-musl -p c2-node
deploy/duo-s/install.sh <board-host> <node-name> <broker-host:port>
```

For example, `deploy/duo-s/install.sh 192.168.1.89 node-a 192.168.1.163:1883`. The board must accept SSH as root with your key. `C2_BROKER_USER` and `C2_BROKER_PASS` override the development credentials (`c2` / `c2pass`), and `C2_KNOWN_HOSTS` points SSH at a different known hosts file.

Re-running the script upgrades the binary and restarts the node.

A fourth argument gives the node a LoRa address and enables the RYLR890 bearer on UART2 (pins 11 and 13, reset on pin 15). `C2_LORA_NETWORK`, `C2_LORA_POWER_DBM` and `C2_LORA_SPREADING_FACTOR` override the radio defaults (6, 15, 9).

The LoRa bearer is a stitch-p2p sync session, like the peer-to-peer QUIC sessions: every write to the shared picture goes out as its replication frame, carrying the origin's stamp and per-origin sequence number, and a received frame is applied with that stamp rather than re-ingested, so the radio path keeps the same convergence rules as the rest of the exchange. On the air a frame is re-encoded compactly (a C2 record travels in the compact codec, about 130 bytes in all). Writes are sent one at a time and acknowledged (stop-and-wait, with a short random gap before each new frame so two nodes do not stay in lockstep); a frame that arrives out of order is held, up to 32, until the missing one arrives. Every 30 seconds, and when a gap persists, each node broadcasts its per-origin cursors as an unacknowledged datagram, and its peer resends exactly the frames it is missing. That repairs frames the link gave up on and brings a node back in line after a partition. A frame larger than one LoRa payload (237 bytes) is split into up to 16 fragments of about 230 bytes, each sent and acknowledged like any other frame; the receiver reassembles them by sender and fragment id in any order, holds at most 4 incomplete messages and drops one after 60 seconds, and a write whose fragments never all arrive is resent whole by the cursor catch-up. Records up to about 3.5 KB therefore cross the radio; a larger one is skipped and logged, and would block later writes from its origin.

## GNSS

Setting `C2_GNSS=1` when running `install.sh` enables a GNSS receiver with PPS on UART1, at `C2_GNSS_BAUD` (default 115200, which suits the UM980; the GT-U7 needs 9600). Both receivers use the same header pins. Wiring for the UM980 carrier:

| Duo S header | Receiver |
|---|---|
| pin 2 (VSYS 5V) | VCC |
| pin 20 (GND) | GND |
| pin 22 (UART1 RX) | TX2 |
| pin 18 (UART1 TX) | RX2 |
| pin 16 (GPIO 500) | PPS |

The receiver's own USB-C stays unplugged while VCC is wired. At start the node asks the receiver for GGA and RMC once per second, so the receiver's saved configuration is left alone. The PPS rising edge is read from `/dev/gpiochip0` line 20 with its kernel interrupt timestamp and paired with the RMC that follows it. Each second the measured offset goes to the kernel clock discipline (`adjtimex` in PLL mode), which corrects both the time and the oscillator's frequency error, so the clock stays on GNSS time and keeps the learned frequency if the fix is lost. An offset of half a second or more, such as the 1970 clock at boot, is stepped instead. With GNSS enabled, `run.sh` does not wait for NTP before starting the node, and it replaces the ntpd daemon with a one-shot `ntpd -gq`, so the network can still set the clock when there is no fix without fighting the GNSS discipline afterwards. Every minute the node logs fix quality, satellite count, PPS edges, the latest and worst clock offset; it never logs coordinates.

Once it has a fix and its clock is disciplined, the node also shares its own position every `C2_POSITION_INTERVAL_S` seconds (default 60) as a `PositionReport` with id `POS-<name>`: it goes into the shared picture and out over LoRa. Every report reuses the same id, so the picture holds the latest position of each node.

## Wi-Fi

`deploy/duo-s/wifi.sh <board-host> <ssid> < passphrase-file` joins a WPA2 network. The key is derived on the host (PBKDF2, as `wpa_passphrase` does), so the plaintext passphrase never reaches the board; `/etc/wpa_supplicant.conf` holds only the derived key and is root-only. Re-running replaces the network's key. `/etc/init.d/S99wifi` starts `wpa_supplicant` at boot once the AIC8800 driver has created `wlan0`, and `dhcpcd` takes the address.

## What it sets up

- `/opt/c2/c2-node`: the node binary.
- `/etc/default/c2-node`: one `KEY=value` setting per line, read (not executed) at start.
- `/opt/c2/run.sh`: unless GNSS is enabled, waits up to 120 seconds for NTP to set the clock (the board has no RTC and boots at 1970); with GNSS, swaps the ntpd daemon for a one-shot sync. It then applies the `C2_PINMUX` pin routing and runs the node with its output sent to syslog.
- A `::respawn:/opt/c2/run.sh` line in `/etc/inittab`, so busybox init starts the node at boot and restarts it if it exits.
- `/opt/c2/inbox`: drop NIEM JSON messages here and the node ingests them into the shared picture.

## Operating

- Logs: `grep c2-node /var/log/messages`. Syslog keeps them in RAM with size-based rotation, so they do not survive a reboot.
- Stop for good: remove the `run.sh` line from `/etc/inittab`, run `kill -HUP 1`, then `killall c2-node`.
- The Ethernet MAC is randomised on every boot, so DHCP addresses change unless the router reserves them. Find a board by connecting to SSH hosts and checking `hostname`, or use the USB link. The stock image puts every board at `192.168.42.1`; to connect two at once, change the address in `/mnt/system/usb-ncm.sh` and the DHCP range in `/etc/dnsmasq.conf` on one of them (node-b uses `192.168.43.1`).
- The shared picture and its replication log persist in `C2_STORE_DIR` (`/opt/c2/store`), so a rebooted node comes back with its picture and continues its own write sequence where it left off. Wiping that directory gives the node a fresh log under the same identity; its new writes would reuse sequence numbers its peers already hold and be dropped, so a node with a wiped store should be given a new `C2_NAME`.
- The broker is optional. The node's identity is derived from `C2_NAME`, and LoRa, GNSS and the inbox start at boot whether or not the broker is reachable, so a node out of network range still exchanges messages over the radio. Registration and peer-to-peer sync join in the background once the broker answers; the node probes it with backoff from 1 to 60 seconds.
