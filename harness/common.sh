#!/usr/bin/env bash
DOCKER_DEFAULT_PLATFORM="linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
export DOCKER_DEFAULT_PLATFORM

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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

build_images() {
    log "build MQDB broker image (skip if already present)"
    if ! docker image inspect mqdb:poc >/dev/null 2>&1; then
        docker build -t mqdb:poc "$MQDB_DIR"
    fi

    log "build MQDB runtime image (alpine base + baked dev auth)"
    docker build -t mqdb-run:poc -f "$HARNESS_DIR/mqdb.Dockerfile" "$HARNESS_DIR"

    log "build c2-node image"
    docker build -t c2-node:poc -f "$HARNESS_DIR/Dockerfile" "$C2_DIR"
}

start_and_converge() {
    log "prepare run dirs"
    mkdir -p "$HARNESS_DIR/run/inbox/edge"

    log "bring up MQDB broker and three nodes"
    compose up -d

    log "wait for discovery + convergence via MQDB (all nodes count=3)"
    local n
    for n in c2-edge c2-relay c2-hq; do
        if wait_for_count "$n" 3 90; then
            echo "$n converged to 3"
        else
            echo "FAIL: $n did not reach 3"
            docker logs --tail 20 "$n"
            exit 1
        fi
    done
}
