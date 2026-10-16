#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
trap cleanup EXIT

hr() { printf '\n========== %s ==========\n' "$1"; }

mqdb_peers() {
    echo "now=$(date +%s) (presence lease: on one host, _expires_at behind now means no renewal for a whole lease)"
    docker run --rm --network c2net mqdb-run:poc \
        list peers --broker mqdb:1883 --user c2 --pass c2pass 2>&1 || true
}
pic() { docker logs "$1" 2>&1 | grep PICTURE | tail -1 | sed -E 's/\x1b\[[0-9;]*m//g'; }
reg() { docker logs "$1" 2>&1 | grep REGISTERED | tail -1 | sed -E 's/\x1b\[[0-9;]*m//g'; }
start_and_converge

hr "PHASE A: converged. MQDB peer registry (proves discovery via MQDB)"
mqdb_peers
hr "PHASE A: node registrations (each node registered with the MQDB broker)"
for n in c2-edge c2-relay c2-hq; do reg "$n"; done
hr "PHASE A: each node's shared picture (message-id sets must match)"
for n in c2-edge c2-relay c2-hq; do pic "$n"; done

hr "apply UHF netem (rate+delay) to the edge link, both directions; picture must hold"
shape rate 64kbit delay 40ms 15ms
show_shaping

hr "PARTITION edge from the network"
partition_edge
sleep 3
inject_edge
wait_for_count c2-edge 4 30 || { echo "edge did not ingest C-2"; exit 1; }
hr "hold the partition past the 6s presence lease so edge's lease lapses"
sleep 8
hr "PHASE B: during partition, edge has C-2, hq does not"
echo "edge: $(pic c2-edge)"
echo "hq:   $(pic c2-hq)"
hr "PHASE B: MQDB registry while edge is partitioned (edge _expires_at < now = lapsed; relay/hq stay fresh)"
mqdb_peers

hr "HEAL partition; edge re-discovers via MQDB"
heal_edge
for n in c2-edge c2-relay c2-hq; do wait_for_count "$n" 4 90 || { echo "FAIL reconverge $n"; docker logs --tail 30 "$n"; exit 1; }; done
hr "PHASE C: reconverged. each node's final picture (all 4 ids, identical sets)"
for n in c2-edge c2-relay c2-hq; do pic "$n"; done
hr "PHASE C: MQDB peer registry after reconnect"
mqdb_peers

hr "EVIDENCE RUN COMPLETE"
