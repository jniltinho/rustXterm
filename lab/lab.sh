#!/bin/sh
# Host-side lab driver: up | down | status | test. Needs only Docker.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
gen="$root/lab/.generated"
compose() { docker compose -f "$root/lab/compose.yaml" "$@"; }

case "${1:-}" in
up)
    mkdir -p "$gen/logs"
    compose build
    if [ ! -f "$gen/bob_ed25519" ]; then
        docker run --rm --user "$(id -u):$(id -g)" -v "$gen:/out" --entrypoint ssh-keygen \
            rxt-lab-ssh:dev -q -t ed25519 -N '' -C rxt-lab-bob -f /out/bob_ed25519
    fi
    compose up -d --wait --wait-timeout 120
    ;;
down) compose down -v --remove-orphans ;;
status) compose ps ;;
test)
    RXT_DOCKER_NET=host CARGO_TARGET_DIR=target/lab exec "$root/scripts/cargo-docker.sh" cargo xtask lab test
    ;;
*) echo "usage: lab/lab.sh up|down|status|test" >&2; exit 2 ;;
esac
