#!/usr/bin/env bash
# End-to-end DDIL harness demo: build the node image from a minimal staging
# context, bring up a 3-node ring, and show the shared C2 picture converge, then
# reconverge after a network partition with a write injected during the outage.
set -euo pipefail

# Build and run natively for the host arch; an amd64 base under emulation is
# both slow and flaky for a QUIC/tokio workload.
export DOCKER_DEFAULT_PLATFORM="linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"

HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
C2_DIR="$(cd "$HARNESS_DIR/.." && pwd)"
LAB_DIR="$(cd "$C2_DIR/.." && pwd)"
compose() { docker compose -f "$HARNESS_DIR/docker-compose.yml" "$@"; }

log() { printf '\n=== %s ===\n' "$1"; }

wait_for_count() {
    # wait_for_count <container> <expected count> <timeout secs>
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
    [ -n "${STAGE:-}" ] && rm -rf "$STAGE"
}
trap cleanup EXIT

log "assemble build context"
STAGE="$(mktemp -d)"
mkdir -p "$STAGE/stitch-rs/crates"
rsync -a --exclude target "$LAB_DIR/stitch-rs/crates/stitch-p2p" "$STAGE/stitch-rs/crates/"
rsync -a --exclude target "$LAB_DIR/stitch-rs/crates/mqp2p" "$STAGE/stitch-rs/crates/"
rsync -a --exclude target --exclude .git --exclude run "$C2_DIR/" "$STAGE/c2-fusion/"

log "build image c2-node:poc"
docker build -t c2-node:poc -f "$HARNESS_DIR/Dockerfile" "$STAGE"

log "prepare run dirs"
mkdir -p "$HARNESS_DIR/run/inbox/edge"

log "bring up ring (edge -> relay -> hq -> edge)"
compose up -d

log "wait for initial convergence (all nodes count=3)"
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 3 60; then echo "$n converged to 3"; else echo "FAIL: $n did not reach 3"; exit 1; fi
done

log "apply UHF bearer profile to edge uplink (rate+delay, no loss so no GSO caveat)"
if docker exec c2-edge sh -c 'command -v tc >/dev/null'; then
    docker exec c2-edge tc qdisc add dev eth0 root netem rate 64kbit delay 40ms 15ms
    docker exec c2-edge tc qdisc show dev eth0
    echo "expecting the picture to hold under the degraded link"
    wait_for_count c2-hq 3 30 && echo "hq holds count=3 under UHF profile"
else
    echo "tc not in image, skipping netem shaping (partition demo below does not need it)"
fi

log "partition edge from the network"
docker network disconnect c2net c2-edge
sleep 3

log "inject a new contact (C-2) into edge during the outage"
cp "$HARNESS_DIR/messages/inject.json" "$HARNESS_DIR/run/inbox/edge/inject.json"
wait_for_count c2-edge 4 30 && echo "edge ingested C-2 locally (count=4) while isolated"
if docker logs --tail 3 c2-hq | grep -q "count=4"; then
    echo "UNEXPECTED: hq already has C-2 during partition"; else
    echo "confirmed: hq still count=3, C-2 not yet propagated"
fi

log "heal the partition"
docker network connect c2net c2-edge

log "wait for reconvergence (all nodes count=4)"
for n in c2-edge c2-relay c2-hq; do
    if wait_for_count "$n" 4 60; then echo "$n reconverged to 4"; else echo "FAIL: $n did not reach 4"; exit 1; fi
done

log "DEMO PASSED: converge, degrade, partition, inject, reconverge"
