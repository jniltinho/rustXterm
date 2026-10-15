# Vendored crates

Third-party crates patched for this workspace through `[patch.crates-io]` in the root `Cargo.toml`.
They are excluded from the workspace (`exclude = ["vendor"]`), keep their upstream lints and licenses,
and differ from the published crate only as listed here.

## picky 7.0.0-rc.25 and sspi 0.21.3 (RDP, task 4.3)

Why: `ironrdp-connector` 0.10 (latest, used by ironrdp 0.17) pins `picky =7.0.0-rc.25` and `sspi ^0.21`.
picky rc.25 and sspi 0.21.3 pin `curve25519-dalek =5.0.0-rc.1` and other RustCrypto release candidates,
which cannot share one lockfile with russh 0.64 (`curve25519-dalek ^5`, stable). No ironrdp release on
crates.io uses picky >= rc.27 yet.

Change: `Cargo.toml` dependency requirements only; no source changes.

- `picky` (from crates.io `picky-7.0.0-rc.25.crate`): the `src/` tree is byte-identical to picky
  7.0.0-rc.27, whose manifest drops `curve25519-dalek`, `ecdsa`, `primeorder`, `rustcrypto-ff`,
  `rustcrypto-ff_derive`, `rustcrypto-group` (unused by the source) and relaxes `ed25519-dalek`/`x25519-dalek`
  to `3`, `p256`/`p384`/`p521` to `0.14` and `aes-gcm` to `0.11`. The same edits are applied here; the
  version stays `7.0.0-rc.25` so ironrdp's `=` requirement matches.
- `sspi` (from crates.io `sspi-0.21.3.crate`): removes the macOS/iOS-only dependencies
  `curve25519-dalek`, `ed25519-dalek`, `p256`, `p384`, `p521`, `primeorder`, `rustcrypto-ff`,
  `rustcrypto-ff_derive`, `rustcrypto-group` (pinned release candidates, not referenced by the source).
  `tests/`, `examples/`, `test_assets/` and `tools/` are not vendored.

Remove both patches once an ironrdp release depends on picky >= 7.0.0-rc.27 (and sspi >= 0.22);
verify with `scripts/cargo-docker.sh cargo tree -i curve25519-dalek` (one version).

## vnc-rs 0.6.0 (VNC, task 4.4)

Why: vnc-rs has no mid-message stall timeout and buffered up to 256 MiB of network data when the decoder
fell behind (design D13 limits).

Change (11 lines + one new file): `src/client/stall.rs` adds `StallGuard` (each read inside a started
message must progress within 30 s; idle time between messages is never timed out);
`src/client/connection.rs` wraps the decoder reader with it, marks message boundaries and sets
`NETWORK_CHANNEL_SIZE` to 64 (≤ 4 MiB buffered); `src/client/mod.rs` adds `pub mod stall`.

## wgpu-hal 30.0.1 (Windows port, add-windows-port W1 smoke)

Why: on GPU-less Windows machines (VirtualBox without 3D acceleration, such as the
Windows test VM) DXGI lists two "Microsoft Basic Render Driver" (WARP) adapters: a
non-software one that owns the display output and the software one without outputs. wgpu-hal
drops the first and renders on the second; DX12 flip presentation from that adapter leaves the
window black (desktop capture, `PrintWindow` and DirectComposition alike).

Change (3 lines + comment, `src/auxil/dxgi/factory.rs` `should_keep_adapter`): the non-software
WARP adapter is skipped only when it has no output (`EnumOutputs(0)` fails), so headless machines
keep upstream behavior. `examples/` are not vendored (their `[[example]]` entries are removed from
`Cargo.toml`). Remove the patch once wgpu keeps output-owning WARP adapters upstream.

## ironrdp-connector 0.10.0 and ironrdp-pdu 0.9.0 (opt-in Standard RDP Security, add-rdp-standard-security)

Why: IronRDP refuses servers whose Remote Desktop Security Layer is "RDP" (Standard RDP Security,
MS-RDPBCGR 5.3). rustXterm supports them opt-in per session; the RC4 layer itself lives in
`crates/rdp/src/legacy.rs` and runs on the transport, so the IronRDP changes stay small. Both crates
are copied from the crates.io packages (`src/`, `Cargo.toml`, licenses, README); every change is
marked with a `rustXterm:` comment.

- `ironrdp-connector` (`src/connection.rs`, `src/license_exchange.rs`):
  - a `Config` with neither `enable_tls` nor `enable_credssp` requests `PROTOCOL_RDP` instead of
    failing, and accepts it as the selected protocol;
  - in that mode the Client Security Data offers 40/56/128-bit RC4 (never FIPS), and neither network
    auto-detect nor the message channel is requested (their PDUs carry security headers the transform
    does not map);
  - `ClientConnector::server_security` keeps the Server Security Data (server random, certificate);
  - the licensing sequence uses that certificate when a Server License Request omits its own
    (MS-RDPELE 2.2.2.1).
  - TLS/NLA connections are byte-identical to upstream.
- `ironrdp-pdu` (`src/gcc/security_data.rs`): `MAX_SERVER_CERT_LEN` 1024 → 16 KiB, so X.509 chains
  in Server Security Data decode (still bounded; the transport PDU cap applies too).

Remove both patches once IronRDP supports Standard RDP Security upstream (or drops the 1024-byte
certificate cap and accepts `PROTOCOL_RDP`); verify with `crates/rdp/tests/vm_rdp.rs`
`standard_security` tests against the Windows VM.
