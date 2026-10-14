#!/bin/sh
# Runs inside a desktop VM (see create-vm.sh) over SSH:
#   sh vm-run.sh start <binary> wayland|x11|default [env...] start rustXterm in the logged-in session
#   sh vm-run.sh stop [keep]                                 SIGTERM it, print its log (keep: leave its dirs)
#   sh vm-run.sh env                                         print the session environment (for `eval`)
# wayland: WAYLAND_DISPLAY only (DISPLAY unset), so winit must use Wayland natively.
# x11: DISPLAY only (Xorg session, or Xwayland under a Wayland compositor).
# default: the session environment unchanged (both set under Wayland: how users launch it).
# Scratch XDG config/data dirs keep runs independent; agent control is on for the CLI checks.
set -eu
session_env() {
    # Environment of a client of the graphical session (the compositor itself lacks WAYLAND_DISPLAY).
    # Newest first, so a restarted session wins over leftovers of the previous one.
    for p in $(pgrep -u "$(id -un)" | sort -rn); do
        e=$(tr '\0' '\n' 2>/dev/null < /proc/$p/environ) || continue
        printf '%s\n' "$e" | grep -q '^WAYLAND_DISPLAY=\|^DISPLAY=' || continue
        printf '%s\n' "$e" | grep -E '^(DISPLAY|WAYLAND_DISPLAY|XAUTHORITY|XDG_RUNTIME_DIR|DBUS_SESSION_BUS_ADDRESS|XDG_CURRENT_DESKTOP|XDG_SESSION_TYPE|XDG_SESSION_DESKTOP)='
        return 0
    done
    echo "no graphical session found" >&2; return 1
}
state="/run/user/$(id -u)/rxt-last"
case "${1:?start|stop|env}" in
env) session_env | sed 's/^/export /' ;;
start)
    bin=$2 mode=$3; shift 3
    vars=$(session_env)
    while IFS= read -r var; do export "$var"; done <<VARS
$vars
VARS
    case "$mode" in wayland) unset DISPLAY ;; x11) unset WAYLAND_DISPLAY ;; esac
    for kv in "$@"; do export "$kv"; done
    # RXT_SCRATCH=<dir from a previous run> reuses its config and data (restart checks).
    scratch=${RXT_SCRATCH:-$(mktemp -d /tmp/rxt.XXXXXXXX)}; mkdir -p "$scratch/c" "$scratch/d"
    XDG_CONFIG_HOME=$scratch/c XDG_DATA_HOME=$scratch/d RXT_AGENT_CONTROL=1 RUST_LOG=${RUST_LOG:-info} \
        setsid "$bin" > "$scratch/app.log" 2>&1 < /dev/null &
    pid=$!
    sleep 8
    echo "$pid $scratch" > "$state"
    alive=no; kill -0 $pid 2>/dev/null && alive=yes
    echo "mode=$mode desktop=${XDG_CURRENT_DESKTOP:-} session=${XDG_SESSION_TYPE:-} wayland=${WAYLAND_DISPLAY:-} display=${DISPLAY:-} pid=$pid alive=$alive"
    if [ "$mode" = x11 ]; then
        w=""
        for c in $(xwininfo -root -tree 2>/dev/null | grep '"rustXterm"' | awk '{print $1}'); do
            [ "$(xprop -id "$c" _NET_WM_PID 2>/dev/null | grep -o '[0-9]*$')" = "$pid" ] && w=$c && break
        done
        echo "x11_window=${w:-none}"
        [ -n "$w" ] && xprop -id "$w" WM_CLASS && echo "icon=$(xprop -id "$w" -len 8 _NET_WM_ICON | grep -o 'Icon ([0-9]* x [0-9]*)' | head -1)"
    fi
    echo "--- log"; cat "$scratch/app.log" ;;
stop)
    read -r pid scratch < "$state"
    kill -TERM "$pid" 2>/dev/null || true
    sleep 2
    kill -0 "$pid" 2>/dev/null && echo "still running" || echo "exited"
    echo "--- log"; cat "$scratch/app.log"
    [ "${2:-}" = keep ] || case "$scratch" in /tmp/rxt.*) rm -rf "$scratch" ;; esac
    rm -f "$state" ;;
esac
