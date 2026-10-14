#!/bin/sh
# shellcheck disable=SC2016 # container scripts are single-quoted on purpose: they expand inside the container
# Distribution matrix (task 10.2): runs the release binary's and the AppImage's `--self-test` in clean
# containers, then a GUI smoke under Xvfb with Mesa software rendering. Needs only Docker.
#
#   scripts/distro-matrix.sh [DIST_DIR]     # default dist/rc; built with `--target dist` if empty
#   RXT_VERSION=0.1.0 scripts/distro-matrix.sh
#   RXT_MATRIX_GUI=0 scripts/distro-matrix.sh   # self-tests only
#
# Images are pinned by digest (refresh: `docker buildx imagetools inspect <tag>`).
# Self-tests run with --network none; the GUI smoke needs the network for apt only.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-$root/dist/rc}

IMAGES="
ubuntu:20.04@sha256:8feb4d8ca5354def3d8fce243717141ce31e2c428701f6682bd2fafe15388214
ubuntu:22.04@sha256:5ec03bb3441e8b0bf3b4f9cd4629a1ae763010dc3035bb8da3ae6cf026486401
ubuntu:24.04@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55
ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7
debian:12@sha256:2c037a04925515fdd6ea85ea14a682d0e79931f5e9f5d07b6dbfc6ba12f9e858
debian:13@sha256:913f6706df59a68922d1dd08f78c2476560a8d367897200a6005b00e5f67c2d5
fedora:latest@sha256:43b29f65a41eb9c35e1cd5323e3bdf3b655c2357a9f4f1ff2f9c2798e5045d80
archlinux:latest@sha256:3c64b538588416593f69a1aebd2f2ec0f2541b29159f32556fa7def2167147a0
"
GUI_IMAGES="
ubuntu:24.04@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55
ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7
"

if ! ls "$dir"/rustxterm-*-x86_64.AppImage >/dev/null 2>&1; then
    docker build --target dist ${RXT_VERSION:+--build-arg RXT_VERSION="$RXT_VERSION"} \
        --output type=local,dest="$dir" "$root"
fi
dir=$(cd "$dir" && pwd)
bin=$(basename "$(ls "$dir"/rustxterm-*-x86_64)")
app=$(basename "$(ls "$dir"/rustxterm-*-x86_64.AppImage)")

# One container per image: glibc version, portable binary, AppImage (extracted: no FUSE in containers).
selftest='set -e
cd /tmp
printf "glibc: "; (ldd --version 2>&1 || true) | head -n 1
/dist/$BIN --self-test
/dist/$APP --appimage-extract >/dev/null
squashfs-root/AppRun --self-test'

# Window must appear and the process must stay up (wgpu on lavapipe, X11 through Xvfb).
gui='set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb x11-utils mesa-vulkan-drivers libvulkan1 libegl1 libgl1 \
    libxkbcommon-x11-0 libx11-xcb1 libxcursor1 libxrandr2 libxi6 libwayland-client0 >/dev/null
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
sleep 2
export DISPLAY=:99 HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/run WGPU_BACKEND=vulkan
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"; chmod 0700 "$XDG_RUNTIME_DIR"
/dist/$BIN >/tmp/app.log 2>&1 &
pid=$!
for _ in $(seq 1 30); do
    xwininfo -root -tree | grep -q "\"rustXterm\"" && break
    kill -0 $pid 2>/dev/null || { cat /tmp/app.log; exit 1; }
    sleep 1
done
xwininfo -root -tree | grep "\"rustXterm\"" || { echo "no rustXterm window"; cat /tmp/app.log; exit 1; }
sleep 10
kill -0 $pid || { echo "exited after mapping the window"; cat /tmp/app.log; exit 1; }
kill $pid; wait $pid 2>/dev/null || true
echo "gui smoke ok"'

fail=0
results=""
run() { # label image script [extra docker args]
    label=$1 image=$2 script=$3; shift 3
    echo "== $label: ${image%@*}"
    if docker run --rm --pull missing "$@" -v "$dir:/dist:ro" -e BIN="$bin" -e APP="$app" \
        "$image" sh -c "$script"; then
        results="$results
$label ${image%@*} pass"
    else
        results="$results
$label ${image%@*} FAIL"
        fail=1
    fi
}

for image in $IMAGES; do run self-test "$image" "$selftest" --network none; done
if [ "${RXT_MATRIX_GUI:-1}" = 1 ]; then
    for image in $GUI_IMAGES; do run gui-smoke "$image" "$gui"; done
fi

echo "== results ($bin)$results"
exit $fail
