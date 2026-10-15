# Proposal

## Why

Windows servers whose Remote Desktop "Security Layer" is set to **RDP** (GPO "Require use of specific
security layer for remote (RDP) connections" = RDP, registry `SecurityLayer=0`) refuse TLS and NLA
and only speak the legacy Standard RDP Security layer (MS-RDPBCGR 5.3: RSA key exchange, RC4). IronRDP
0.17 does not implement it, so rustXterm cannot open these hosts: the user gets "negotiation failure:
server only supports Standard RDP Security" (reported against two Windows Server 2022 Standard hosts),
while MobaXterm connects because it embeds Microsoft's Terminal Services ActiveX client
(`MobaRDPTerminalServiceClient2`), which still implements the legacy layer. Users cannot always change
the server's policy, so rustXterm needs its own opt-in path on Linux and Windows.

## What Changes

- New per-session RDP option "Allow legacy RDP security (no TLS, insecure)", off by default, next to
  "Allow legacy TLS". Existing sessions keep it off.
- With the option on, a server that answers the TLS/NLA request with `SSL_NOT_ALLOWED_BY_SERVER` is
  reconnected with Standard RDP Security: client/server random exchange encrypted with the server's
  RSA key, RC4 session keys (40/56/128-bit), encrypted and MAC-checked slow-path and fast-path PDUs,
  key refresh every 4096 packets. No downgrade happens for any other failure, nor for a host that
  already has a trusted TLS certificate (the refusal is unauthenticated and could be forged).
- Trust prompt for the server's RSA public key (fingerprint shown, saved like a certificate, a changed
  key is refused), with a warning that the connection is not protected against interception.
- Servers with encryption level None or FIPS are refused with a clear error (credentials would travel
  in clear, or 3DES is not implemented).
- With the option off, the error says what to change: enable the option, or set the server's Security
  Layer to Negotiate/SSL (already implemented and tested, not yet committed).
- Receipt against the Windows Server 2022 Vagrant VM with `SecurityLayer=0`, from Linux and Windows
  clients.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `remote-desktop`: adds a requirement for opt-in legacy Standard RDP Security (fallback, trust prompt
  on the server key, refused encryption levels, actionable error when off).

## Impact

- `crates/rdp`: negotiation fallback, Standard RDP Security transform (security header, RC4, MAC,
  key refresh) on the wire, trust of the server RSA key, tests.
- `vendor/ironrdp-connector` and `vendor/ironrdp-pdu` (new vendored patches, listed in
  `vendor/README.md`): request `PROTOCOL_RDP`, advertise client encryption methods, no auto-detect or
  message channel in legacy mode, server certificates over 1024 bytes (bounded), licensing that reuses
  the GCC server key; removable if upstream adds the feature.
- `crates/core` (`RdpOptions::allow_standard_security`, serde default off), `crates/ui` (session editor
  checkbox, prompt warning), `crates/store` export round trip.
- Dependencies already in the lockfile: `md-5`, `sha1`, `num-bigint` (direct deps of `rxt-rdp`).
  Test-only oracle: a FreeRDP 3 harness in the Docker toolchain image for crypto vectors (not shipped).
- Docs: `docs/README.md` RDP section, `docs/gates/` receipt, `lab/vagrant/README.md` recipe for
  toggling `SecurityLayer` on the Windows VM.
