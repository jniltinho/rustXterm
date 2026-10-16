# Standard RDP Security receipt (add-rdp-standard-security)

Opt-in Standard RDP Security ("Allow legacy RDP security") against a real Windows host, from the Linux
and the Windows client. User guide: [`docs/rdp.md`](../rdp.md).

## Run — 2026-10-15

- Source: working tree on top of `c0489fc` (change uncommitted at the time of the run).
- VM: the Windows Server 2022 test VM (`lab/vagrant/windows`, box
  `gusztavvargadr/windows-server-2022-standard` 2601.0.0), RDP forwarded to 127.0.0.1:53389,
  fictional `vagrant` account. Security layer switched with the recipe in `lab/vagrant/README.md`.
- Linux client: `cargo test -p rxt-rdp --features windows-vm --test vm_rdp -- --ignored` in the
  toolchain image (`RXT_DOCKER_NET=host`).
- Windows client: the same test executable cross-built with the `windows-toolchain` image and run
  inside the VM, with a temporary `netsh interface portproxy` 127.0.0.1:53389 → 127.0.0.1:3389
  (removed after the run).
- Reported failure reproduced first: with `SecurityLayer=0` the previous build failed with
  "negotiation failure: server only supports Standard RDP Security" (same as the two Windows Server
  2022 Standard hosts reported by the user).

| Check | Server setting | Linux | Windows |
|---|---|---|---|
| Option on: legacy key prompt (`RSA …` fingerprint, interception warning) before the password, login, first frame, mouse and keys, 4200 client packets (RC4 key update every 4096), display-control resize to 1280×720 (`rdp_windows_standard_security_login_input_and_resize`) | `SecurityLayer=0`, `UserAuthentication=0`, `MinEncryptionLevel=2` (Client Compatible) | PASS | PASS |
| Same test | `MinEncryptionLevel=1` (Low: server-to-client in clear) | PASS | — |
| Same test | `MinEncryptionLevel=3` (High) | PASS | — |
| `MACHINE\user` and `.\user` in the user field, Domain empty (`rdp_windows_machine_backslash_user_logs_in`) | NLA, and `SecurityLayer=0` | PASS (both) | — |
| Option off: actionable error naming the option and the server fix, no prompt (`rdp_windows_standard_security_off_is_actionable`) | `SecurityLayer=0` | PASS | PASS |
| No regression: NLA login, input, resize; wrong password fails in CredSSP (`rdp_windows_nla_*`) | `SecurityLayer=1`, `UserAuthentication=1` | PASS | — |
| TLS without NLA | `SecurityLayer=2`, `UserAuthentication=0` | PASS (before the change; TLS path unchanged) | PASS (before the change) |

Offline checks (`cargo test -p rxt-rdp`):

- `legacy::tests`: session keys, plain and salted MACs, and ciphertexts across the 4096-packet key
  update for 40/56/128-bit match vectors from `crates/rdp/tests/oracle/freerdp_vectors.c` (FreeRDP
  3.10.3 algorithms; RSA raw encryption too); per-PDU transform, Low-level clear traffic, bad MAC,
  None/FIPS refusal.
- `hostile_rdp`: a refusal reconnects requesting `PROTOCOL_RDP`; other failure codes never fall back;
  a forged refusal for a host with a saved TLS certificate is a downgrade error; Server Security Data
  with a 3 KiB certificate decodes and 20 KiB is rejected.

Not covered by this VM: a license server issuing licenses (the VM has no RD Licensing role, so the
server answers with a valid-client licensing message), servers with an X.509 certificate chain in
Server Security Data (the VM sends a proprietary certificate), and whether the server used salted MACs
(both variants are implemented and tested offline).

## Legacy-TLS servers (Windows Server 2008 R2) — 2026-10-15

Against a real Windows Server 2008 R2 (TLS 1.0 only), with "Allow legacy TLS" on, the Linux client
connects end to end: X.224 → legacy TLS 1.0 → NLA/CredSSP (domain `DOMAIN\user`) → activation →
live screen and pointer updates. Two server-interop fixes were needed and are covered offline:

- `legacy_server`: the Connect Initial no longer advertises RDP 8 features (MCS message channel,
  network auto-detect, skip channel join) when "Allow legacy TLS" is on; a 2008-era server reset the
  connection on the Connect Initial otherwise.
- Batched Share Control PDUs: the 2008 R2 server batches Synchronize + Bitmap Update + Synchronize +
  Pointer Update in one MCS Send Data Indication; the vendored `ironrdp-session` splits them by the
  Share Control total length. Regression: `crates/rdp/tests/batched_slowpath.rs` (a real capture).

The Windows 2022 NLA receipt above still passes (no regression).

## Bitmap rendering fix (sheared text on 2008 R2) — 2026-10-15

Against the real Windows Server 2008 R2, bitmap updates whose width the server padded to a multiple
of 4 (e.g. a 609-px rectangle sent as a 612-px bitmap) were sheared: flat areas fine, text and icons
torn. The vendored `ironrdp-session` now derives the source row stride from the buffer. Captured the
server's framebuffer before and after (login screen and full desktop) — text and icons render crisp.
Regression `crates/rdp/tests/bitmap_stride.rs` (a padded bitmap must not shear). No change for
unpadded bitmaps (Windows 2022 NLA receipt unchanged).
