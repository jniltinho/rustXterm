#!/usr/bin/env python3
"""Probe an RDP server's security layer without authenticating.

Sends only the X.224 Connection Request with an RDP Negotiation Request (MS-RDPBCGR 2.2.1.1) and
reads the server's answer. No credentials are sent, nothing is guessed: this reports whether the
host needs TLS/NLA or only speaks the legacy Standard RDP Security layer, so you know whether to tick
"Allow legacy RDP security" in rustXterm.

    scripts/rdp-probe.py 192.168.10.91            # default port 3389
    scripts/rdp-probe.py host 3389 --timeout 5

Exit code 0 if the server answered, 1 on a connection/parse error.
"""
import argparse
import socket
import struct
import sys

# RDP Negotiation requestedProtocols we advertise (PROTOCOL_SSL | HYBRID | HYBRID_EX), like a client
# that prefers TLS/NLA. The server's reply tells us what it actually allows.
REQUESTED = 0x01 | 0x02 | 0x08

FAILURE = {
    1: ("SSL_REQUIRED_BY_SERVER", "server requires TLS; no legacy option needed"),
    2: ("SSL_NOT_ALLOWED_BY_SERVER", 'server only allows Standard RDP Security -> tick "Allow legacy RDP security"'),
    3: ("SSL_CERT_NOT_ON_SERVER", "server has no valid certificate for TLS"),
    4: ("INCONSISTENT_FLAGS", "inconsistent negotiation flags"),
    5: ("HYBRID_REQUIRED_BY_SERVER", "server requires NLA/CredSSP (TLS + NLA); no legacy option needed"),
    6: ("SSL_WITH_USER_AUTH_REQUIRED_BY_SERVER", "server requires TLS with a client certificate"),
}
PROTOCOL = {0: "RDP (Standard RDP Security, no TLS)", 1: "TLS (SSL)", 2: "HYBRID (TLS + NLA/CredSSP)", 8: "HYBRID_EX"}


def connection_request() -> bytes:
    """TPKT + X.224 CR carrying an RDP_NEG_REQ for REQUESTED."""
    neg = struct.pack("<BBHI", 0x01, 0x00, 0x0008, REQUESTED)  # type, flags, length, requestedProtocols
    x224 = struct.pack("<BBHHB", 0x0E, 0xE0, 0x0000, 0x0000, 0x00) + neg  # CR TPDU, no cookie
    tpkt = struct.pack(">BBH", 0x03, 0x00, 4 + len(x224))
    return tpkt + x224


def parse(reply: bytes) -> str:
    if len(reply) < 11 or reply[0] != 0x03:
        raise ValueError(f"not a TPKT/X.224 reply ({reply[:16].hex()})")
    body = reply[11:]  # skip TPKT (4) + X.224 CC fixed part (7)
    if len(body) < 8:
        return "server accepted but sent no RDP negotiation response (very old server; likely RDP-only)"
    kind, _flags, _len, value = struct.unpack_from("<BBHI", body, 0)
    if kind == 0x02:
        return f"OK  selectedProtocol = {PROTOCOL.get(value, hex(value))}"
    if kind == 0x03:
        name, hint = FAILURE.get(value, (f"unknown(0x{value:x})", ""))
        return f"NEG_FAILURE {name}: {hint}"
    return f"unexpected negotiation type 0x{kind:02x}"


def probe(host: str, port: int, timeout: float) -> str:
    with socket.create_connection((host, port), timeout) as sock:
        sock.settimeout(timeout)
        sock.sendall(connection_request())
        reply = sock.recv(1024)
    if not reply:
        raise ValueError("server closed the connection without replying")
    return parse(reply)


def _selftest() -> None:
    # A built Negotiation Response selecting HYBRID round-trips through parse().
    cc = struct.pack(">BBH", 0x03, 0x00, 19) + struct.pack("<BBHHB", 0x0E, 0xD0, 0, 0x1234, 0)
    assert "HYBRID" in parse(cc + struct.pack("<BBHI", 0x02, 0x00, 8, 0x02)), "response parse"
    assert "SSL_NOT_ALLOWED" in parse(cc + struct.pack("<BBHI", 0x03, 0, 8, 2)), "failure parse"
    assert connection_request()[0] == 0x03 and len(connection_request()) == 19, "request framing"
    print("selftest ok")


def main() -> int:
    ap = argparse.ArgumentParser(description="Probe an RDP server's security layer (no credentials sent).")
    ap.add_argument("host", nargs="?", help="RDP host")
    ap.add_argument("port", nargs="?", type=int, default=3389)
    ap.add_argument("--timeout", type=float, default=5.0)
    ap.add_argument("--selftest", action="store_true", help="run the offline self-check and exit")
    args = ap.parse_args()
    if args.selftest:
        _selftest()
        return 0
    if not args.host:
        ap.error("host is required")
    print(f"probing {args.host}:{args.port} (no credentials) ...")
    try:
        print(probe(args.host, args.port, args.timeout))
    except (OSError, ValueError) as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
