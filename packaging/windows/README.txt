rustXterm - terminal and remote session manager (SSH, SFTP, FTP/FTPS, RDP, VNC, Telnet, Serial)

Windows 10 1809 or newer, Windows 11, Windows Server 2019 or newer (x86_64).

Run:
    rustxterm.exe                    the application (no console window)
    rustxterm-cli.exe --help         command line: sessions, tabs, `mcp` server for agents
    rustxterm-cli.exe --self-test    headless checks, exits non-zero on failure

Install: unzip anywhere and start rustxterm.exe, or use the per-user MSI (no administrator
rights; installs to %LOCALAPPDATA%\Programs\rustXterm with a Start Menu shortcut).
Data: settings in %APPDATA%\rustxterm, the encrypted session database in
%LOCALAPPDATA%\rustxterm, keyring entries "rustxterm:*" in Credential Manager. Uninstalling
leaves them in place; delete them by hand to remove everything.

Unsigned builds: Windows SmartScreen may warn about an unrecognized app. Check the
download against SHA256SUMS first (PowerShell: Get-FileHash <file> -Algorithm SHA256).

Licenses: LICENSE (MIT) covers rustXterm itself. ATTRIBUTION.md lists bundled icons and
their origin; THIRD-PARTY-NOTICES.txt lists the Rust crates compiled into the executables.
