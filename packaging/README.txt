rustXterm - native Linux terminal and remote session manager (SSH, SFTP, FTP/FTPS, RDP, VNC)

Run:
    ./rustxterm                  # the portable binary (x86_64, glibc 2.31 or newer)
    ./rustxterm --self-test      # headless checks, exits non-zero on failure

Install for the current user (optional):
    install -Dm755 rustxterm ~/.local/bin/rustxterm
    install -Dm644 rustxterm.desktop ~/.local/share/applications/rustxterm.desktop
    install -Dm644 rustxterm.svg ~/.local/share/icons/hicolor/scalable/apps/rustxterm.svg

Runtime needs: glibc, plus Vulkan or OpenGL drivers and Wayland or X11 from the host
(loaded at run time). SQLCipher and OpenSSL are linked statically.

Licenses: LICENSE (MIT) covers rustXterm itself. ATTRIBUTION.md lists bundled icons and
their origin; THIRD-PARTY-NOTICES.txt lists the Rust crates compiled into the binary.
Verify downloads with: sha256sum --check --ignore-missing SHA256SUMS
