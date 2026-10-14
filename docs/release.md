# Cutting a release

Binaries are built only by `.github/workflows/release.yml`, which runs when a tag
`vX.Y.Z` (or `vX.Y.Z-rc.N`) is pushed. `ci.yml` runs the checks on `main` and pull
requests and never uploads binaries.

1. Set `version` in `[workspace.package]` of `Cargo.toml` to `X.Y.Z` (the release job
   fails if the tag and the workspace version differ), refresh `Cargo.lock` inside
   Docker (`scripts/cargo-docker.sh cargo build --workspace`), commit and push to `main`.
2. Wait for `ci` to pass on that commit.
3. Tag and push:

   ```sh
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```

The release workflow does not rerun the checks (they run in `ci.yml` on `main`; tag a commit
whose CI is green). It only builds `docker build --target dist --build-arg RXT_VERSION=X.Y.Z`,
runs `cargo xtask dist-check dist` (glibc <= 2.31, allowed `NEEDED` libraries, stripped,
`--self-test` of the binary and the AppImage, `SHA256SUMS`, guard over the tarball and
AppImage) and creates the GitHub Release with:

- `rustxterm-X.Y.Z-x86_64` (portable binary)
- `rustxterm-X.Y.Z-x86_64.AppImage`
- `rustxterm-X.Y.Z-x86_64.tar.gz` (binary, `.desktop`, icon, `LICENSE`, `ATTRIBUTION.md`,
  `THIRD-PARTY-NOTICES.txt`, `README.txt`)
- `SHA256SUMS`

Tags with a `-` suffix are published as pre-releases.

## Local dry run

```sh
docker build --target dist --build-arg RXT_VERSION=0.0.0-test --output type=local,dest=dist/test .
scripts/cargo-docker.sh cargo xtask dist-check dist/test
scripts/distro-matrix.sh dist/test
```

`cargo deny check` locally (same pinned release binary as `ci.yml`, SHA-256 checked):

```sh
mkdir -p target/tools
curl -fsSL https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz -o /tmp/deny.tgz
echo "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f  /tmp/deny.tgz" | sha256sum -c -
tar -xzf /tmp/deny.tgz -C target/tools --strip-components=1 cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny
scripts/cargo-docker.sh target/tools/cargo-deny check
```

Release receipts (matrix, CI, desktop acceptance, scenario map) live in `docs/gates/release.md`.
