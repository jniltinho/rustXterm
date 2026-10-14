#!/bin/sh
# Authenticode signing of the Windows release artifacts (add-windows-port design W9), run after
# `xtask windows-receipt --check` accepted the unsigned build. In the windows-toolchain image:
#   WINDOWS_SIGNING_PFX=<file.pfx> WINDOWS_SIGNING_PASSWORD_FILE=<file> [WINDOWS_SIGNING_CA=<pem>] \
#   [WINDOWS_SIGNING_TIMESTAMP_URL=<url>] SOURCE_DATE_EPOCH=<time> packaging/windows/sign.sh DIST VERSION
# Signs both exes taken from the unsigned zip, repackages zip and MSI from them, signs the MSI,
# verifies the signatures of the MSI and of the exes extracted from the new zip and MSI, and only
# then replaces the unsigned zip and MSI in DIST. WINDOWS_SIGNING_CA verifies a private chain (tests).
set -eu
dist=$1 version=$2
: "${WINDOWS_SIGNING_PFX:?}" "${WINDOWS_SIGNING_PASSWORD_FILE:?}"
base=rustxterm-$version-windows-x86_64
msi=rustxterm-$version-x86_64.msi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
ts=${WINDOWS_SIGNING_TIMESTAMP_URL:+-ts $WINDOWS_SIGNING_TIMESTAMP_URL}
ca=${WINDOWS_SIGNING_CA:+-CAfile $WINDOWS_SIGNING_CA}

sign() {  # in out
    # shellcheck disable=SC2086
    osslsigncode sign -pkcs12 "$WINDOWS_SIGNING_PFX" -readpass "$WINDOWS_SIGNING_PASSWORD_FILE" \
        -n rustXterm -h sha256 $ts -in "$1" -out "$2" >/dev/null
}
verify() {
    # shellcheck disable=SC2086
    osslsigncode verify $ca -in "$1" >/dev/null || { echo "signature verification failed: $1" >&2; exit 1; }
}

python3 -m zipfile -e "$dist/$base.zip" "$work/unsigned"
mkdir -p "$work/signed" "$work/out"
for exe in rustxterm.exe rustxterm-cli.exe; do
    sign "$work/unsigned/$base/$exe" "$work/signed/$exe"
done
sh packaging/windows/package.sh "$work/signed" "$work/unsigned/$base/THIRD-PARTY-NOTICES.txt" "$version" "$work/out"
sign "$work/out/$msi" "$work/out/signed.msi"
mv "$work/out/signed.msi" "$work/out/$msi"

verify "$work/out/$msi"
python3 -m zipfile -e "$work/out/$base.zip" "$work/check-zip"
mkdir -p "$work/check-msi"
msiextract -C "$work/check-msi" "$work/out/$msi" >/dev/null
for exe in rustxterm.exe rustxterm-cli.exe; do
    verify "$work/check-zip/$base/$exe"
    verify "$(find "$work/check-msi" -name "$exe")"
done
cp "$work/out/$base.zip" "$work/out/$msi" "$dist/"
echo "signed: $base.zip $msi"
