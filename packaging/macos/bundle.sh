#!/bin/sh
# Build rustXterm.app and package it as a .dmg and a .tar.gz. Runs on macOS (the Apple SDK only
# runs on Apple hardware), headless and with nothing extra installed: `hdiutil create` makes the
# DMG from a folder with no mounted device or Finder scripting.
#
# Usage: packaging/macos/bundle.sh <binary> <version> <out-dir> [arch]
#   arch defaults to arm64; pass `universal` for a lipo'd arm64+x86_64 binary (artifact names use it).
# Env:
#   MACOS_SIGN_IDENTITY  Developer ID identity; when set the bundle is signed with the hardened
#                        runtime and a secure timestamp. Unset → ad-hoc signature (`-`).
#   MACOS_ICON           path to an .icns; when set it becomes the app icon. Optional.
#   MACOSX_DEPLOYMENT_TARGET is expected to have been 13.0 at compile time (checked by dist-check).
set -eu

bin=${1:?binary path}
version=${2:?version}
out=${3:?output directory}
arch=${4:-arm64}
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

[ -f "$bin" ] || { echo "binary not found: $bin" >&2; exit 1; }
mkdir -p "$out"
# Absolute, so the `cd "$stage"` subshells below still write here.
out=$(CDPATH= cd -- "$out" && pwd)

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
app="$stage/rustXterm.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$bin" "$app/Contents/MacOS/rustxterm"
chmod +x "$app/Contents/MacOS/rustxterm"
sed "s/__VERSION__/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"

# Build an .icns from a square PNG using macOS' sips + iconutil (an .iconset with the standard
# sizes, @2x included). Returns non-zero if the tools or source are missing.
build_icns() {
    src=$1
    out=$2
    command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1 || return 1
    [ -f "$src" ] || return 1
    work=$(mktemp -d)
    iconset="$work/icon.iconset"
    mkdir -p "$iconset"
    for s in 16 32 128 256 512; do
        s2=$((s * 2))
        sips -z "$s" "$s" "$src" --out "$iconset/icon_${s}x${s}.png" >/dev/null 2>&1 || { rm -rf "$work"; return 1; }
        sips -z "$s2" "$s2" "$src" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null 2>&1 || { rm -rf "$work"; return 1; }
    done
    iconutil -c icns "$iconset" -o "$out" >/dev/null 2>&1
    rc=$?
    rm -rf "$work"
    return $rc
}

# Icon: an explicit .icns (MACOS_ICON), a prebuilt one next to this script, or one generated from
# the app PNG at build time; otherwise the bundle ships without a custom icon (generic in Finder).
icns=""
if [ -n "${MACOS_ICON:-}" ] && [ -f "${MACOS_ICON}" ]; then
    icns=$MACOS_ICON
elif [ -f "$here/rustXterm.icns" ]; then
    icns="$here/rustXterm.icns"
elif build_icns "$root/assets/icons/rustxterm-256.png" "$stage/rustXterm.icns"; then
    icns="$stage/rustXterm.icns"
fi
if [ -n "$icns" ]; then
    cp "$icns" "$app/Contents/Resources/rustXterm.icns"
else
    # No icon available: drop the key so Finder does not look for a missing file.
    /usr/libexec/PlistBuddy -c "Delete :CFBundleIconFile" "$app/Contents/Info.plist" 2>/dev/null || true
fi

# Sign: Developer ID (hardened runtime + timestamp) when a secret identity exists, else ad-hoc.
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
else
    codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"

# Licenses alongside the app.
for f in LICENSE ATTRIBUTION.md; do
    [ -f "$root/$f" ] && cp "$root/$f" "$stage/$f"
done

tarball="$out/rustxterm-$version-macos-$arch.tar.gz"
( cd "$stage" && tar czf "$tarball" rustXterm.app $( [ -f LICENSE ] && echo LICENSE ) $( [ -f ATTRIBUTION.md ] && echo ATTRIBUTION.md ) )

# DMG: a staging folder with the app and an /Applications shortcut, packed read-only compressed.
dmgstage=$(mktemp -d)
cp -R "$app" "$dmgstage/"
ln -s /Applications "$dmgstage/Applications"
dmg="$out/rustxterm-$version-macos-$arch.dmg"
rm -f "$dmg"
hdiutil create -volname "rustXterm" -srcfolder "$dmgstage" -ov -format UDZO "$dmg" >/dev/null
rm -rf "$dmgstage"

echo "built:"
ls -la "$tarball" "$dmg"
shasum -a 256 "$tarball" "$dmg"
