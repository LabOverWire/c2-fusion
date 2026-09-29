#!/usr/bin/env bash
set -euo pipefail

# Applies an emulated tactical-bearer profile to a network interface using tc/netem.
# Linux only, requires NET_ADMIN (run inside the harness containers, not on the host).
# Bandwidth, delay, and loss are placeholders pending the real HF/UHF/SATCOM figures
# gathered in the reading track; see harness/README.md.

usage() {
    echo "usage: $0 <interface> <hf|uhf|satcom|clear>" >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
iface="$1"
profile="$2"

tc qdisc del dev "$iface" root 2>/dev/null || true

case "$profile" in
    hf)
        # High Frequency: very low rate, long delay, lossy.
        tc qdisc add dev "$iface" root netem rate 2400bit delay 250ms 100ms loss 15%
        ;;
    uhf)
        # Ultra High Frequency line-of-sight: modest rate, low delay, some loss.
        tc qdisc add dev "$iface" root netem rate 64kbit delay 40ms 15ms loss 3%
        ;;
    satcom)
        # Geostationary SATCOM: usable rate, very long delay, occasional loss.
        tc qdisc add dev "$iface" root netem rate 256kbit delay 300ms 40ms loss 1%
        ;;
    clear)
        echo "cleared netem on $iface"
        exit 0
        ;;
    *)
        usage
        ;;
esac

echo "applied $profile profile to $iface"
tc qdisc show dev "$iface"
