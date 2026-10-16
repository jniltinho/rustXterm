## Context

- The platform seam already supports two adapters: `apps/rustxterm/src/os.rs` picks `rxt-platform-linux` or `rxt-platform-windows` per target; `crates/platform` holds the contracts (paths, theme, clipboard, notifications, dialogs, keyring, control transport, opener, serial).
- The Linux adapter (~1,400 lines) is mostly portable Unix code: the control socket checks the peer with `UnixStream::peer_cred()` (available on macOS through the standard library and tokio), serial uses `serialport`, dialogs `rfd`, clipboard `arboard`, notifications `notify-rust`. Linux-only parts are the Secret Service keyring (`oo7`), the XDG portal theme (`ashpd`) and the `.desktop` entry.
- Unix-only code outside the adapter is already behind `cfg(unix)` (modes 0600/0700 in `store` and `engine`, SSH agent socket, X11 over Unix sockets), so it applies to macOS unchanged.
- Builds are Docker-only. macOS cannot follow that rule: the Apple SDK is licensed for Apple hardware only, `cargo-zigbuild` lacks the AppKit/Metal/Security frameworks the app links to, and `osxcross` would need the SDK inside a Docker image. The macOS build therefore runs natively on macOS, mirroring how the Windows receipt builds/tests in the Windows Vagrant VM under `lab/`. Two hosts: a disposable local macOS KVM VM (osx-kvm / ultimate-macOS-kvm) for development, or a hosted macOS runner for CI. Running macOS under KVM on non-Apple hardware is outside Apple's licence (operator's decision, never in CI or artifacts); the release receipt still needs a real Mac.

## Goals / Non-Goals

**Goals:**
- `aarch64-apple-darwin` build of the existing app with feature parity to Linux phase 1 (SSH, SFTP, FTP/FTPS, RDP, VNC, Telnet, Serial, agent control).
- No `unsafe` code; no new crate layer; protocol, engine and UI crates untouched except for `cfg` where needed.
- CI and release jobs on a hosted macOS arm64 runner; `.app` in `.dmg` and `.tar.gz`, ad-hoc signed by default.

**Non-Goals:**
- Intel or universal binaries, macOS < 13, App Store/sandbox, local shell, Homebrew cask (all later).
- Cross-compiling macOS from Linux.

## Decisions

1. **Separate crate `crates/platform-macos`**, mirroring the Windows adapter, rather than turning `platform-linux` into a Unix crate. The modules that are identical in substance (control socket, serial) start as adapted copies of the Linux ones. Alternative (shared `platform-unix` crate) adds a layer for ~600 lines; revisit if the copies drift.
2. **Keychain through `security-framework`** (generic password, service `rustxterm`, account `rustxterm:<id>`), the safe API, so the crate keeps `unsafe_code = "forbid"`. Alternative `keyring` crate: an extra abstraction over the same calls.
3. **Theme from winit/eframe**: egui already receives the system appearance and its change events on macOS; the adapter reports it instead of polling `defaults`. Fallback: read `AppleInterfaceStyle` on start.
4. **Paths** from `dirs` (`data_dir` = `~/Library/Application Support`, `cache_dir`, logs under `~/Library/Logs`); runtime directory for the control socket: `$TMPDIR/rustxterm-<uid>` (per-user `TMPDIR` on macOS, mode 0700, checked as on Linux).
5. **Serial**: `serialport` enumeration filtered to `/dev/cu.*`; the matching `/dev/tty.*` entries are dropped.
6a. **macOS test VM (osx-kvm / ultimate-macOS-kvm).** A disposable VM under `lab/macos/` built from [osx-kvm](https://github.com/kholia/osx-kvm) (`boot-macOS-headless.sh`, a QEMU/KVM/OpenCore setup) or [ultimate-macOS-kvm](https://github.com/Coopydood/ultimate-macOS-kvm) (a guided `./ultimate-macos-kvm.py` setup over the same stack), with Apple-Silicon-equivalent `aarch64` guest where the host supports it, else an `x86_64` guest (then the macOS artifact is `x86_64-apple-darwin` until a universal build lands). It is created on demand, builds and tests run inside it over SSH, and it is destroyed afterwards — exactly like `lab/vagrant/windows` and its `rxt.sh` driver (`push` the build, run commands, screenshot). A `lab/macos/README.md` records the create/destroy recipe, the fictional guest account and the forwarded SSH port; the VM image is never committed, never in a Docker build context, and never distributed. ultimate-macOS-kvm is preferred for its guided setup; osx-kvm for a minimal scripted path. Alternative: a hosted macOS runner (licence-clean, paid) or a borrowed Mac.

6. **Packaging** in a small script (`packaging/macos/bundle.sh`) run on the runner: `Info.plist` (bundle id `io.github.jniltinho.rustxterm`, version from the workspace, `LSMinimumSystemVersion` 13.0, `NSHighResolutionCapable`), `.icns` built from the existing 1024/512/256 px PNGs with `iconutil`, `codesign --sign -` (ad-hoc) or Developer ID with `--options runtime` when secrets exist, `hdiutil create` for the `.dmg`, `notarytool` + `stapler` when secrets exist.
7. **CI/build**: the macOS steps (pinned toolchain from `rust-toolchain.toml`, `cargo clippy --locked -D warnings`, `cargo test --locked` with lab tests `#[ignore]`/Linux-gated, `--self-test`, and in `release.yml` the bundle/dmg step and `dist-check`: `otool -L` allowlist, bundle layout, checksums, guard) run the same way on either host. For development that host is the local KVM VM over SSH (driven from `lab/macos/`); for CI it is an optional `macos` job on `macos-15` (arm64), enabled once hosted runners are available. The toolchain is installed only inside the VM or the runner, never on a developer desktop.
8. **Receipt** like Windows: `docs/gates/macos.md` bound to the commit and artifact checksums; `release.yml` drops the macOS artifacts when no receipt covers the tag.

## Risks / Trade-offs

- **Runner availability/cost** → macOS minutes on a private repository bill at 10× Linux. Mitigation: the local KVM VM is the default build/test host (no CI minutes needed), and the optional hosted `macos` job runs only on `main`/tags with a cached `target/`.
- **KVM VM licence and fragility** → running macOS under KVM on non-Apple hardware is outside Apple's licence (operator's decision; never in CI or artifacts), and OpenCore/osx-kvm setups can break on macOS updates. Mitigation: pin the guest macOS version and the osx-kvm/ultimate-macOS-kvm commit in `lab/macos/README.md`, keep the licence-clean paths (hosted runner, real Mac) documented, and keep the real-Mac receipt as the release gate.
- **No local verification** → developers without a Mac cannot reproduce macOS failures. Mitigation: keep macOS-specific code small and in one crate; every CI failure log is the reference; the receipt needs someone with a Mac.
- **Gatekeeper on unsigned builds** → users see "cannot be opened because the developer cannot be verified". Mitigation: ad-hoc signature, release-note instructions; Developer ID + notarization (Apple Developer Program, user decision and cost) removes the prompt.
- **Reproducibility** → macOS linker output and signatures are not bit-for-bit reproducible across runner images. Trade-off: checksums are published, reproducibility is only required for Linux and unsigned Windows artifacts.
- **Graphics** → wgpu uses Metal on macOS; the UI snapshot tests may render slightly differently there. Mitigation: per-platform tolerance or Linux-only snapshot comparison with a macOS smoke screenshot.
