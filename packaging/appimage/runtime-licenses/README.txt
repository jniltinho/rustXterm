AppImage type-2 runtime notices

The AppImage file starts with the unmodified official AppImage type-2 runtime
(https://github.com/AppImage/type2-runtime, release 20251108, commit
dd6cebedcbddde9c82f89b011e8e1d40b6e43868), MIT license. The runtime statically
links musl libc, libfuse 3.15.0 (LGPL-2.1, patched mount.c upstream),
squashfuse 0.5.2, zstd 1.5.6, zlib 1.3.1 and mimalloc. Versions of squashfuse,
zstd and zlib were read from strings in the pinned binary; musl 1.2.5 and
mimalloc 2.1.7 are inferred from the upstream Alpine 3.21 build image.
Corresponding runtime source: the upstream repository at that commit.
Exact source URLs and SHA-256 values are in packaging/appimage/pins.txt.
