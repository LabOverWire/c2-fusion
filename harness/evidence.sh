#!/usr/bin/env bash
# Evidence run: shows the real mechanism, MQDB peer registrations, per-node
# message-id sets, and the drop/resync, at each phase. Reuses already-built
# images (run.sh builds them). Keeps full node logs; prints to stdout.
set -euo pipefail

export DOCKER_DEFAULT_PLATFORM="linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
compose() { docker compose -f "$HARNESS_DIR/docker-compose.yml" "$@"; }
hr() { printf '\n========== %s ==========\n' "$1"; }

mqdb_peers() {
    echo "now=$(date +%s) (presence lease: a peer is live only while its _expires_at > now)"
    docker run --rm --network c2net mqdb-run:poc \
        list peers --broker mqdb:1883 --user c2 --pass c2pass 2>&1 || true
}
pic() { docker logs "$1" 2>&1 | grep PICTURE | tail -1 | sed -E 's/\x1b\[[0-9;]*m//g'; }
reg() { docker logs "$1" 2>&1 | grep REGISTERED | tail -1 | sed -E 's/\x1b\[[0-9;]*m//g'; }
wait_count() {
    local c="$1" n="$2" t="$3" d; d=$(( $(date +%s) + t ))
    while [ "$(date +%s)" -lt "$d" ]; do docker logs --tail 3 "$c" 2>/dev/null | grep -q "count=$n" && return 0; sleep 1; done
    return 1
}

cleanup() { hr "teardown"; compose down -v --remove-orphans >/dev/null 2>&1 || true; rm -rf "$HARNESS_DIR/run"; }
trap cleanup EXIT

mkdir -p "$HARNESS_DIR/run/inbox/edge"
hr "bring up MQDB broker + 3 nodes"
compose up -d
for n in c2-edge c2-relay c2-hq; do wait_count "$n" 3 90 || { echo "FAIL converge $n"; docker logs --tail 30 "$n"; exit 1; }; done

hr "PHASE A: converged. MQDB peer registry (proves discovery via MQDB)"
mqdb_peers
hr "PHASE A: node registrations (each node registered with the MQDB broker)"
for n in c2-edge c2-relay c2-hq; do reg "$n"; done
hr "PHASE A: each node's shared picture (message-id sets must match)"
for n in c2-edge c2-relay c2-hq; do pic "$n"; done

hr "apply UHF netem (rate+delay) to edge; picture must hold"
docker exec c2-edge tc qdisc add dev eth0 root netem rate 64kbit delay 40ms 15ms
docker exec c2-edge tc qdisc show dev eth0

hr "PARTITION edge from the network"
docker network disconnect c2net c2-edge
sleep 3
cp "$HARNESS_DIR/messages/inject.json" "$HARNESS_DIR/run/inbox/edge/inject.json"
wait_count c2-edge 4 30 || { echo "edge did not ingest C-2"; exit 1; }
hr "hold the partition past the 6s presence lease so edge's lease lapses"
sleep 8
hr "PHASE B: during partition, edge has C-2, hq does not"
echo "edge: $(pic c2-edge)"
echo "hq:   $(pic c2-hq)"
hr "PHASE B: MQDB registry while edge is partitioned (edge _expires_at < now = lapsed; relay/hq stay fresh)"
mqdb_peers

hr "HEAL partition; edge re-discovers via MQDB"
docker network connect c2net c2-edge
for n in c2-edge c2-relay c2-hq; do wait_count "$n" 4 90 || { echo "FAIL reconverge $n"; docker logs --tail 30 "$n"; exit 1; }; done
hr "PHASE C: reconverged. each node's final picture (all 4 ids, identical sets)"
for n in c2-edge c2-relay c2-hq; do pic "$n"; done
hr "PHASE C: MQDB peer registry after reconnect"
mqdb_peers

hr "EVIDENCE RUN COMPLETE"
