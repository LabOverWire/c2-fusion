#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

export COMPOSE_PROFILES=monitoring
export C2_OTLP_CORE=http://10.231.0.20:4318
export C2_OTLP_EDGE=http://10.231.1.20:4318
export C2_OTLP_INTERVAL_MS="${C2_OTLP_INTERVAL_MS:-5000}"
HOLD="${HOLD:-30}"

if [ "${KEEP:-0}" = 1 ]; then
    trap 'log "stack left running; tear down with: COMPOSE_PROFILES=monitoring docker compose -f $HARNESS_DIR/docker-compose.yml down -v"' EXIT
else
    trap cleanup EXIT
fi

QUERY_FORMAT=$(cat <<'PY'
import json, sys
for row in json.load(sys.stdin)["data"]["result"]:
    labels = ",".join(k + "=" + v for k, v in sorted(row["metric"].items()) if k not in ("job", "__name__"))
    print("  " + (labels or "value") + " " + row["value"][1])
PY
)

query() {
    curl -s --get http://127.0.0.1:9090/api/v1/query --data-urlencode "query=$1" | python3 -c "$QUERY_FORMAT"
}

snapshot() {
    log "telemetry: $1"
    echo "live peers per node:"; query 'c2_peers_live'
    echo "distinct pictures (1 means converged):"; query 'count(count_values("digest", c2_picture_digest))'
    echo "replication lag in writes, per node and origin:"; query 'max by (origin) (c2_replication_cursor) - on (origin) group_right () c2_replication_cursor'
}

build_images
start_and_converge

log "monitoring stack up: Grafana http://127.0.0.1:3000, Prometheus http://127.0.0.1:9090"
sleep "$HOLD"
snapshot "converged"

log "partition edge from the network"
partition_edge
cp "$HARNESS_DIR/messages/inject.json" "$HARNESS_DIR/run/inbox/edge/inject.json"
wait_for_count c2-edge 4 30 && echo "edge ingested C-2 while isolated"
sleep "$HOLD"
snapshot "edge partitioned, C-2 only on edge"

log "heal the partition"
heal_edge
for n in c2-edge c2-relay c2-hq; do
    wait_for_count "$n" 4 90 && echo "$n reconverged to 4"
done
sleep "$HOLD"
snapshot "healed"
