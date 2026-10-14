#!/bin/sh
# $1 = test binary. Runs the no-bus test without a session bus, then the Secret Service test in
# a private session bus with gnome-keyring whose HOME/data live outside the test's $HOME.
set -eu
bin=$1
t=$(mktemp -d)
mkdir -p "$t/home" "$t/daemon" "$t/run"
chmod 700 "$t/run"
export XDG_RUNTIME_DIR="$t/run"
env -u DBUS_SESSION_BUS_ADDRESS HOME="$t/home" "$bin" --exact keyring::tests::no_session_bus_is_unsupported_quickly
exec dbus-run-session -- sh -c '
    printf %s lab-keyring-pass | HOME="$1/daemon" XDG_DATA_HOME="$1/daemon/data" \
        gnome-keyring-daemon --unlock --components=secrets >/dev/null
    HOME="$1/home" "$2" --exact keyring::tests::secret_service_round_trip_writes_no_files
' sh "$t" "$bin"
