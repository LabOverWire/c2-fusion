#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
trap cleanup EXIT

RATES="${LOSS_RATES:-0 1 5 10 20}"
TRIALS="${LOSS_TRIALS:-3}"
TIMEOUT=120
WINDOW="${LOSS_WINDOW:-30}"
read -r -a PROFILE <<< "${LOSS_PROFILE:-}"

export C2_QUIC_SEGMENTATION_OFFLOAD=false
export C2_PICTURE_INTERVAL_MS=200

epoch_of() {
    python3 -c 'import sys, datetime; print(datetime.datetime.fromisoformat(sys.argv[1][:26] + "+00:00").timestamp())' "$1"
}

first_timestamp() {
    local container="$1" pattern="$2"
    docker logs --timestamps "$container" 2>&1 | grep -m1 -- "$pattern" | cut -d' ' -f1
}

inject() {
    local id="$1"
    printf '{"message_type":"ContactReport","id":"%s","reporting_unit":"UAV-7","domain":"Air","service":"Intelligence","observed_at":"2026-10-11T12:00:00Z","latitude":45.5,"longitude":-73.5,"description":"Loss trial"}' "$id" > "$HARNESS_DIR/run/inbox/edge/$id.tmp"
    mv "$HARNESS_DIR/run/inbox/edge/$id.tmp" "$HARNESS_DIR/run/inbox/edge/$id.json"
}

netem_stats() {
    docker exec c2-edge tc -s qdisc show dev eth0 | python3 -c '
import re, sys
text = sys.stdin.read()
sent = re.search(r"Sent (\d+) bytes (\d+) pkt \(dropped (\d+)", text)
if not sent:
    sys.exit("no netem statistics")
bytes_sent, packets, dropped = (int(group) for group in sent.groups())
offered = packets + dropped
print(f"{dropped / offered * 100 if offered else 0:.1f} {offered} {bytes_sent / packets if packets else 0:.0f}")
'
}

build_images
start_and_converge

log "loss sweep on the edge uplink (profile: ${LOSS_PROFILE:-none}, UDP segmentation offload disabled, ${TRIALS} trials per rate)"
printf '%-10s %-6s %-12s %-10s %-14s %s\n' "configured" "trial" "measured" "packets" "avg_bytes_pkt" "convergence_s"
for rate in $RATES; do
    for trial in $(seq 1 "$TRIALS"); do
        id="LOSS-${rate}-${trial}"
        docker exec c2-edge tc qdisc del dev eth0 root 2>/dev/null || true
        docker exec c2-edge tc qdisc add dev eth0 root netem "${PROFILE[@]}" loss "${rate}%"
        window_end=$(( $(date +%s) + WINDOW ))
        inject "$id"
        deadline=$(( $(date +%s) + TIMEOUT ))
        until docker logs --tail 5 c2-hq 2>&1 | grep -q -- "$id"; do
            if [ "$(date +%s)" -ge "$deadline" ]; then
                echo "FAIL: $id did not reach hq within ${TIMEOUT}s at ${rate}% loss"
                exit 1
            fi
            sleep 0.2
        done
        ingested="$(first_timestamp c2-edge "ingested id=$id")"
        seen="$(first_timestamp c2-hq "$id")"
        while [ "$(date +%s)" -lt "$window_end" ]; do
            sleep 1
        done
        convergence="$(python3 -c 'import sys; print(f"{float(sys.argv[2]) - float(sys.argv[1]):.2f}")' "$(epoch_of "$ingested")" "$(epoch_of "$seen")")"
        read -r measured packets average <<< "$(netem_stats)"
        printf '%-10s %-6s %-12s %-10s %-14s %s\n' "${rate}%" "$trial" "${measured}%" "$packets" "$average" "$convergence"
    done
done
docker exec c2-edge tc qdisc del dev eth0 root 2>/dev/null || true

log "LOSS SWEEP COMPLETE"
