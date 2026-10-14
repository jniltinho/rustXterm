# syntax=docker/dockerfile:1.19.0@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6
# rustXterm build image. The host needs only Docker (see design D10).
FROM curlimages/curl:8.17.0@sha256:43ebaa53d3806db6b1ce4353b6b26ae638ec1c167ee351524b05690f988bb20d AS bootstrap-ca

FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b AS toolchain
COPY --from=bootstrap-ca /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
# Snapshot apt pins transitive tool versions; runtime libs are only for tests (wgpu lavapipe, winit dlopen).
RUN printf '%s\n' \
        'Types: deb' \
        'URIs: https://snapshot.ubuntu.com/ubuntu/20261001T000000Z/' \
        'Suites: noble noble-updates noble-security' \
        'Components: main universe' \
        'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
        'Check-Valid-Until: no' > /etc/apt/sources.list.d/ubuntu.sources \
    && apt-get update --error-on=any \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl xz-utils build-essential binutils perl pkgconf git python3 \
        mesa-vulkan-drivers libvulkan1 libegl1 libgl1 libxkbcommon0 libxkbcommon-x11-0 \
        libwayland-client0 libx11-6 libx11-xcb1 libxcursor1 libxrandr2 libxi6 xvfb xauth socat \
    && rm -rf /var/lib/apt/lists/*
COPY docker/rust-components.sha256 /tmp/rust-components.sha256
RUN set -eu; cd /tmp; \
    while read -r checksum archive; do \
        curl --fail --show-error --silent --location --retry 3 \
            "https://static.rust-lang.org/dist/2026-10-01/$archive" -o "$archive"; \
        printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --strict -; \
        tar -xJf "$archive"; \
        "${archive%.tar.xz}/install.sh" --prefix=/opt/rust --disable-ldconfig; \
        rm -rf "$archive" "${archive%.tar.xz}"; \
    done < /tmp/rust-components.sha256
ARG ZIG_VERSION=0.16.0
ARG ZIG_SHA256=70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00
RUN set -eu; cd /tmp; \
    curl --fail --show-error --silent --location --retry 3 \
        "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz" -o zig.tar.xz; \
    printf '%s  zig.tar.xz\n' "$ZIG_SHA256" | sha256sum --check --strict -; \
    mkdir -p /opt/zig; tar -xJf zig.tar.xz -C /opt/zig --strip-components=1; rm zig.tar.xz
ENV PATH="/opt/rust/bin:/opt/zig:/opt/cargo-tools/bin:${PATH}"
RUN CARGO_HOME=/tmp/ch cargo install --locked --root /opt/cargo-tools cargo-zigbuild@0.23.4 && rm -rf /tmp/ch
RUN useradd --uid 10001 --create-home builder && mkdir -p /work /out && chown builder:builder /work /out
ENV CARGO_HOME=/home/builder/.cargo
WORKDIR /work
USER builder

# Windows cross toolchain (add-windows-port design W1): mingw-w64 from the same snapshot and the
# checksummed Rust std for x86_64-pc-windows-gnu. Separate stage so the Linux toolchain cache stays.
# msitools (wixl, msibuild, msiinfo, msiextract) and osslsigncode package, inspect and sign (W9).
FROM toolchain AS windows-toolchain
USER root
RUN apt-get update --error-on=any \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 binutils-mingw-w64-x86-64 msitools wixl osslsigncode \
    && rm -rf /var/lib/apt/lists/*
COPY docker/rust-components-windows.sha256 /tmp/rust-components-windows.sha256
RUN set -eu; cd /tmp; \
    while read -r checksum archive; do \
        curl --fail --show-error --silent --location --retry 3 \
            "https://static.rust-lang.org/dist/2026-10-01/$archive" -o "$archive"; \
        printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --strict -; \
        tar -xJf "$archive"; \
        "${archive%.tar.xz}/install.sh" --prefix=/opt/rust --disable-ldconfig; \
        rm -rf "$archive" "${archive%.tar.xz}"; \
    done < /tmp/rust-components-windows.sha256
# Static libgcc/winpthreads (only Windows system DLLs imported) and deterministic PE headers.
ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C link-arg=-static -C link-arg=-Wl,--no-insert-timestamp"
USER builder

FROM toolchain AS source
COPY --chown=builder:builder Cargo.toml Cargo.lock rust-toolchain.toml LICENSE ./
COPY --chown=builder:builder apps/ apps/
COPY --chown=builder:builder crates/ crates/
COPY --chown=builder:builder xtask/ xtask/
COPY --chown=builder:builder assets/ assets/
COPY --chown=builder:builder vendor/ vendor/

# Only this stage may access the network.
FROM source AS fetch
RUN cargo fetch --locked

FROM fetch AS build
RUN --network=none cargo zigbuild --workspace --release --frozen --target x86_64-unknown-linux-gnu.2.31 \
    && cp target/x86_64-unknown-linux-gnu/release/rustxterm /out/

FROM scratch AS artifacts
COPY --from=build /out/ /

# Release packaging (task 10.1). Tools from the same pinned snapshot plus the pinned AppImage runtime.
FROM toolchain AS appimage-tools
USER root
COPY packaging/appimage/pins.txt /tmp/appimage-pins.txt
RUN set -eu; apt-get update --error-on=any \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        squashfs-tools librsvg2-bin desktop-file-utils \
    && rm -rf /var/lib/apt/lists/*; \
    mkdir -p /opt/appimage /dist; chown builder:builder /dist; \
    awk '$2 == "runtime-x86_64" { print $1, $3 }' /tmp/appimage-pins.txt | { \
        read -r checksum url; \
        curl --fail --show-error --silent --location --retry 3 "$url" -o /opt/appimage/runtime-x86_64; \
        printf '%s  %s\n' "$checksum" /opt/appimage/runtime-x86_64 | sha256sum --check --strict -; }; \
    chmod 0644 /opt/appimage/runtime-x86_64
USER builder

# Dependency metadata for the third-party license summary.
FROM fetch AS metadata
RUN --network=none cargo metadata --format-version 1 --frozen --filter-platform x86_64-unknown-linux-gnu > /tmp/metadata.json

FROM appimage-tools AS appimage
COPY --from=build /out/rustxterm /out/rustxterm
COPY --from=metadata /tmp/metadata.json /tmp/metadata.json
COPY --chown=builder:builder LICENSE ATTRIBUTION.md ./
COPY --chown=builder:builder assets/icons/rustxterm.svg assets/icons/rustxterm.svg
COPY --chown=builder:builder packaging/ packaging/
# Empty means the workspace version; CI passes the tag without the leading `v`.
ARG RXT_VERSION=
RUN --network=none RXT_VERSION="$RXT_VERSION" sh packaging/dist.sh

# Windows release build, portable zip and per-user MSI (add-windows-port W9): the fetched crates
# and sources of the Linux build, cross-compiled offline with the windows-toolchain image.
FROM windows-toolchain AS dist-windows
COPY --from=fetch --chown=builder:builder /home/builder/.cargo /home/builder/.cargo
COPY --from=fetch --chown=builder:builder /work /work
COPY --chown=builder:builder ATTRIBUTION.md ./
COPY --chown=builder:builder packaging/ packaging/
ARG RXT_VERSION=
# Committer time of the source commit in CI (release.yml); the default keeps local builds stable.
ARG SOURCE_DATE_EPOCH=315532800
RUN --network=none set -eu; \
    cargo build --release --frozen --target x86_64-pc-windows-gnu -p rustxterm; \
    cargo metadata --format-version 1 --frozen --filter-platform x86_64-pc-windows-gnu > /tmp/metadata.json; \
    python3 packaging/third-party-notices.py /tmp/metadata.json > /tmp/THIRD-PARTY-NOTICES.txt; \
    version=${RXT_VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)}; \
    SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH XTASK="cargo run -q --frozen -p xtask --" sh packaging/windows/package.sh target/x86_64-pc-windows-gnu/release \
        /tmp/THIRD-PARTY-NOTICES.txt "$version" /home/builder/dist

# SHA256SUMS over every artifact, computed last (signed releases recompute it after signing).
FROM toolchain AS checksums
COPY --from=appimage /dist/ /home/builder/dist/
COPY --from=dist-windows /home/builder/dist/ /home/builder/dist/
USER root
RUN cd /home/builder/dist && sha256sum -- * > SHA256SUMS

# docker build --target dist --output type=local,dest=dist/ .
FROM scratch AS dist
COPY --from=checksums /home/builder/dist/ /
