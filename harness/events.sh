#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

export C2_EVENTS_LISTEN=0.0.0.0:7600
CONSUMER="$C2_DIR/examples/events_consumer.py"
PIDS=()

finish() {
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    cleanup
}
trap finish EXIT

wait_for_line() {
    local file="$1" pattern="$2" timeout="$3" deadline
    deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        grep -q "$pattern" "$file" 2>/dev/null && return 0
        sleep 1
    done
    return 1
}

last_digest() {
    docker logs --tail 3 "$1" 2>/dev/null | grep '^PICTURE' | tail -1 | sed 's/.*digest=\([0-9a-f]*\).*/\1/'
}

build_images
start_and_converge

log "attach the example consumer to hq (127.0.0.1:7600) and edge (127.0.0.1:7601)"
for target in hq:7600 edge:7601; do
    python3 -I "$CONSUMER" --port "${target#*:}" > "$HARNESS_DIR/run/consumer-${target%%:*}.log" &
    PIDS+=("$!")
done
for node in hq edge; do
    wait_for_line "$HARNESS_DIR/run/consumer-$node.log" "^READY node=$node count=3" 30 || { echo "FAIL: $node consumer did not get the snapshot"; exit 1; }
    echo "$node consumer received the 3-message snapshot"
done

log "partition edge and inject C-2 while it is isolated"
partition_edge
cp "$HARNESS_DIR/messages/inject.json" "$HARNESS_DIR/run/inbox/edge/inject.json"
wait_for_count c2-edge 4 30 && echo "edge ingested C-2 while isolated"

log "heal the partition"
heal_edge
for n in c2-edge c2-relay c2-hq; do
    wait_for_count "$n" 4 90 && echo "$n reconverged to 4"
done
sleep 3

log "stop the consumers"
for pid in "${PIDS[@]}"; do
    kill "$pid"
done
wait "${PIDS[@]}" 2>/dev/null || true
PIDS=()

status=0
for node in hq edge; do
    file="$HARNESS_DIR/run/consumer-$node.log"
    echo "--- consumer on $node"
    cat "$file"
    want=$(last_digest "c2-$node")
    got=$(grep '^CONSUMER' "$file" | tail -1 | sed 's/.*digest=\([0-9a-f]*\).*/\1/')
    if [ "$got" = "$want" ]; then
        echo "$node: consumer picture digest $got matches the node's PICTURE digest"
    else
        echo "FAIL: $node consumer digest $got, node digest $want"
        status=1
    fi
done
grep -q "op=insert id=C-2 origin=local" "$HARNESS_DIR/run/consumer-edge.log" && echo "edge consumer saw C-2 as a local insert during the partition" || { echo "FAIL: edge consumer missed C-2"; status=1; }
grep -q "op=insert id=C-2 origin=remote" "$HARNESS_DIR/run/consumer-hq.log" && echo "hq consumer saw C-2 as a remote insert after the heal" || { echo "FAIL: hq consumer missed C-2"; status=1; }

[ "$status" = 0 ] && echo "EVENTS PASSED: snapshot, live local and remote changes, consumer picture equals node picture"
exit "$status"
