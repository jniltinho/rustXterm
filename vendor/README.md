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
