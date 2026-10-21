//! WSL backend on Windows (wsl-sessions spec, design W2/W3). Distributions come from the per-user
//! `Lxss` registry entries; `wsl.exe --list --verbose` is only a fallback. Nothing is installed or
//! started here; the ConPTY local shell runs the launch this resolves.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rxt_core::{LaunchInfo, Result, WslOptions};
use rxt_platform::wsl::{Wsl, WslDistribution, WslStatus, parse_wsl_list, wsl_launch_info};

use crate::sys;

/// Per-user distribution registry (one GUID subkey each).
const LXSS: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
/// Store WSL service (has `--cd`); the inbox service is `LxssManager`.
const WSL_SERVICE: &str = r"SYSTEM\CurrentControlSet\Services\WslService";
const LXSS_MANAGER: &str = r"SYSTEM\CurrentControlSet\Services\LxssManager";
/// `CREATE_NO_WINDOW`: the fallback probe shows no console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Build floor for `wsl --install` (Windows 10 2004) and for `--cd` on inbox WSL (21286).
const INSTALL_FLOOR: u32 = 19041;
const CD_FLOOR: u32 = 21286;
/// Fallback probe timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Windows WSL backend. `None` in [`rxt_platform::Platform::wsl`] on other platforms.
pub struct WindowsWsl;

impl WindowsWsl {
    /// `Some` when WSL could run here (always, on Windows): a backend that reports the real state.
    pub fn detect() -> Option<Self> {
        Some(Self)
    }
}

fn var(k: &str) -> Option<String> {
    std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty())
}

fn system_root() -> String {
    var("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())
}

fn wsl_exe() -> String {
    format!(r"{}\System32\wsl.exe", system_root())
}

/// Windows profile folder, or the system drive root when it is not a directory.
fn profile_dir() -> String {
    var("USERPROFILE")
        .filter(|h| Path::new(h).is_dir())
        .unwrap_or_else(|| var("SystemDrive").map_or_else(|| r"C:\".into(), |d| format!(r"{d}\")))
}

/// A WSL service is registered (the launcher `wsl.exe` alone does not prove WSL is installed).
fn wsl_service_present() -> bool {
    sys::reg_key_exists(sys::HKLM, WSL_SERVICE) || sys::reg_key_exists(sys::HKLM, LXSS_MANAGER)
}

/// `CurrentBuildNumber` as a number, when readable.
fn windows_build() -> Option<u32> {
    sys::reg_sz(sys::HKLM, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Whether the installed WSL supports `--cd`: the Store WSL (its service), or an inbox build from
/// 21286.
fn supports_cd() -> bool {
    sys::reg_key_exists(sys::HKLM, WSL_SERVICE) || windows_build().is_some_and(|b| b >= CD_FLOOR)
}

/// Distributions from the `Lxss` registry entries, the default marked. `Err` on registry failure
/// (told apart from "no distribution", which is `Ok(empty)`).
fn registry_distributions() -> std::io::Result<Vec<WslDistribution>> {
    let default_guid = sys::reg_sz(sys::HKCU, LXSS, "DefaultDistribution").unwrap_or_default();
    let subkeys = match sys::reg_subkeys(sys::HKCU, LXSS) {
        Ok(k) => k,
        // No `Lxss` key means no distribution, not a query failure.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut distros = Vec::new();
    for sk in subkeys.iter().filter(|k| k.starts_with('{')) {
        let path = format!(r"{LXSS}\{sk}");
        let name = match sys::reg_sz(sys::HKCU, &path, "DistributionName") {
            Ok(n) if !n.is_empty() => n,
            _ => continue,
        };
        // The registry `Version` value is the Lxss key schema version (2), not the WSL version;
        // the WSL version is bit 3 of `Flags` (set = WSL 2), confirmed against a real distribution
        // (Flags=7 → WSL 1). `--list --verbose` stays the authority.
        // ponytail: Flags bit-3 heuristic for the WSL version; verbose listing is the source of truth.
        let version = sys::reg_dword_at(sys::HKCU, &path, "Flags").ok().map(|f| if f & 0x8 != 0 { 2 } else { 1 });
        distros.push(WslDistribution { name, default: sk.eq_ignore_ascii_case(&default_guid), version, running: None });
    }
    distros.sort_by(|a, b| b.default.cmp(&a.default).then_with(|| a.name.cmp(&b.name)));
    Ok(distros)
}

/// `wsl.exe --list --verbose` fallback with `WSL_UTF8=1`, killed and reaped after the timeout.
/// `None` when the probe cannot run.
fn verbose_distributions() -> Option<Vec<WslDistribution>> {
    let mut child = Command::new(wsl_exe())
        .args(["--list", "--verbose"])
        .env("WSL_UTF8", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // The output is small; a reader thread avoids a full-pipe deadlock under the poll loop.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // A failed probe (no distribution, WSL error) prints human guidance, not a table: do not parse
    // it as distributions. The registry is the source of truth; this is only a confirming fallback.
    if !status.success() {
        return None;
    }
    let buf = reader.join().ok()?;
    Some(parse_wsl_list(&buf))
}

impl Wsl for WindowsWsl {
    fn status(&self) -> WslStatus {
        if !wsl_service_present() {
            return WslStatus::NotInstalled { install_command: windows_build().is_some_and(|b| b >= INSTALL_FLOOR) };
        }
        match registry_distributions() {
            Ok(d) if !d.is_empty() => WslStatus::Listed(d),
            // Registry has no distribution; the verbose fallback is a second opinion.
            Ok(_) => match verbose_distributions() {
                Some(d) if !d.is_empty() => WslStatus::Listed(d),
                _ => WslStatus::NoDistribution,
            },
            Err(e) => match verbose_distributions() {
                Some(d) if !d.is_empty() => WslStatus::Listed(d),
                _ => WslStatus::CouldNotQuery(e.to_string()),
            },
        }
    }

    fn launch_info(&self, opts: &WslOptions) -> Result<LaunchInfo> {
        wsl_launch_info(opts, supports_cd(), &wsl_exe(), &profile_dir())
    }
}
