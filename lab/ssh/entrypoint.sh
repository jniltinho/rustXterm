#!/bin/sh
# Fresh host keys per container; bob's public key comes from lab/.generated (mounted at /lab).
set -eu
mkdir -p /run/sshd
ssh-keygen -A >/dev/null
install -o root -g root -m 0644 /lab/bob_ed25519.pub /etc/ssh/authorized_keys/bob
mkdir -p /lab/logs
umask 022
exec /usr/sbin/sshd -D -e 2>>"/lab/logs/$(hostname).log"
