#!/usr/bin/env bash
# End-to-end DDIL harness: an MQDB broker for peer discovery, three c2-node
# containers that discover each other through MQDB and converge over QUIC, then a
# network partition with a write injected during the outage, then reconvergence.
set -euo pipefail

export DOCKER_DEFAULT_PLATFORM="linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"

HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
C2_DIR="$(cd "$HARNESS_DIR/.." && pwd)"
MQDB_DIR="$(cd "$C2_DIR/../../MQDB" && pwd)"
compose() { docker compose -f "$HARNESS_DIR/docker-compose.yml" "$@"; }

log() { printf '\n=== %s ===\n' "$1"; }

wait_for_count() {
    local container="$1" want="$2" timeout="$3" deadline
    deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if docker logs --tail 3 "$container" 2>/dev/null | grep -q "count=$want"; then
            return 0
        fi
        sleep 1
    done
    return 1
}

cleanup() {
    log "teardown"
    compose down -v --remove-orphans >/dev/null 2>&1 || true
    rm -rf "$HARNESS_DIR/run"
}
trap cleanup EXIT

log "build MQDB broker image (skip if already present)"
if ! docker image inspect mqdb:poc >/dev/null 2>&1; then
    docker build -t mqdb:poc "$MQDB_DIR"
fi

log "build MQDB runtime image (alpine base + baked dev auth)"
docker build -t mqdb-run:poc -f "$HARNESS_DIR/mqdb.Dockerfile" "$HARNESS_DIR"

log "build c2-node image"
docker build -t c2-node:poc -f "$HARNESS_DIR/Dockerfile" "$C2_DIR"

log "prepare run dirs"
mkdir -p "$HARNESS_DIR/run/inbox/edge"

log "bring up MQDB broker and three nodes"
compose up -d

log "wait for discovery + convergence via MQDB (all nodes count=3)"
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 3 90; then echo "$n converged to 3"; else echo "FAIL: $n did not reach 3"; docker logs --tail 20 "$n"; exit 1; fi
done

log "apply UHF bearer profile to edge uplink (rate+delay)"
docker exec c2-edge tc qdisc add dev eth0 root netem rate 64kbit delay 40ms 15ms || true
docker exec c2-edge tc qdisc show dev eth0 || true
wait_for_count c2-hq 3 30 && echo "hq holds count=3 under the degraded profile"

log "partition edge from the network (loses broker and peers)"
docker network disconnect c2net c2-edge
sleep 3

log "inject a new contact (C-2) into edge during the outage"
cp "$HARNESS_DIR/messages/inject.json" "$HARNESS_DIR/run/inbox/edge/inject.json"
wait_for_count c2-edge 4 30 && echo "edge ingested C-2 locally (count=4) while isolated"
if docker logs --tail 3 c2-hq | grep -q "count=4"; then
    echo "UNEXPECTED: hq already has C-2 during partition"; else
    echo "confirmed: hq still count=3, C-2 not yet propagated"
fi

log "heal the partition (edge re-discovers via MQDB)"
docker network connect c2net c2-edge

log "wait for reconvergence (all nodes count=4)"
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 4 90; then echo "$n reconverged to 4"; else echo "FAIL: $n did not reach 4"; docker logs --tail 20 "$n"; exit 1; fi
done

log "DEMO PASSED: MQDB discovery, converge, degrade, partition, inject, reconverge"
