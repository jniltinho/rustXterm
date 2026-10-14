#!/bin/sh
# Assemble release artifacts in /dist from the built ELF in /out (Dockerfile `appimage` stage).
# Runs offline. Inputs: /out/rustxterm, /tmp/metadata.json, RXT_VERSION (optional override).
set -eu
here=packaging
runtime=/opt/appimage/runtime-x86_64
grep -v '^#' "$here/appimage/pins.txt" | while read -r sum name _url; do
    case $name in
        runtime-x86_64) file=$runtime ;;
        *) file=$here/appimage/$name ;;
    esac
    printf '%s  %s\n' "$sum" "$file" | sha256sum --check --strict --quiet -
done

version=${RXT_VERSION:-$(/out/rustxterm --version | awk '{print $2}')}
case $version in ''|*[!0-9A-Za-z.+-]*) echo "invalid version: $version" >&2; exit 1 ;; esac
base=rustxterm-$version-x86_64
umask 022
mkdir -p /dist
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

desktop-file-validate "$here/rustxterm.desktop"
python3 "$here/third-party-notices.py" /tmp/metadata.json > "$work/THIRD-PARTY-NOTICES.txt"
rsvg-convert --width 256 --height 256 assets/icons/rustxterm.svg --output "$work/rustxterm.png"

# Portable binary (already stripped by the release profile).
cp /out/rustxterm "/dist/$base"
chmod 0755 "/dist/$base"

# Tarball: fixed order, owner and mtime keep it independent of the build host.
tree=$work/tar/$base
mkdir -p "$tree"
cp /out/rustxterm "$tree/rustxterm"
cp "$here/rustxterm.desktop" assets/icons/rustxterm.svg "$work/rustxterm.png" LICENSE ATTRIBUTION.md \
    "$work/THIRD-PARTY-NOTICES.txt" "$tree/"
cp "$here/README.txt" "$tree/README.txt"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$work/tar" -cf - "$base" \
    | gzip -9n > "/dist/$base.tar.gz"

# AppImage: the same ELF plus desktop entry, icons and notices; bundles no shared libraries.
appdir=$work/AppDir
doc=$appdir/usr/share/doc/rustxterm
icons=$appdir/usr/share/icons/hicolor
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$icons/256x256/apps" "$icons/scalable/apps" "$doc"
cp /out/rustxterm "$appdir/usr/bin/rustxterm"
ln -s usr/bin/rustxterm "$appdir/AppRun"
cp "$here/rustxterm.desktop" "$appdir/rustxterm.desktop"
cp "$here/rustxterm.desktop" "$appdir/usr/share/applications/rustxterm.desktop"
cp assets/icons/rustxterm.svg "$icons/scalable/apps/rustxterm.svg"
cp "$work/rustxterm.png" "$icons/256x256/apps/rustxterm.png"
cp "$work/rustxterm.png" "$appdir/rustxterm.png"
ln -s rustxterm.png "$appdir/.DirIcon"
cp LICENSE ATTRIBUTION.md "$work/THIRD-PARTY-NOTICES.txt" "$doc/"
cp -R "$here/appimage/runtime-licenses" "$doc/appimage-runtime-licenses"
mksquashfs "$appdir" "$work/image.squashfs" -noappend -all-root -no-xattrs \
    -comp zstd -mkfs-time 0 -all-time 0 -quiet -no-progress
cat "$runtime" "$work/image.squashfs" > "/dist/$base.AppImage"
chmod 0755 "/dist/$base.AppImage"

# SHA256SUMS over the Linux and Windows artifacts: Dockerfile `checksums` stage.
ls -l /dist
