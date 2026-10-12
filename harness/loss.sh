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
    local container="$1" pattern="$2" logs
    logs="$(docker logs --timestamps "$container" 2>&1)"
    grep -m1 -- "$pattern" <<< "$logs" | cut -d' ' -f1
}

inject() {
    local id="$1"
    printf '{"message_type":"ContactReport","id":"%s","reporting_unit":"UAV-7","domain":"Air","service":"Intelligence","observed_at":"2026-10-11T12:00:00Z","latitude":45.5,"longitude":-73.5,"description":"Loss trial"}' "$id" > "$HARNESS_DIR/run/inbox/edge/$id.tmp"
    mv "$HARNESS_DIR/run/inbox/edge/$id.tmp" "$HARNESS_DIR/run/inbox/edge/$id.json"
}

direction_stats() {
    docker exec c2-router tc -s qdisc show dev "$(router_iface "$1")" | python3 -c '
import re, sys
sent = re.search(r"Sent (\d+) bytes (\d+) pkt \(dropped (\d+)", sys.stdin.read())
if not sent:
    sys.exit("no netem statistics")
print(*sent.groups())
'
}

netem_stats() {
    python3 -c '
import sys
up = [int(value) for value in sys.argv[1].split()]
down = [int(value) for value in sys.argv[2].split()]
def loss(stats):
    offered = stats[1] + stats[2]
    return stats[2] / offered * 100 if offered else 0
packets = up[1] + down[1]
offered = packets + up[2] + down[2]
print(f"{loss(up):.1f} {loss(down):.1f} {offered} {(up[0] + down[0]) / packets if packets else 0:.0f}")
' "$(direction_stats "$CORE_SIDE")" "$(direction_stats "$EDGE_SIDE")"
}

build_images
start_and_converge

log "loss sweep on the edge link through the router, both directions (profile: ${LOSS_PROFILE:-none}, UDP segmentation offload disabled, ${TRIALS} trials per rate)"
printf '%-10s %-6s %-12s %-12s %-10s %-14s %s\n' "configured" "trial" "from_edge" "to_edge" "packets" "avg_bytes_pkt" "convergence_s"
for rate in $RATES; do
    for trial in $(seq 1 "$TRIALS"); do
        id="LOSS-${rate}-${trial}"
        unshape
        shape "${PROFILE[@]}" loss "${rate}%"
        window_end=$(( $(date +%s) + WINDOW ))
        inject "$id"
        deadline=$(( $(date +%s) + TIMEOUT ))
        until grep -q -- "$id" <<< "$(docker logs --tail 5 c2-hq 2>&1)"; do
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
        read -r from_edge to_edge packets average <<< "$(netem_stats)"
        printf '%-10s %-6s %-12s %-12s %-10s %-14s %s\n' "${rate}%" "$trial" "${from_edge}%" "${to_edge}%" "$packets" "$average" "$convergence"
    done
done
unshape

log "LOSS SWEEP COMPLETE"
