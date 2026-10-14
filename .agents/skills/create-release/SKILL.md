---
name: create-release
description: Create a rustXterm version tag and GitHub Release. Use when the user asks to release, tag, publish or cut a version. Releases are allowed only when CI on main is green for the commit being tagged.
---

# Releasing rustXterm

Binaries are built only by `.github/workflows/release.yml` on `v*.*.*` tags. Checks (fmt, clippy,
tests + snapshots, layers, guard, scenarios, cargo-deny, lab) run only in `ci.yml` on `main`.

## Release rule

**Only release a commit whose `ci` run on `main` is green.** The release workflow enforces it (it
refuses to publish when the tagged commit has no successful `ci` run), but check before tagging.

## Before you start

```bash
gh auth status                      # repo scope
git status                          # must be clean
git fetch origin --tags && git checkout main && git pull --ff-only
LAST=$(git describe --tags --abbrev=0 2>/dev/null || echo none)
echo "Last tag: $LAST"
```

## Versioning

Semantic versions with a `v` prefix: `v0.1.0`, `v0.1.1` (fixes), `v0.2.0` (features), pre-releases
`v0.2.0-rc.1` (published as prerelease). The tag must equal `[workspace.package] version` in the root
`Cargo.toml` (the workflow checks it).

```bash
NEXT=v0.1.1   # choose per the changes
```

## Process

### 1. Review changes since the last release

```bash
git log $LAST..HEAD --oneline
git log $LAST..HEAD --stat
```

### 2. Bump the version (if not already)

Set `version = "X.Y.Z"` in the root `Cargo.toml`, refresh the lock in Docker and commit:

```bash
scripts/cargo-docker.sh cargo update --workspace
git commit -am "Release X.Y.Z: version bump"
git push origin main
```

Snapshots that show the version (About dialog) may need `UPDATE_SNAPSHOTS=1`; see docs/DEVELOPMENT.md.

### 3. Require green CI on that commit

```bash
SHA=$(git rev-parse HEAD)
gh run list --workflow ci.yml --commit "$SHA" --json status,conclusion -q '.[0]'
# wait until {"conclusion":"success","status":"completed"}; never tag a red or running commit
```

### 4. Tag and push (triggers release.yml)

```bash
git tag -a $NEXT -m "Release $NEXT"
git push origin $NEXT
```

`release.yml` then: verifies the green CI run, checks tag == workspace version, builds the toolchain
image and `docker build --target dist`, runs `cargo xtask dist-check dist`, and creates the GitHub
Release with `rustxterm-X.Y.Z-x86_64`, `.AppImage`, `.tar.gz` and `SHA256SUMS`.

### 5. Monitor

```bash
gh run list --workflow release.yml --limit 3
gh release view $NEXT          # wait until the 4 assets appear
```

### 6. Write the release notes

Categorize commits in this exact order; omit empty sections:

| Section | Content |
|---|---|
| **✨ New Features** | new user-visible functionality (protocols, screens, CLI/MCP commands) |
| **🔧 Improvements** | fixes, performance, refactors, CI/build |
| **🔒 Security** | security-relevant changes (trust, crypto, agent boundary) |
| **🧹 Cleanup** | dead code, dependency cleanup |
| **📚 Documentation** | README, docs, diagrams, guides |

```bash
cat > /tmp/release-notes-$NEXT.md << 'NOTES'
# Release vX.Y.Z

## ✨ New Features

- ...

## 🔧 Improvements

- ...

## 📦 Downloads

- `rustxterm-X.Y.Z-x86_64` portable binary (glibc >= 2.31)
- `rustxterm-X.Y.Z-x86_64.AppImage`
- `rustxterm-X.Y.Z-x86_64.tar.gz` (binary, .desktop, icon, licenses)
- `SHA256SUMS`

**Full Changelog**: https://github.com/jniltinho/rustXterm/compare/vPREV...vNEXT
NOTES
gh release edit $NEXT --title "$NEXT" --notes-file /tmp/release-notes-$NEXT.md
```

### 7. Verify

```bash
gh release view $NEXT
```

- Title is `$NEXT`, sections present, Full Changelog range correct.
- Four assets attached; `SHA256SUMS` matches (`sha256sum -c` after download).

## Quick reference

```bash
SHA=$(git rev-parse HEAD); gh run list --workflow ci.yml --commit "$SHA" -L 1
git tag -a $NEXT -m "Release $NEXT" && git push origin $NEXT
gh run list --workflow release.yml -L 3; gh release view $NEXT
gh release edit $NEXT --title "$NEXT" --notes-file /tmp/release-notes-$NEXT.md
```
