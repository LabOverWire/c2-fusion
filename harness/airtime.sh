#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
trap cleanup EXIT

WINDOW="${AIRTIME_WINDOW:-120}"
SETTLE="${AIRTIME_SETTLE:-30}"
read -r -a PROFILE <<< "${AIRTIME_PROFILE:-rate 2400bit delay 250ms 100ms}"
OUT="${AIRTIME_OUT:-$HARNESS_DIR/airtime.pcap}"

export C2_QUIC_SEGMENTATION_OFFLOAD=false
export C2_PULL_MS="${C2_PULL_MS:-30000}"
export C2_DISCOVERY_MS="${C2_DISCOVERY_MS:-30000}"
export C2_LEASE_SECS="${C2_LEASE_SECS:-6}"

build_images
start_and_converge

log "shape the edge link (${PROFILE[*]}), both directions, and let it settle for ${SETTLE} s"
shape "${PROFILE[@]}"
sleep "$SETTLE"

log "capture ${WINDOW} s of idle traffic on the edge side of the router (pull ${C2_PULL_MS} ms, discovery ${C2_DISCOVERY_MS} ms, lease ${C2_LEASE_SECS} s)"
iface="$(router_iface "$EDGE_SIDE")"
docker exec c2-router timeout "$WINDOW" tcpdump -i "$iface" -s 0 -U -w /tmp/airtime.pcap 2>/dev/null || true
docker cp c2-router:/tmp/airtime.pcap "$OUT"
echo "wrote $OUT"
