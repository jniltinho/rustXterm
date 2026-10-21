//! WSL backend contract (wsl-sessions spec). The adapter lists distributions and resolves the
//! `wsl.exe` launch; the pure validation and argument building live here so they are tested on
//! every platform. Only `platform-windows` implements [`Wsl`]; elsewhere [`crate::Platform::wsl`]
//! is `None`.

use std::sync::{Arc, Mutex};

use rxt_core::{Error, LaunchInfo, Result, WslOptions};
pub use rxt_core::{WslDistribution, WslStatus};

/// WSL backend; `Some` in [`crate::Platform::wsl`] only on Windows (design W1).
pub trait Wsl: Send + Sync {
    /// List distributions and the installation state. Must not block the UI thread.
    fn status(&self) -> WslStatus;
    /// Resolve the `wsl.exe` launch for `opts` without starting anything. Validates the
    /// distribution, user, folder and startup commands; refuses before any process starts.
    fn launch_info(&self, opts: &WslOptions) -> Result<LaunchInfo>;
}

/// Distribution name rule (WSL's own registration rule).
pub fn validate_distribution(name: &str) -> Result<()> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
        return Err(Error::Invalid(format!("invalid distribution name '{name}'")));
    }
    Ok(())
}

/// POSIX-style user name: letters, digits, `_`, `.`, `-`, not starting with a digit, `-` or `.`,
/// with an optional single trailing `$` (`Dev_User`, `_service`, `machine$`).
pub fn validate_user(name: &str) -> Result<()> {
    let core = name.strip_suffix('$').unwrap_or(name);
    let ok_first = core.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_');
    let ok_rest = core.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if !ok_first || !ok_rest {
        return Err(Error::Invalid(format!("invalid user name '{name}'")));
    }
    Ok(())
}

/// Reject C0 control characters (0x00..=0x1F, CR and LF included) and DEL (0x7F): a line editor
/// would act on them. Use per line, after splitting startup commands on the editor's newlines.
fn validate_control_free(s: &str, what: &str) -> Result<()> {
    if s.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(Error::Invalid(format!("control character in {what}")));
    }
    Ok(())
}

/// The `cd` fallback for an older WSL without `--cd`: `cd` for `~`, `cd ~/'<rest>'` for `~/<rest>`
/// (the tilde stays unquoted so the shell expands it), `cd '<folder>'` otherwise. A folder holding
/// `'` is refused in this mode. bash, zsh and fish all read these the same way.
fn cd_fallback_line(folder: &str) -> Result<String> {
    if folder.contains('\'') {
        return Err(Error::Invalid("start folder containing ' needs a newer WSL with --cd".into()));
    }
    Ok(if folder == "~" {
        "cd".into()
    } else if let Some(rest) = folder.strip_prefix("~/") {
        format!("cd ~/'{rest}'")
    } else {
        format!("cd '{folder}'")
    })
}

/// Build the `wsl.exe` [`LaunchInfo`] for `opts`. `has_cd` says whether the installed WSL supports
/// `--cd` (else the start folder becomes a leading `cd` line in `startup_lines`). `wsl_exe` is the
/// launcher path and `cwd` the Windows working folder (both OS facts supplied by the adapter).
/// Nothing follows `--`, so the distribution starts the user's own login shell.
pub fn wsl_launch_info(opts: &WslOptions, has_cd: bool, wsl_exe: &str, cwd: &str) -> Result<LaunchInfo> {
    let mut args = Vec::new();
    if let Some(dist) = &opts.distribution {
        validate_distribution(dist)?;
        args.push("-d".into());
        args.push(dist.clone());
    }
    if let Some(user) = &opts.user {
        validate_user(user)?;
        args.push("-u".into());
        args.push(user.clone());
    }
    let mut startup_lines = Vec::new();
    if let Some(folder) = &opts.start_folder {
        validate_control_free(folder, "start folder")?;
        if has_cd {
            args.push("--cd".into());
            args.push(folder.clone());
        } else {
            startup_lines.push(cd_fallback_line(folder)?);
        }
    }
    if let Some(cmds) = &opts.startup_commands {
        for line in cmds.split('\n') {
            validate_control_free(line, "startup command")?;
            if !line.is_empty() {
                startup_lines.push(line.to_string());
            }
        }
    }
    Ok(LaunchInfo {
        shell: wsl_exe.to_string(),
        args,
        cwd: cwd.to_string(),
        user: opts.user.clone().unwrap_or_default(),
        host: opts.label().to_string(),
        startup_lines,
    })
}

/// Decode `wsl.exe` output: UTF-16LE when it starts with a `FF FE` BOM or holds NUL bytes (no
/// BOM), otherwise UTF-8 (what `WSL_UTF8=1` asks for). Lossy, never fails.
pub fn decode_wsl_output(bytes: &[u8]) -> String {
    let (body, utf16) = match bytes.strip_prefix(&[0xff, 0xfe]) {
        Some(rest) => (rest, true),
        None if bytes.contains(&0) => (bytes, true),
        None => (bytes, false),
    };
    if utf16 {
        let units: Vec<u16> = body.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// Parse `wsl.exe --list --verbose` output into distributions. The first non-empty line is the
/// header, skipped by position (never matched as text, so a localized header works). `*` marks the
/// default. Names hold no spaces; the first column is the name and the last is the version (the
/// state column may hold a multi-word localized value such as "Em execução", so it is not counted).
pub fn parse_wsl_list(bytes: &[u8]) -> Vec<WslDistribution> {
    let text = decode_wsl_output(bytes);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    lines.next(); // header, skipped by position
    lines
        .filter_map(|line| {
            let default = line.trim_start().starts_with('*');
            let cols: Vec<&str> = line.trim_start().trim_start_matches('*').split_whitespace().collect();
            let name = cols.first()?.to_string();
            // Only a name, state and version: the version is the last column (never the name).
            let version = (cols.len() > 1).then(|| cols.last()).flatten().and_then(|v| v.parse().ok());
            Some(WslDistribution { name, default, version, running: None })
        })
        .collect()
}

/// In-memory WSL backend for tests. Clones share state.
#[derive(Clone)]
pub struct FakeWsl {
    pub status: Arc<Mutex<WslStatus>>,
    /// Whether the fake WSL supports `--cd` (true by default).
    pub has_cd: Arc<Mutex<bool>>,
    /// Error returned by the next `launch_info` instead of building it.
    pub fail: Arc<Mutex<Option<String>>>,
}

impl Default for FakeWsl {
    fn default() -> Self {
        let listed = WslStatus::Listed(vec![
            WslDistribution { name: "Ubuntu".into(), default: true, version: Some(2), running: Some(false) },
            WslDistribution { name: "Debian".into(), default: false, version: Some(2), running: Some(false) },
        ]);
        Self {
            status: Arc::new(Mutex::new(listed)),
            has_cd: Arc::new(Mutex::new(true)),
            fail: Arc::new(Mutex::new(None)),
        }
    }
}

impl FakeWsl {
    /// Fictional launcher path and working folder used by the fake.
    pub const EXE: &str = r"C:\Windows\System32\wsl.exe";
    pub const CWD: &str = r"C:\Users\alice";
}

impl Wsl for FakeWsl {
    fn status(&self) -> WslStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_else(|e| e.into_inner().clone())
    }

    fn launch_info(&self, opts: &WslOptions) -> Result<LaunchInfo> {
        if let Some(e) = self.fail.lock().ok().and_then(|f| f.clone()) {
            return Err(Error::Io(e));
        }
        let has_cd = self.has_cd.lock().map(|b| *b).unwrap_or(true);
        wsl_launch_info(opts, has_cd, Self::EXE, Self::CWD)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn opts(dist: Option<&str>, user: Option<&str>, folder: Option<&str>, cmds: Option<&str>) -> WslOptions {
        WslOptions {
            distribution: dist.map(Into::into),
            user: user.map(Into::into),
            start_folder: folder.map(Into::into),
            startup_commands: cmds.map(Into::into),
        }
    }

    #[test]
    fn named_distribution_user_and_cd() {
        let info = wsl_launch_info(&opts(Some("Debian"), Some("dev"), Some("/srv/app"), None), true, "wsl.exe", "C:\\")
            .unwrap();
        assert_eq!(info.args, ["-d", "Debian", "-u", "dev", "--cd", "/srv/app"]);
        assert!(info.startup_lines.is_empty());
        assert_eq!(info.host, "Debian");
    }

    #[test]
    fn default_distribution_has_no_flag() {
        let info = wsl_launch_info(&opts(None, None, None, None), true, "wsl.exe", "C:\\").unwrap();
        assert!(info.args.is_empty());
        assert_eq!(info.host, "Default");
    }

    #[test]
    fn startup_commands_become_typed_lines() {
        let info = wsl_launch_info(
            &opts(None, None, None, Some("source .venv/bin/activate\nls \"$HOME\" | grep -c \"my file\"")),
            true,
            "wsl.exe",
            "C:\\",
        )
        .unwrap();
        assert_eq!(info.startup_lines, ["source .venv/bin/activate", "ls \"$HOME\" | grep -c \"my file\""]);
    }

    #[test]
    fn cd_fallback_without_cd_option() {
        let line = |f: &str| {
            wsl_launch_info(&opts(None, None, Some(f), None), false, "wsl.exe", "C:\\").unwrap().startup_lines
        };
        assert_eq!(line("~"), ["cd"]);
        assert_eq!(line("~/src"), ["cd ~/'src'"]);
        assert_eq!(line("/srv/app"), ["cd '/srv/app'"]);
        // --cd is not used in fallback mode.
        let info = wsl_launch_info(&opts(None, None, Some("/srv/app"), None), false, "wsl.exe", "C:\\").unwrap();
        assert!(!info.args.iter().any(|a| a == "--cd"));
        // A folder with ' is refused in fallback mode.
        assert!(wsl_launch_info(&opts(None, None, Some("/a'b"), None), false, "wsl.exe", "C:\\").is_err());
    }

    #[test]
    fn invalid_names_refused() {
        assert!(wsl_launch_info(&opts(Some("Ubuntu & calc.exe"), None, None, None), true, "wsl.exe", "C:\\").is_err());
        assert!(wsl_launch_info(&opts(Some("Ubuntu"), Some("9bad"), None, None), true, "wsl.exe", "C:\\").is_err());
        assert!(wsl_launch_info(&opts(Some("Ubuntu"), Some("-bad"), None, None), true, "wsl.exe", "C:\\").is_err());
    }

    #[test]
    fn valid_unusual_user_names() {
        for u in ["Dev_User", "_service", "machine$"] {
            assert!(validate_user(u).is_ok(), "{u}");
        }
    }

    #[test]
    fn control_characters_refused() {
        for bad in ["a\rb", "a\x15b", "a\x1bb", "a\x7fb"] {
            assert!(
                wsl_launch_info(&opts(None, None, Some(bad), None), true, "wsl.exe", "C:\\").is_err(),
                "folder {bad:?}"
            );
            assert!(
                wsl_launch_info(&opts(None, None, None, Some(bad)), true, "wsl.exe", "C:\\").is_err(),
                "command {bad:?}"
            );
        }
    }

    /// Encode a string as UTF-16LE bytes, optionally with a BOM.
    fn utf16le(s: &str, bom: bool) -> Vec<u8> {
        let mut out = if bom { vec![0xff, 0xfe] } else { Vec::new() };
        out.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        out
    }

    #[test]
    fn parse_list_utf8_and_default_marker() {
        let out = "  NAME      STATE     VERSION\n* Ubuntu    Running   2\n  Debian    Stopped   2\n";
        let d = parse_wsl_list(out.as_bytes());
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].default, d[0].version), ("Ubuntu", true, Some(2)));
        assert_eq!((d[1].name.as_str(), d[1].default, d[1].version), ("Debian", false, Some(2)));
    }

    #[test]
    fn parse_list_utf16_bom_and_no_bom_and_localized_header() {
        // Localized header, skipped by position; a multi-word localized state ("Em execução")
        // must not be mistaken for the version column. UTF-16LE with a BOM.
        let out = "  NOME      ESTADO    VERSÃO\n* Ubuntu    Em execução 2\n";
        let d = parse_wsl_list(&utf16le(out, true));
        assert_eq!((d[0].name.as_str(), d[0].version), ("Ubuntu", Some(2)));
        // UTF-16LE without a BOM (detected by NUL bytes).
        assert_eq!(parse_wsl_list(&utf16le(out, false))[0].name, "Ubuntu");
    }

    #[test]
    fn parse_list_no_distributions() {
        // Header only, or an informational message: no rows parsed.
        assert!(parse_wsl_list(b"  NAME  STATE  VERSION\n").is_empty());
    }

    #[test]
    fn fake_lists_and_launches() {
        let w = FakeWsl::default();
        assert!(matches!(w.status(), WslStatus::Listed(d) if d.len() == 2 && d[0].default));
        let info = w.launch_info(&opts(Some("Ubuntu"), None, None, None)).unwrap();
        assert_eq!(info.shell, FakeWsl::EXE);
    }
}
