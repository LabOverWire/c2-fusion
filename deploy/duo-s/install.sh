#!/bin/sh
set -eu

if [ "$#" -ne 3 ] && [ "$#" -ne 4 ]; then
    echo "usage: install.sh <board-host> <node-name> <broker-host:port> [lora-address]" >&2
    exit 1
fi

target="root@$1"
name=$2
broker=$3
user=${C2_BROKER_USER:-c2}
pass=${C2_BROKER_PASS:-c2pass}
lora_address=${4:-}
known_hosts="UserKnownHostsFile=${C2_KNOWN_HOSTS:-$HOME/.ssh/known_hosts}"

here=$(cd "$(dirname "$0")" && pwd)
binary="$here/../../target/riscv64gc-unknown-linux-musl/release/c2-node"

if [ ! -f "$binary" ]; then
    echo "missing $binary, build it with: cargo zigbuild --release --target riscv64gc-unknown-linux-musl -p c2-node" >&2
    exit 1
fi

ssh -o "$known_hosts" "$target" 'mkdir -p /opt/c2/inbox /etc/default'
ssh -o "$known_hosts" "$target" 'cat > /opt/c2/c2-node.new && chmod 755 /opt/c2/c2-node.new && mv /opt/c2/c2-node.new /opt/c2/c2-node' < "$binary"
ssh -o "$known_hosts" "$target" 'cat > /opt/c2/run.sh && chmod 755 /opt/c2/run.sh' < "$here/run.sh"
{
    printf 'C2_NAME=%s\nC2_BROKER=%s\nC2_BROKER_USER=%s\nC2_BROKER_PASS=%s\n' "$name" "$broker" "$user" "$pass"
    printf 'C2_INBOX=/opt/c2/inbox\nC2_INBOX_FMT=c2-json\nC2_PICTURE_INTERVAL_MS=60000\nC2_STORE_DIR=/opt/c2/store\n'
    pinmux=""
    if [ -n "$lora_address" ]; then
        pinmux="B11/UART2_TX,B12/UART2_RX"
        printf 'C2_LORA_PORT=/dev/ttyS2\nC2_LORA_RESET_GPIO=470\n'
        printf 'C2_LORA_ADDRESS=%s\nC2_LORA_NETWORK=%s\nC2_LORA_POWER_DBM=%s\nC2_LORA_SPREADING_FACTOR=%s\n' \
            "$lora_address" "${C2_LORA_NETWORK:-6}" "${C2_LORA_POWER_DBM:-15}" "${C2_LORA_SPREADING_FACTOR:-9}"
    fi
    if [ -n "${C2_GNSS:-}" ]; then
        pinmux="${pinmux:+$pinmux,}A18/UART1_RX,A19/UART1_TX,A20/A20"
        printf 'C2_GNSS_PORT=/dev/ttyS1\nC2_GNSS_BAUD=%s\nC2_PPS_CHIP=/dev/gpiochip0\nC2_PPS_LINE=20\n' "${C2_GNSS_BAUD:-115200}"
    fi
    printf 'C2_PINMUX=%s\n' "$pinmux"
} | ssh -o "$known_hosts" "$target" 'cat > /etc/default/c2-node && chmod 600 /etc/default/c2-node'
ssh -o "$known_hosts" "$target" 'grep -q "/opt/c2/run.sh" /etc/inittab || echo "::respawn:/opt/c2/run.sh" >> /etc/inittab'
ssh -o "$known_hosts" "$target" 'kill -HUP 1; killall c2-node 2>/dev/null; true'

echo "installed c2-node as $name on $1, broker $broker"
