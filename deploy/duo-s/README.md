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

## Wi-Fi

`deploy/duo-s/wifi.sh <board-host> <ssid> < passphrase-file` joins a WPA2 network. The key is derived on the host (PBKDF2, as `wpa_passphrase` does), so the plaintext passphrase never reaches the board; `/etc/wpa_supplicant.conf` holds only the derived key and is root-only. Re-running replaces the network's key. `/etc/init.d/S99wifi` starts `wpa_supplicant` at boot once the AIC8800 driver has created `wlan0`, and `dhcpcd` takes the address.

## What it sets up

- `/opt/c2/c2-node`: the node binary.
- `/etc/default/c2-node`: one `KEY=value` setting per line, read (not executed) at start.
- `/opt/c2/run.sh`: waits up to 120 seconds for NTP to set the clock (the board has no RTC and boots at 1970), then applies the `C2_PINMUX` pin routing and runs the node with its output sent to syslog.
- A `::respawn:/opt/c2/run.sh` line in `/etc/inittab`, so busybox init starts the node at boot and restarts it if it exits.
- `/opt/c2/inbox`: drop NIEM JSON messages here and the node ingests them into the shared picture.

## Operating

- Logs: `grep c2-node /var/log/messages`. Syslog keeps them in RAM with size-based rotation, so they do not survive a reboot.
- Stop for good: remove the `run.sh` line from `/etc/inittab`, run `kill -HUP 1`, then `killall c2-node`.
- The Ethernet MAC is randomised on every boot, so DHCP addresses change unless the router reserves them. Find a board by connecting to SSH hosts and checking `hostname`, or use the USB link. The stock image puts every board at `192.168.42.1`; to connect two at once, change the address in `/mnt/system/usb-ncm.sh` and the DHCP range in `/etc/dnsmasq.conf` on one of them (node-b uses `192.168.43.1`).
- The shared picture is held in memory, so a rebooted node rejoins empty and re-syncs from its peers.
