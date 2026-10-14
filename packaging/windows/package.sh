#!/bin/sh
# Portable zip and per-user MSI from the two Windows executables (add-windows-port design W9).
# Runs offline in the windows-toolchain image (python3, wixl, msibuild, msiinfo, cargo xtask), from the repo root:
#   SOURCE_DATE_EPOCH=<commit time> packaging/windows/package.sh EXE_DIR NOTICES VERSION OUT_DIR
# Every input mtime, the zip member times and the MSI summary times come from SOURCE_DATE_EPOCH,
# so the outputs depend only on the inputs (checked by `xtask dist-check --reproduce`).
set -eu
exes=$1 notices=$2 version=$3 out=$4
here=packaging/windows
: "${SOURCE_DATE_EPOCH:?set SOURCE_DATE_EPOCH}"
case $version in ''|*[!0-9A-Za-z.+-]*) echo "invalid version: $version" >&2; exit 1 ;; esac
base=rustxterm-$version-windows-x86_64
msi=rustxterm-$version-x86_64.msi
umask 022
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

tree=$work/$base
mkdir -p "$tree"
cp "$exes/rustxterm.exe" "$exes/rustxterm-cli.exe" LICENSE ATTRIBUTION.md "$tree/"
cp "$notices" "$tree/THIRD-PARTY-NOTICES.txt"
cp "$here/README.txt" "$tree/README.txt"
chmod 0644 "$tree"/*
find "$tree" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
python3 "$here/zip.py" "$out/$base.zip" "$tree"

# MSI: ProductVersion is numeric (a pre-release suffix is dropped); ProductCode and PackageCode are
# UUIDv5 of the full version under a fixed namespace, so a rebuild of one version is identical.
uuid() { python3 -c 'import sys, uuid; print(str(uuid.uuid5(uuid.UUID("1C72B1F3-731B-44EA-BD45-1B2CA25A8975"), sys.argv[1])).upper())' "$1"; }
wixl --arch x64 -D "Version=${version%%[-+]*}" -D "ProductCode=$(uuid "product:$version")" \
    -D "PackageCode=$(uuid "package:$version")" -D "SourceDir=$tree" -o "$work/$msi" "$here/rustxterm.wxs"
# Start Menu shortcut AppUserModelID (toasts), a table wixl does not author.
cp "$here/MsiShortcutProperty.idt" "$work/"
msibuild "$work/$msi" -i "$work/MsiShortcutProperty.idt"
# wixl 0.103 ignores Package/@Id and writes the build time: fixed PackageCode and dates.
${XTASK:-cargo xtask} msi-normalize "$work/$msi" "$(uuid "package:$version")" "$SOURCE_DATE_EPOCH"
msiinfo export "$work/$msi" MsiShortcutProperty | grep -q 'rustXterm\.rustXterm' \
    || { echo "MSI lacks the AppUserModelID row" >&2; exit 1; }
cp "$work/$msi" "$out/$msi"
ls -l "$out/$base.zip" "$out/$msi"
