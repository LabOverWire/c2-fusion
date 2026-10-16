#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
trap cleanup EXIT

wait_for_line() {
    local container="$1" since="$2" pattern="$3" timeout="$4" deadline
    deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if docker logs --since "$since" "$container" 2>&1 | grep -q "$pattern"; then
            return 0
        fi
        sleep 1
    done
    return 1
}

build_images
start_and_converge

log "inject C-2 at edge and wait for every node to hold it"
inject_edge
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 4 60; then echo "$n has C-2 (count=4)"; else echo "FAIL: $n did not reach 4"; docker logs --tail 20 "$n"; exit 1; fi
done

log "stop edge with docker stop (SIGTERM, then SIGKILL after 10 s)"
started=$(date +%s)
docker stop c2-edge >/dev/null
took=$(( $(date +%s) - started ))
if docker logs --tail 5 c2-edge 2>&1 | grep -q 'STOP name=edge signal=SIGTERM' && [ "$took" -lt 5 ]; then
    echo "edge stopped on SIGTERM in ${took}s"
else
    echo "FAIL: edge did not stop on SIGTERM (took ${took}s)"
    docker logs --tail 20 c2-edge
    exit 1
fi

log "start edge again on the same store"
since=$(date +%s)
docker start c2-edge >/dev/null
if wait_for_line c2-edge "$since" 'PICTURE name=edge count=4' 30; then
    echo "edge came back with its 4-message picture from the store"
else
    echo "FAIL: edge did not come back with its picture"
    docker logs --tail 20 c2-edge
    exit 1
fi

log "inject C-3 at edge after the restart; a reused write sequence would make its peers drop it"
printf '{"message_type":"ContactReport","id":"C-3","reporting_unit":"UAV-7","domain":"Air","service":"Intelligence","observed_at":"2026-10-14T12:00:00Z","latitude":45.5,"longitude":-73.5,"description":"After restart"}' > "$HARNESS_DIR/run/inbox/edge/C-3.tmp"
mv "$HARNESS_DIR/run/inbox/edge/C-3.tmp" "$HARNESS_DIR/run/inbox/edge/C-3.json"
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 5 60; then echo "$n has C-3 (count=5)"; else echo "FAIL: $n did not reach 5"; docker logs --tail 20 "$n"; exit 1; fi
done

log "RESTART PASSED: SIGTERM stop, picture restored from the store, post-restart write replicated"
