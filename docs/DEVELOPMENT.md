# Developer guide

How to build, test and contribute to rustXterm. Project rules live in [`AGENTS.md`](../AGENTS.md); the
[documentation index](README.md) explains the design.

- [Prerequisites](#prerequisites)
- [Repository layout](#repository-layout)
- [Docker workflow](#docker-workflow)
- [Common tasks](#common-tasks)
- [Snapshots and screenshots](#snapshots-and-screenshots)
- [Protocol lab](#protocol-lab)
- [Running the GUI locally](#running-the-gui-locally)
- [Release process](#release-process)
- [OpenSpec workflow](#openspec-workflow)
- [Coding rules](#coding-rules)
- [Parallel agents](#parallel-agents)
- [Troubleshooting](#troubleshooting)

## Prerequisites

- Docker (with Compose and BuildKit) and git. `make` is optional.
- Nothing else. Do **not** install a Rust toolchain, Zig, Node or system libraries on the host: builds,
  lints and tests run only in the pinned toolchain image (`Dockerfile`, target `toolchain`).

After cloning, enable the pre-push publication guard once:

```sh
git config core.hooksPath .githooks
```

## Repository layout

```
apps/rustxterm/     single binary: GUI, CLI subcommands, `mcp`, `--self-test`
crates/             one crate per concern (see layers below)
xtask/              developer tasks (`cargo xtask ...`)
lab/                Docker protocol lab, keyring test, optional Vagrant VMs
packaging/          AppImage and tarball packaging used by the `dist` Docker target
scripts/            cargo-docker.sh (toolchain wrapper), distro-matrix.sh
vendor/             vendored third-party crates (own lints, see vendor/README.md)
openspec/           proposals, designs, specs and task lists
docs/               documentation, diagrams, screenshots, gate receipts
assets/             icons and fonts (every file listed in ATTRIBUTION.md)
```

The workspace is layered; a crate may depend only on crates in lower layers, and `cargo xtask layers`
enforces it.

| Layer | Crates |
|---|---|
| L0 | `rxt-core`, `rxt-platform` (contracts only, no IO) |
| L1 | `rxt-platform-linux`, `rxt-store`, `rxt-term` |
| L2 | `rxt-ssh`, `rxt-sftp`, `rxt-ftp`, `rxt-rdp`, `rxt-vnc`, `rxt-telnet`, `rxt-serial` (one crate per protocol) |
| L3 | `rxt-engine` (session manager, command registry, authorization boundary, control server) |
| L4 | `rxt-ui` (egui), `rxt-agent` (CLI and MCP, no egui) |
| app | `apps/rustxterm` (wires `rxt-platform-linux` in) |

![Architecture: crate layers](diagrams/01-architecture-layers.png)

Protocol and store crates use the `platform` contracts, never `platform-linux` directly, so other OS
adapters can be added later. Every UI action is a `Command` handled by the engine.

## Docker workflow

`scripts/cargo-docker.sh` runs cargo (or any command) in the toolchain image, building the image on first
use. The repository is mounted at `/work`, the container runs as your UID, and `HOME` and `CARGO_HOME`
live under `target/`, so nothing is written outside the checkout. Without arguments it runs
`cargo build --workspace`.

```sh
scripts/cargo-docker.sh cargo test -p rxt-ssh
scripts/cargo-docker.sh bash          # interactive shell
```

| Variable | Default | Effect |
|---|---|---|
| `CARGO_TARGET_DIR` | `target/docker` | Build directory inside the checkout. Parallel agents use `target/agent-<name>`; the lab uses `target/lab`. |
| `RXT_MOUNT_BASE` | `0` | `base/` is masked with an empty tmpfs; `1` mounts it read-only. Only for `cargo xtask guard`. |
| `RXT_MOUNT_PRINTS` | `0` | `docs/prints/` is masked; `1` bind-mounts it writable. Only for `cargo xtask prints`. |
| `RXT_DOCKER_NET` | `bridge` | Container network; `host` lets tests reach the lab on `127.0.0.1`. |
| `RXT_DOCKER_CPUS` | unset | `--cpus` limit, e.g. `4` for gate measurements on the 4-core reference. |
| `CARGO_INCREMENTAL` | `0` | Incremental compilation; off by default to save disk. |
| `RXT_TOOLCHAIN_IMAGE` | `rustxterm-toolchain:dev` | Image tag to use. |

`RUST_LOG`, `INSTA_UPDATE` and `UPDATE_SNAPSHOTS` are passed through. After changing the `Dockerfile`,
rebuild the image with `make toolchain`.

### Published build cache

Building the `toolchain`, `windows-toolchain` and `appimage-tools` stages downloads about 150 Ubuntu
packages from the dated `snapshot.ubuntu.com` mirror, which can take most of an hour from CI runners.
The stages are therefore published, with inline build cache, as
`docker.io/jniltinho/rustxterm-{toolchain,windows-toolchain,appimage-tools}:snapshot-20261001`
(the tag names the apt snapshot date), and `ci.yml` / `release.yml` list them in `cache-from`. The
Dockerfile stays the source of truth: when it changes, BuildKit simply rebuilds the changed stages.
After changing a tool stage or the snapshot date, republish with a new tag and update the workflows:

```sh
for t in toolchain windows-toolchain appimage-tools; do
  docker buildx build --builder default --target $t --cache-to type=inline \
    --tag docker.io/jniltinho/rustxterm-$t:snapshot-YYYYMMDD --push .
done
```

## Common tasks

The root `Makefile` wraps everything above; `make help` lists the targets. Each target calls
`scripts/cargo-docker.sh` or `docker`, never a host cargo.

| Make | Equivalent |
|---|---|
| `make build` / `make release` | `cargo build --workspace [--release]` |
| `make test` | `cargo test --workspace --locked` |
| `make test-ui` | `cargo test -p rxt-ui --locked` (kittest UI and snapshot tests) |
| `make snapshots` | `UPDATE_SNAPSHOTS=1 cargo test -p rxt-ui` |
| `make fmt` / `make fmt-check` | `cargo fmt --all [--check]` |
| `make clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| `make check` | fmt-check, clippy, test, layers, scenarios (the CI `checks` job minus guard and deny) |
| `make layers` | `cargo xtask layers` |
| `make scenarios` | `cargo xtask scenarios` (every spec scenario maps to a test) |
| `make guard` | `RXT_MOUNT_BASE=1 cargo xtask guard` (run before every push) |
| `make guard-ci` | `cargo xtask guard --ci` (as CI runs it, without `base/`) |
| `make deny` | `cargo deny check` with the pinned, SHA-256 checked binary CI uses (cached in `target/tools/`) |
| `make dist [VERSION=X.Y.Z]` | `docker build --target dist --output type=local,dest=dist/ .` |
| `make dist-check` | `cargo xtask dist-check dist` |
| `make matrix` | `scripts/distro-matrix.sh dist` |
| `make lab-up` / `lab-test` / `lab-down` | `lab/lab.sh up` / `test` / `down` |
| `make prints` | `RXT_MOUNT_PRINTS=1 cargo xtask prints` |
| `make shell` | interactive shell in the toolchain container |
| `make clean` | deletes `target/` (all build dirs, cargo home, tools) and `dist/`, without asking |

`DIST=path` changes the artifact directory of `dist`, `dist-check`, `matrix` and `clean`.

`cargo xtask` subcommands: `layers`, `guard [--ci] [--artifact PATH]...`, `dist-check DIR`,
`lab test` (use `lab/lab.sh test`, which adds the host network), `prints`, `scenarios`,
`windows-receipt` and `msi-normalize`.

## Snapshots and screenshots

`rxt-ui` renders every screen and dialog offscreen in Light and Dark with egui kittest on wgpu. In the
container wgpu uses Mesa lavapipe (software Vulkan), which the toolchain image installs for tests.

- Snapshots live in `crates/*/tests/snapshots/` and are compared on every `cargo test`. After an
  intended UI change run `make snapshots`, review the changed PNGs and commit them.
- `make prints` writes every screen at 1x and 2x to `docs/prints/rustxterm/`. `docs/prints/` is
  local-only and never committed.
- The published [screenshot gallery](screenshots/README.md) is a curated copy of prints from
  `docs/prints/rustxterm/` into `docs/screenshots/` (fictional data only).

## Protocol lab

A disposable Docker Compose lab (sshd, bastion + internal host, vsftpd FTP/FTPS, xrdp, TigerVNC). Every
port binds `127.0.0.1`, and all credentials are fictional (`alice` / `lab-alice-pass`, key-only `bob`,
...; see [lab/README.md](../lab/README.md)). Never point tests at real servers.

```sh
lab/lab.sh up      # build images, generate lab/.generated/, wait for healthchecks
lab/lab.sh test    # cargo xtask lab test with RXT_DOCKER_NET=host and CARGO_TARGET_DIR=target/lab
lab/lab.sh down
```

- Lab tests are `#[ignore]`d and run with `--ignored`. The executed test names must equal
  `lab/expected-tests.txt`; add a line whenever you add or rename a lab test.
- Keyring: `lab/keyring/run.sh` runs the Secret Service tests of `rxt-platform-linux` in a throwaway
  dbus + gnome-keyring container.
- Agent socket: `lab/agent/uid-test.sh` checks that another UID cannot use the control socket.
- Windows receipts (manual, never in CI or `expected-tests.txt`): `lab/vagrant/windows/` is an evaluation
  Windows Server box for RDP with NLA/CredSSP; the commands and results are recorded in
  [gates/m2.md](gates/m2.md). Accept Microsoft's evaluation terms yourself. `lab/vagrant/debian/` is a
  minimal sshd VM.

## Running the GUI locally

Build in Docker, run the binary on the host (it needs your X11/Wayland session and a Vulkan or OpenGL
driver; Mesa is fine). Never install toolchains on the host to do this.

```sh
make dist VERSION=0.0.0-dev
./dist/rustxterm-0.0.0-dev-x86_64            # portable binary, glibc 2.31 baseline
./dist/rustxterm-0.0.0-dev-x86_64 --self-test
```

For a faster loop, `make release` builds `target/docker/release/rustxterm`, linked against the
toolchain image's glibc (Ubuntu 24.04); it runs on hosts with that glibc or newer. To keep test data away
from your real sessions, point `RXT_DATA_DIR` at a scratch directory.

## Release process

See [release.md](release.md). In short: set the workspace version in `Cargo.toml`, refresh `Cargo.lock`
in Docker, wait for `ci` to pass on `main`, then push a `vX.Y.Z` tag. Tags matching `v*` trigger
`.github/workflows/release.yml`, the only workflow that builds and publishes binaries; `ci.yml` only
checks. A local dry run is `make dist VERSION=0.0.0-test DIST=dist/test`, `make dist-check DIST=dist/test`,
`make matrix DIST=dist/test`.

## OpenSpec workflow

Requirements and decisions live in [`openspec/`](../openspec/): `specs/` holds the accepted specs and
`changes/<name>/` holds one change each (`proposal.md`, `design.md`, delta `specs/`, `tasks.md`). The
roadmap table in the [documentation index](README.md#roadmap-and-openspec-changes) lists the changes.

With Claude Code, the `/opsx:*` commands drive the flow:

| Command | Use |
|---|---|
| `/opsx:explore` | Think through an idea before proposing |
| `/opsx:propose` | Create a change with all artifacts |
| `/opsx:update` | Revise a change's artifacts and keep them coherent |
| `/opsx:apply` | Implement the tasks of a change |
| `/opsx:sync` | Merge delta specs into `openspec/specs/` |
| `/opsx:archive` | Archive a finished change |

`tasks.md` is the source of truth for progress: tick `- [x]` items as they are verified and put the task
ids in commit messages. Work in large code batches, then one verification batch per milestone; record
its commands and results as a receipt in [`docs/gates/`](gates/).

## Coding rules

- `unsafe` is forbidden (`unsafe_code = "forbid"`); `unwrap`, `expect`, `todo!`, `unimplemented!` and
  `dbg!` are denied by clippy outside tests.
- Code, comments, CLI messages, docs and OpenSpec artifacts are in English.
- Respect the layers; no protocol or store crate depends on `platform-linux` or the UI.
- Every screen supports light, dark and system themes through theme tokens; no hard-coded colors.
- `base/` (local scratch and reference material) and `docs/prints/` are local-only, git-ignored: never add them to Git, Docker build
  contexts (`.dockerignore`) or artifacts.
- Every new icon, image, font or fixture gets a row in [ATTRIBUTION.md](../ATTRIBUTION.md).
- Tests use the lab or small VMs with fictional credentials, never real servers or real secrets.
- Run `make guard` before every push; the pre-push hook (`git config core.hooksPath .githooks`) runs it
  for you. Docs must not contain absolute home paths.

## Parallel agents

- Each agent owns a disjoint set of crates and does not edit others' files.
- Each agent builds with its own target directory: `CARGO_TARGET_DIR=target/agent-<name>`, so builds do
  not block each other. Use the shared `target/docker` when disk is tight.
- `make fmt-check` and `make clippy` cover the whole workspace and can fail on another agent's work in
  progress; scope with `-p <crate>` through the wrapper while others are mid-change.
- One agent integrates, runs the milestone verification batch and commits.

## Troubleshooting

- **Disk full**: each target directory holds tens of GB. Remove stale ones (`rm -rf target/agent-*`,
  `target/lab`), or everything with `make clean`. Reclaim Docker space with `docker builder prune` (and
  `docker image prune` for old toolchain images). Keep `CARGO_INCREMENTAL=0`.
- **Lab port conflicts**: the lab uses 127.0.0.1 ports 2222, 2223, 2121, 2990, 13389, 5901 and
  30000-30019. Stop whatever holds them (or a stale lab: `lab/lab.sh down`), then `lab/lab.sh up`. The
  Vagrant VMs use other ports (2322, 53389) to avoid clashes.
- **Lab tests cannot connect**: run them through `lab/lab.sh test` (host network), not plain
  `cargo test`.
- **Snapshot diffs on another machine**: snapshots are rendered with lavapipe inside the toolchain
  image; always regenerate them through `make snapshots`, never with a host GPU.
- **Guard fails on `base/`**: `cargo xtask guard` needs `RXT_MOUNT_BASE=1` (`make guard`); in CI use
  `make guard-ci`.
- **Stale toolchain image** after a `Dockerfile` change: `make toolchain`.
