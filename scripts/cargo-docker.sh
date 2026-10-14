#!/bin/sh
# Run cargo (or any command) inside the pinned toolchain image.
# base/ and docs/prints/ are masked with empty tmpfs mounts; set RXT_MOUNT_BASE=1 only for `xtask guard`
# and RXT_MOUNT_PRINTS=1 only for `xtask prints`.
# Parallel agents set CARGO_TARGET_DIR=target/agent-<name>.
# RXT_TOOLCHAIN_STAGE=windows-toolchain uses the image with the x86_64-pc-windows-gnu cross toolchain.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
stage=${RXT_TOOLCHAIN_STAGE:-toolchain}
default_image=rustxterm-toolchain:dev
[ "$stage" = toolchain ] || default_image="rustxterm-$stage:dev"
image=${RXT_TOOLCHAIN_IMAGE:-$default_image}
if ! docker image inspect "$image" >/dev/null 2>&1; then
    docker build --target "$stage" -t "$image" "$root"
fi
prints="--mount type=tmpfs,destination=/work/docs/prints"
if [ "${RXT_MOUNT_PRINTS:-0}" = 1 ]; then
    mkdir -p "$root/docs/prints"
    prints="--mount type=bind,source=$root/docs/prints,destination=/work/docs/prints"
fi
mask="--mount type=tmpfs,destination=/work/base $prints"
[ "${RXT_MOUNT_BASE:-0}" = 1 ] && mask="--mount type=bind,source=$root/base,destination=/work/base,readonly $prints"
[ $# -eq 0 ] && set -- cargo build --workspace
net=${RXT_DOCKER_NET:-bridge}
tty=""; [ -t 0 ] && tty="-t"
# RXT_DOCKER_CPUS limits the container CPUs (gate measurements on a 4-core reference).
cpus=""; [ -n "${RXT_DOCKER_CPUS:-}" ] && cpus="--cpus=$RXT_DOCKER_CPUS"
# shellcheck disable=SC2086
exec docker run --rm -i $tty $cpus --network "$net" --user "$(id -u):$(id -g)" \
    -v "$root:/work" $mask -w /work \
    -e HOME=/work/target/home -e CARGO_HOME=/work/target/cargo-home \
    -e CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/docker}" \
    -e CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}" -e RUST_BACKTRACE=1 -e RUST_LOG -e INSTA_UPDATE -e UPDATE_SNAPSHOTS \
    "$image" "$@"
