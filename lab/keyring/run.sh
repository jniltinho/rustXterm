#!/bin/sh
# Keyring test (task 7.1): builds the rxt-platform-linux test binary in the toolchain image,
# then runs the Secret Service tests in a throwaway dbus + gnome-keyring container.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/lab}"
bin=$("$root/scripts/cargo-docker.sh" cargo test -p rxt-platform-linux --features keyring-test --lib --no-run --message-format=json \
    | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -n 1)
[ -n "$bin" ] || { echo "test binary not found" >&2; exit 1; }
docker build -q -t rxt-lab-keyring:dev "$root/lab/keyring" >/dev/null
docker run --rm --network none --user "$(id -u):$(id -g)" -v "$root:/work:ro" -w /work \
    rxt-lab-keyring:dev "$bin"
