#!/bin/sh
# Task 9.1: other-UID rejection on the agent control socket, with a real second UID.
# Host-side (needs Docker): builds the probe in the toolchain image, runs a headless engine as
# the invoking user in one container and clients in others sharing the socket directory:
#   1. UID 12345, default modes      -> cannot reach the socket (directory 0700)
#   2. same UID                      -> served (positive control)
#   3. socket dir 0755 + socket 0666 -> UID 12345 connects and is closed without a response
#      (SO_PEERCRED check), and the engine audits nothing for it.
# Usage: lab/agent/uid-test.sh   (exit 0 = all assertions hold)
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
image=${RXT_TOOLCHAIN_IMAGE:-rustxterm-toolchain:dev}
target=target/lab
CARGO_TARGET_DIR=$target "$root/scripts/cargo-docker.sh" cargo build -p rustxterm --example agent_uid_probe
probe=/work/$target/debug/examples/agent_uid_probe
work=$target/uid-test
rm -rf "${root:?}/$work"
mkdir -p "$root/$work"
server=rxt-uid-server-$$
run() { # run <uid:gid> <args...>
    user=$1; shift
    docker run --rm --network none --user "$user" -v "$root:/work" -w /work "$image" "$probe" "$@"
}
cleanup() { docker rm -f "$server" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$server" --network none --user "$(id -u):$(id -g)" -v "$root:/work" -w /work \
    "$image" "$probe" serve "/work/$work" >/dev/null
i=0
until [ -f "$root/$work/ready" ]; do
    i=$((i + 1)); [ $i -gt 100 ] && { docker logs "$server"; echo "uid-test: server not ready" >&2; exit 1; }
    sleep 0.1
done
sock="$root/$work/run/rustxterm/control.sock"
echo "socket: $(stat -c '%a %u' "$root/$work/run/rustxterm") dir, $(stat -c '%a %u' "$sock") socket"

fail=0
expect() { # expect <label> <wanted exit> <uid:gid> <client>
    set +e; out=$(run "$3" connect "/work/$work" "$4"); rc=$?; set -e
    if [ "$rc" = "$2" ]; then echo "ok    $1: $out"; else echo "FAIL  $1: exit $rc, $out"; fail=1; fi
}
expect "uid 12345, default modes" 10 12345:12345 other-uid-default
expect "same uid $(id -u)" 0 "$(id -u):$(id -g)" same-uid
chmod 0755 "$root/$work/run/rustxterm"
chmod 0666 "$sock"
expect "uid 12345, loosened modes" 11 12345:12345 other-uid-loosened

touch "$root/$work/done"
docker wait "$server" >/dev/null
audit=$(docker logs "$server" 2>&1)
echo "$audit"
if [ "$(echo "$audit" | grep -c '^audit ')" = 1 ] && echo "$audit" | grep -q '^audit same-uid tabs.list allowed$'; then
    echo "ok    audit holds only the same-uid request"
else
    echo "FAIL  audit"; fail=1
fi
rm -rf "${root:?}/$work"
[ $fail = 0 ] && echo "uid-test: pass" || { echo "uid-test: FAIL" >&2; exit 1; }
