# Design

## Context

See proposal.md for the motivation. Current state:

- `crates/rdp/src/session.rs::establish` drives IronRDP's `ClientConnector` over `Wire` (the guarded
  framing in `wire.rs`: size caps, stall timeout). After X.224 it always upgrades to TLS
  (`tls::upgrade`), asks the trust decision on the leaf certificate, fills the password, runs CredSSP
  when selected, then steps the connector to the active stage. `Driver` keeps using `Wire` for the
  whole session.
- `ironrdp-connector` 0.10 refuses Standard RDP Security twice: it errors before sending a request
  without TLS/CredSSP, and it requires `selected.intersects(requested)`, which is false for
  `PROTOCOL_RDP` (value 0). It hard-codes `ClientSecurityData` with no encryption methods.
- IronRDP already emits the basic security header where Enhanced RDP Security needs one (Client Info
  PDU with `SEC_INFO_PKT`, licensing PDUs with `SEC_LICENSE_PKT`, auto-detect PDUs on the message
  channel); all other PDUs have none. Its RC4/RSA helpers (`ironrdp_pdu::crypto`) are `pub(crate)`.
- `ironrdp-pdu` rejects GCC server certificates over 1024 bytes (`MAX_SERVER_CERT_LEN`), so valid
  X.509 chains fail to decode; its licensing constructors (`ClientNewLicenseRequest`,
  `ClientLicenseInfo`) fail when `SERVER_LICENSE_REQUEST` omits the certificate, which MS-RDPELE
  2.2.2.1 allows (the client must then use the GCC server certificate).
- The connector advertises `SUPPORT_NET_CHAR_AUTODETECT` and requests the message channel.
- Uncommitted: `connector_err` maps `SSL_NOT_ALLOWED_BY_SERVER` to an actionable error, covered by
  `hostile_rdp::standard_rdp_security_only_gives_actionable_error`.
- Windows Server 2022 Vagrant VM (`lab/vagrant/windows`) reproduces the failure with
  `SecurityLayer=0`, `UserAuthentication=0` (verified from Linux and Windows clients).

## Goals / Non-Goals

**Goals:**
- Connect to `SecurityLayer=0` servers with encryption level Low, Client Compatible or High
  (RC4 40/56/128-bit), on Linux and Windows, without changing the TLS/NLA path.
- Keep the existing guards (size caps, stall timeout, trust before credentials) for legacy sessions.
- Keep the vendored IronRDP patch small enough to drop when upstream supports the layer.

**Non-Goals:**
- FIPS encryption (3DES/SHA-1 HMAC, MS-RDPBCGR 5.3.6.1.1): refused.
- Encryption level None: refused (the password would travel in clear).
- Bulk compression, auto-reconnect cookies, server redirection under legacy security.
- Real server authentication: the proprietary certificate is signed with a published key, so the
  layer cannot authenticate the server; the trust prompt is TOFU only.

## Decisions

### D1. Opt-in option and reconnect-on-refusal
`RdpOptions::allow_standard_security` (serde default `false`). The first attempt is unchanged
(TLS|HYBRID). Only when the X.224 confirm is `RDP_NEG_FAILURE` with `SSL_NOT_ALLOWED_BY_SERVER` and
the option is on, `establish` opens a new TCP connection requesting `PROTOCOL_RDP`. This matches what
`mstsc` does. The refusal arrives before any authentication, so an on-path attacker can forge it
and force legacy mode on any opted-in session; the option label and docs say so. Extra guard: when
the host:port already has a trusted TLS certificate in `CertTrust`, the refusal is not acted on and
the error names a possible downgrade (the user removes the trusted certificate if the server really
changed). A forged-refusal test covers both cases.
*Alternatives:* request `PROTOCOL_RDP` first when the option is on (always weakest, and the option
would downgrade servers that support TLS); auto-fallback without an option (silent downgrade).

### D2. Security transform inside `Wire`, small vendored patches
A `legacy::Security` state lives in `Wire` (`Option<Security>`): `write` encrypts outgoing PDUs,
`read_pdu`/`read_by_hint` verify and decrypt incoming ones and rewrite the TPKT/PER (slow-path) or
fast-path lengths, so the connector and `ActiveStage` see the Enhanced-Security byte layout they
already parse. Normalization is per PDU kind and direction, decided from the MCS channel and the
header flags, never blanket. Outbound PDUs after the Security Exchange are always encrypted (every
non-None level encrypts client-to-server). Inbound PDUs follow their own flags: slow-path with
`SEC_ENCRYPT` and fast-path with `FASTPATH_OUTPUT_ENCRYPTED` are MAC-checked and decrypted; without
them (level Low sends server-to-client traffic in clear behind a basic security header) the header
is stripped, nothing is verified and no crypto counter advances. At levels Client Compatible and
High an inbound data PDU without encryption is a protocol error.

| PDU (direction) | Enhanced-Security layout IronRDP expects | Legacy transform |
|---|---|---|
| Client Info (out) | header `SEC_INFO_PKT` | keep header, add `SEC_ENCRYPT` + MAC, encrypt |
| Security Exchange (out) | absent | injected before Client Info (client random, RSA) |
| Licensing (both) | header `SEC_LICENSE_PKT` | keep header; decrypt inbound only when `SEC_ENCRYPT`; outbound encryption per D2b |
| Other slow-path on I/O and static channels (both) | no header | out: add header with `SEC_ENCRYPT` + MAC; in: strip header, decrypt only when flagged |
| Fast-path (both) | no signature, encryption flag clear | out: set `FASTPATH_INPUT_ENCRYPTED`, add MAC; in: when flagged, strip MAC and decrypt, clear flag |
| Auto-detect / message channel | n/a in legacy mode | not advertised; any such PDU is a protocol error |

Vendored patches (`[patch.crates-io]`, each listed in `vendor/README.md` with removal criteria):
- `ironrdp-connector`: a `Config` with neither TLS nor CredSSP enabled requests `PROTOCOL_RDP`
  (no new config field), accepts it as selected, advertises 40/56/128-bit methods (never FIPS), and in
  that mode drops `SUPPORT_NET_CHAR_AUTODETECT` and the message-channel request; it keeps the Server
  Security Data in `ClientConnector::server_security`, and the licensing sequence fills a Server
  License Request without certificate with the GCC certificate before IronRDP's constructors see it.
- `ironrdp-pdu`: `MAX_SERVER_CERT_LEN` raised to 16 KiB (still bounded; the PDU cap in `Wire` also
  applies).

**D2b. Licensing encryption.** Flags come from MS-RDPBCGR 2.2.8.1.1.2.1 (`SEC_LICENSE_ENCRYPT_CS`
and `_SC` are both 0x0200; IronRDP's `LICENSE_ENCRYPT_CS = 0x0100` is wrong but never reaches the
wire, because the transform rewrites inbound license headers to a plain `SEC_LICENSE_PKT` and builds
outbound headers itself). The client follows FreeRDP's client: the Security Exchange PDU carries
`SEC_LICENSE_ENCRYPT_SC` (the client can decrypt licensing PDUs) and client licensing PDUs are sent
encrypted with `SEC_LICENSE_ENCRYPT_CS`. Inbound license PDUs are decrypted only when `SEC_ENCRYPT` is
set and may arrive in clear at any level.

*Alternatives for normalization of the certificate:* extract and blank the GCC certificate in `Wire`
before the connector decodes it (rewrites nested PER/GCC lengths; more fragile than a one-constant
patch).
*Alternatives:* vendor and patch `ironrdp-connector` and `ironrdp-session` throughout (security
headers in every encoder/decoder; large diff that is hard to rebase); write our own connector
(duplicates capability exchange, licensing and channels).

### D3. Crypto in `crates/rdp/src/legacy.rs`, no new crates, independent oracle
Key derivation per MS-RDPBCGR 5.3.5 (pre-master, master secret, session key blob, MD5/SHA-1
salted hashes, 40/56-bit key reduction), MAC per 5.3.6.1, key update every 4096 packets per 5.3.7,
RSA raw encryption of the client random (little-endian, `num-bigint` modpow). RC4 is ~20 lines in
the module. `md-5`, `sha1`, `num-bigint` are already in the lockfile and become direct deps; the
32-byte client random comes from the workspace `rand` (OS RNG). Certificates: proprietary (RSA public key
blob) and X.509 chain (leaf `subjectPublicKeyInfo`, parsed with `x509-cert`, already a dependency of `rxt-rdp`);
modulus over 4096 bits or an unknown certificate type is a protocol error.

RSA follows the serialization rules: little-endian ciphertext of modulus width plus eight trailing
zero bytes; MS-RDPBCGR 4.8 is the RSA vector. MS-RDPBCGR has no session-key or MAC vectors, so the
fixed vectors for 40/56/128-bit key derivation, both MAC variants (salted MAC keeps the cumulative
packet count across slow-path and fast-path) and directional key updates come from FreeRDP 3's
implementation (`security.c`) through a small test-only C harness built in the Docker toolchain
image; the values are frozen in the Rust tests, so FreeRDP is not needed at test time. The Windows
VM receipt confirms them end to end.

### D4. Trust of the server key
The fingerprint is SHA-256 over the RSA public key (exponent and modulus, big-endian) and goes
through the existing `CertTrust` store keyed by host:port, with the prefix `RSA ` so a legacy key
never matches a TLS certificate entry. No new prompt type: `PromptKind::CertificateUnknown` carries
the warning "Legacy RDP security (no TLS): this connection is not protected against interception" as
its subject (so CLI/MCP prompts show it unchanged), and the UI recognises the `RSA ` prefix to title
the dialog "Unknown server key" and show the warning in the danger color. Password is asked only after the decision, as
today, and sent in the encrypted Client Info PDU (auto-logon).

### D5. Errors
Option off → existing actionable error, extended to name the option. Level None or method FIPS in
`ServerSecurityData` → `Error::Protocol` naming the level, raised before the Security Exchange PDU.
MAC mismatch, short security header or wrong flags → `Error::Protocol`, session ends.

## Risks / Trade-offs

- [Legacy layer has no real server authentication; RC4 is broken; the refusal can be forged to
  downgrade an opted-in session] → opt-in per session, warning in the option label, docs and every
  trust prompt, TOFU key pinning, no fallback for hosts with a trusted TLS certificate.
- [Length rewriting in `Wire` could desync framing] → unit tests on encode/decode round trips for
  slow-path and fast-path PDUs, plus the existing size caps run on the decrypted length too.
- [Crypto mistakes are silent against our own fake server] → vectors from an independent
  implementation (FreeRDP harness) and MS-RDPBCGR 4.8 for RSA; the Windows VM receipt is the
  end-to-end proof.
- [Vendored connector drifts from upstream] → patch kept to a few lines, listed in
  `vendor/README.md` with removal criteria.
- [Licensing under legacy security differs (encryption capability, omitted certificate)] → D2b
  rules with unit tests for each case; the VM receipt covers only the grace-period path.

## Migration Plan

Additive: the option defaults to off, old configs deserialize unchanged, export/import round-trips
the field. Rollback is removing the option; sessions that used it fall back to the actionable error.

## Open Questions

- Whether `SEC_SECURE_CHECKSUM` is set by Windows Server 2022 in practice (both MACs are implemented
  and tested; the receipt run did not log it).
- Saved certificates cannot be removed from the UI or CLI yet; the downgrade error therefore points
  to connecting by another host name. A "forget certificate" command is a separate change.
