#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"
trap 'docker rm -f c2-presence-sub >/dev/null 2>&1 || true; cleanup' EXIT

broker_pass=$(sed -n 's/.*C2_BROKER_PASS: //p' "$HARNESS_DIR/docker-compose.yml" | head -1 | tr -d '"')

presence_log() { docker logs c2-presence-sub 2>&1; }

wait_event() {
    local client="$1" event="$2" since="$3" timeout="$4" deadline line
    deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        line=$(presence_log | grep "_presence/$client " | grep "\"event\":\"$event\"" | awk -v s="$since" '$1 >= s' | head -1 || true)
        if [ -n "$line" ]; then
            echo "$line"
            return 0
        fi
        sleep 0.2
    done
    return 1
}

elapsed() { python3 -c "import sys; print(f'{float(sys.argv[1]) - float(sys.argv[2]):.2f}')" "$1" "$2"; }

now() { python3 -c 'import time; print(f"{time.time():.3f}")'; }

measure() {
    local label="$1" client="$2" event="$3" since="$4" timeout="$5" line
    if line=$(wait_event "$client" "$event" "$since" "$timeout"); then
        echo "$label: $event after $(elapsed "$(awk '{print $1}' <<< "$line")" "$since") s :: $(cut -d' ' -f3- <<< "$line")"
    else
        echo "$label: no $event within $timeout s"
    fi
}

build_images
start_and_converge

docker run -d --name c2-presence-sub --network c2net eclipse-mosquitto:2 \
    mosquitto_sub -h 10.231.0.2 -V 5 -u c2 -P "$broker_pass" -v -F '%U %t %p' -t '$DB/_presence/#' >/dev/null
sleep 2
log "retained presence at subscribe time"
presence_log

log "A: docker stop relay (SIGTERM)"
t=$(now); docker stop c2-relay >/dev/null
measure "A relay stop" mqp2p-relay disconnect "$t" 30
t=$(now); docker start c2-relay >/dev/null
measure "A relay start" mqp2p-relay connect "$t" 30

log "B: docker kill hq (SIGKILL)"
t=$(now); docker kill c2-hq >/dev/null
measure "B hq kill" mqp2p-hq disconnect "$t" 150
t=$(now); docker start c2-hq >/dev/null
measure "B hq start" mqp2p-hq connect "$t" 30

log "C: partition edge at the router"
t=$(now); partition_edge
measure "C edge partition" mqp2p-edge disconnect "$t" 150
t=$(now); heal_edge
measure "C edge heal" mqp2p-edge connect "$t" 120

log "full presence log"
presence_log
