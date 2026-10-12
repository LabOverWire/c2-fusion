#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
trap cleanup EXIT

build_images
start_and_converge

log "apply UHF bearer profile to the edge link, both directions (rate+delay)"
shape rate 64kbit delay 40ms 15ms
show_shaping
wait_for_count c2-hq 3 30 && echo "hq holds count=3 under the degraded profile"

log "partition edge from the network (loses broker and peers)"
partition_edge
sleep 3

log "inject a new contact (C-2) into edge during the outage"
cp "$HARNESS_DIR/messages/inject.json" "$HARNESS_DIR/run/inbox/edge/inject.json"
wait_for_count c2-edge 4 30 && echo "edge ingested C-2 locally (count=4) while isolated"
if grep -q "count=4" <<< "$(docker logs --tail 3 c2-hq)"; then
    echo "UNEXPECTED: hq already has C-2 during partition"; else
    echo "confirmed: hq still count=3, C-2 not yet propagated"
fi

log "heal the partition (edge re-discovers via MQDB)"
heal_edge

log "wait for reconvergence (all nodes count=4)"
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 4 90; then echo "$n reconverged to 4"; else echo "FAIL: $n did not reach 4"; docker logs --tail 20 "$n"; exit 1; fi
done

log "DEMO PASSED: MQDB discovery, converge, degrade, partition, inject, reconverge"
