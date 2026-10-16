#!/usr/bin/env bash
# Validate one RDP login with rdesktop, headless. For your own / authorised hosts only.
#
# It is deliberately ONE credential per run (no password lists, no loops): pass the user and the
# password you want to check. First it probes the security layer (no credentials), then it runs
# rdesktop under a virtual X server and reports whether the server accepted the login.
#
#   scripts/rdp-test.sh <host> <user> <password> [domain] [port]
#   scripts/rdp-test.sh 192.168.10.91 administrator 's3cret' criarenet.net
#   scripts/rdp-test.sh 192.168.10.91 administrator 's3cret' .            # local account
#
# Needs: rdesktop, and Xvfb (headless) or a reachable $DISPLAY. The password may instead come from
# the RDP_PASSWORD environment variable (pass "-" as the password argument) to keep it off the
# command line and shell history.
set -euo pipefail

die() { echo "error: $*" >&2; exit 2; }
[ $# -ge 3 ] || die "usage: $0 <host> <user> <password|-> [domain] [port]"
host=$1 user=$2 pass=$3 domain=${4:-} port=${5:-3389}
[ "$pass" = - ] && pass=${RDP_PASSWORD:?pass '-' means set RDP_PASSWORD}
command -v rdesktop >/dev/null || die "rdesktop not installed (apt-get install rdesktop)"
here=$(cd "$(dirname "$0")" && pwd)

echo "== 1. security layer (no credentials)"
if command -v python3 >/dev/null && [ -f "$here/rdp-probe.py" ]; then
  python3 -I "$here/rdp-probe.py" "$host" "$port" || true
else
  timeout 5 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null \
    && echo "port $port open" || echo "port $port unreachable"
fi

echo "== 2. login as ${domain:+$domain\\}$user"
# rdesktop needs an X server; use a throwaway virtual one when there is no $DISPLAY.
run_rdesktop() {
  # -a 16 colour depth, small geometry, connect and disconnect after login so nothing is interactive.
  timeout 25 rdesktop -g 640x480 -a 16 ${domain:+-d "$domain"} -u "$user" -p "$pass" \
    "$host:$port" 2>&1 || true
}
if [ -n "${DISPLAY:-}" ]; then
  out=$(run_rdesktop)
elif command -v xvfb-run >/dev/null; then
  out=$(xvfb-run -a bash -c "$(declare -f run_rdesktop); run_rdesktop" \
        host="$host" port="$port" user="$user" pass="$pass" domain="$domain" 2>&1) || true
else
  die "no DISPLAY and no xvfb-run (apt-get install xvfb) — run on a desktop or install Xvfb"
fi

echo "$out" | sed 's/^/   rdesktop: /'
case "$out" in
  *"CredSSP"*|*"LOGON_FAILURE"*|*"Authentication failure"*|*"ERRINFO"*)
    echo "RESULT: reachable, but the login was REJECTED (wrong user/password/domain)"; exit 1 ;;
  *"only supports Standard RDP"*|*"negotiation"*)
    echo "RESULT: server uses legacy Standard RDP Security (tick 'Allow legacy RDP security' in rustXterm)"; exit 1 ;;
  *"Connection established"*|*"Disconnect"*|*"[not] connected"*|"")
    echo "RESULT: login ACCEPTED — connection validated"; exit 0 ;;
  *"unable to connect"*|*"Connection refused"*|*timed*out*|*"Failed to resolve"*)
    echo "RESULT: could not reach the server (network/firewall/host)"; exit 1 ;;
  *) echo "RESULT: see the rdesktop output above"; exit 1 ;;
esac
