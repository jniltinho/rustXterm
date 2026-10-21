//! Windows adapter for the platform contracts (add-windows-port design W3–W6): Known Folder
//! paths, registry theme with live updates, clipboard, toasts, Win32 file dialogs parented to
//! the main window, Credential Manager keyring, a named-pipe control transport restricted to
//! the current user, `ShellExecuteW` opener, owner-only directory ACLs, COM ports and local shells
//! through ConPTY ([`ConPty`]).
//!
//! Only this crate may use `unsafe` (user decision, design W2). Every Win32 call lives in
//! [`sys`] (and the pseudo-console in `conpty.rs`) behind a safe wrapper; each `unsafe` block
//! holds one operation and a `// SAFETY:`
//! comment (clippy denies both rules here, `cargo xtask layers` checks the lint table).
//!
//! The pure helpers below build on every target so they are unit-tested on Linux too; the
//! services themselves exist only on Windows.

#[cfg(windows)]
mod conpty;
#[cfg(windows)]
mod control;
#[cfg(windows)]
mod keyring;
#[cfg(windows)]
mod serial;
#[cfg(windows)]
mod services;
#[cfg(windows)]
mod sys;
#[cfg(windows)]
mod theme;
#[cfg(windows)]
mod wsl;

#[cfg(windows)]
pub use conpty::ConPty;
#[cfg(windows)]
pub use control::PipeControl;
#[cfg(windows)]
pub use keyring::CredentialManager;
#[cfg(windows)]
pub use serial::WindowsSerial;
#[cfg(windows)]
pub use services::{KnownFolders, platform};
#[cfg(windows)]
pub use theme::RegistryTheme;
#[cfg(windows)]
pub use wsl::WindowsWsl;

/// App subdirectory under the Known Folders.
pub const APP_DIR: &str = "rustxterm";

/// Prefix of Credential Manager targets (`rustxterm:<id>`), so users can find and delete them.
pub const CREDENTIAL_PREFIX: &str = "rustxterm:";

/// Credential Manager limit for a generic credential blob (`CRED_MAX_CREDENTIAL_BLOB_SIZE`).
pub const MAX_CREDENTIAL_BLOB: usize = 5 * 512;

/// Registry key and value holding the Windows "app mode" (1 light, 0 dark).
pub const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
pub const APPS_USE_LIGHT_THEME: &str = "AppsUseLightTheme";

/// `prefers_dark` from the `AppsUseLightTheme` DWORD (`None` when the value is missing).
pub fn prefers_dark(apps_use_light_theme: Option<u32>) -> Option<bool> {
    apps_use_light_theme.map(|v| v == 0)
}

/// Credential Manager target name for keyring item `id`.
pub fn credential_target(id: &str) -> String {
    format!("{CREDENTIAL_PREFIX}{id}")
}

/// Control pipe of the user with SID `sid` (design W4).
pub fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\rustxterm-{sid}-control")
}

/// Pipe security descriptor: protected DACL, generic-all for `sid` only (no Everyone read).
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// Security descriptor for private directories and files: owner `sid`, protected DACL with full
/// control for `sid` only, inherited by new files and subdirectories (design W6).
pub fn private_dir_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
}

/// Whether an SDDL trustee names the user `sid`. SDDL serialization writes the local built-in
/// Administrator (RID 500, the account GitHub's Windows runners log on as) as the alias `LA`.
fn is_user(who: &str, sid: &str) -> bool {
    who == sid || (who == "LA" && sid.starts_with("S-1-5-21-") && sid.ends_with("-500"))
}

/// Acceptable owner: the user, or Administrators (`BA`), the default owner of files an elevated
/// administrator creates (administrators can take ownership anyway).
fn owner_ok(owner: &str, sid: &str) -> bool {
    is_user(owner, sid) || owner == "BA"
}

/// Whether an owner+DACL descriptor read back as SDDL has an acceptable owner and grants access
/// to `sid` only, either protected or made only of ACEs inherited from a parent (which
/// `make_private` verified as protected first).
pub fn sd_is_owner_only(sddl: &str, sid: &str) -> bool {
    let Some((owner, dacl)) = sddl.strip_prefix("O:").and_then(|r| r.split_once("D:")) else { return false };
    let (flags, aces) = dacl.split_at(dacl.find('(').unwrap_or(dacl.len()));
    let aces: Vec<&str> = aces.split_inclusive(')').collect();
    let inherited = |ace: &&str| ace.split(';').nth(1).is_some_and(|f| f.contains("ID"));
    owner_ok(owner, sid)
        && (flags.contains('P') || aces.iter().all(inherited))
        && !aces.is_empty()
        && aces.iter().all(|ace| {
            ace.starts_with("(A;") && ace.rsplit(';').next().is_some_and(|who| is_user(who.trim_end_matches(')'), sid))
        })
}

/// Whether the owner part of an `O:<sid>D:...` read-back is already acceptable (no change needed).
pub fn owner_is(sddl: &str, sid: &str) -> bool {
    sddl.strip_prefix("O:").and_then(|r| r.split_once("D:")).is_some_and(|(owner, _)| owner_ok(owner, sid))
}

/// Token identity: user SID, restricting SIDs or AppContainer, integrity level RID, and the
/// group SIDs that are enabled or deny-only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub sid: String,
    pub restricted: bool,
    pub integrity: u32,
    pub enabled: Vec<String>,
    pub deny_only: Vec<String>,
}

/// RID of an integrity label SID (`S-1-16-8192` → 8192).
pub fn integrity_rid(label_sid: &str) -> Option<u32> {
    label_sid.strip_prefix("S-1-16-")?.parse().ok()
}

/// Refusal message when the two sides of the control pipe run at a different elevation
/// (integrity level or UAC filtering).
pub const DIFFERENT_ELEVATION: &str =
    "agent control runs at a different elevation (run the CLI and rustXterm both elevated or both not)";

/// Integrity RID of the medium level; below it a token is sandboxed, not just non-elevated.
const MEDIUM_INTEGRITY: u32 = 0x2000;

/// `BUILTIN\Administrators`: deny-only in a UAC-filtered (non-elevated) admin token.
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// Groups enabled for `a` that are deny-only for `b` (`b` is more filtered than `a`).
fn more_filtered<'a>(b: &'a Identity, a: &'a Identity) -> impl Iterator<Item = &'a String> {
    b.deny_only.iter().filter(|g| a.enabled.contains(g))
}

/// Why the peer of a control pipe is refused from `me`'s side (design W4), `None` when it may be
/// trusted: same user SID, no restricting SIDs or AppContainer, integrity not below mine, and not
/// more filtered than me (no group enabled for me is deny-only for the peer). Unknown identity is
/// refused. The message is sent to the refused client; a lower integrity (not sandboxed) or UAC
/// filtering of Administrators is reported as a different elevation.
pub fn peer_refusal(peer: Option<&Identity>, me: &Identity) -> Option<&'static str> {
    let Some(p) = peer else { return Some("agent control could not identify the client") };
    if p.sid != me.sid {
        Some("agent control serves only the user running rustXterm")
    } else if p.restricted || more_filtered(p, me).any(|g| g != ADMINISTRATORS) {
        Some("agent control refuses restricted, filtered or AppContainer clients")
    } else if p.integrity < me.integrity && p.integrity < MEDIUM_INTEGRITY {
        Some("agent control refuses low-integrity clients")
    } else if p.integrity < me.integrity || more_filtered(p, me).next().is_some() {
        Some(DIFFERENT_ELEVATION)
    } else {
        None
    }
}

/// Whether the peer of a control pipe may be trusted (see [`peer_refusal`]).
pub fn peer_allowed(peer: Option<&Identity>, me: &Identity) -> bool {
    peer_refusal(peer, me).is_none()
}

/// Client-side check of the pipe server (design W4): the same policy as for clients, and the
/// same elevation (integrity level and UAC filtering) on both sides. `Err` is the message.
pub fn server_allowed(server: Option<&Identity>, me: &Identity) -> Result<(), &'static str> {
    match server {
        Some(s)
            if s.sid == me.sid
                && !s.restricted
                && (s.integrity != me.integrity
                    || more_filtered(s, me).any(|g| g == ADMINISTRATORS)
                    || more_filtered(me, s).any(|g| g == ADMINISTRATORS)) =>
        {
            Err(DIFFERENT_ELEVATION)
        }
        _ if peer_allowed(server, me) => Ok(()),
        _ => Err("control pipe is not served by rustXterm of this user"),
    }
}

/// Access-mask bits that let a principal replace a directory, change its permissions or turn it
/// into a reparse point: `DELETE`, `WRITE_DAC`, `WRITE_OWNER`, `FILE_DELETE_CHILD`,
/// `FILE_WRITE_DATA`, `FILE_APPEND_DATA`, `FILE_WRITE_EA`, `FILE_WRITE_ATTRIBUTES`, `GENERIC_ALL`,
/// `GENERIC_WRITE`.
const REPLACE_RIGHTS: u32 =
    0x1_0000 | 0x4_0000 | 0x8_0000 | 0x40 | 0x2 | 0x4 | 0x10 | 0x100 | 0x1000_0000 | 0x4000_0000;

/// Access mask of an SDDL rights field (`FA`, `0x1301bf`, `CCDCLC…`); `None` if unknown.
fn rights_mask(rights: &str) -> Option<u32> {
    if let Some(hex) = rights.strip_prefix("0x").or_else(|| rights.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    let mut mask = 0;
    for i in (0..rights.len()).step_by(2) {
        mask |= match rights.get(i..i + 2)? {
            "GA" => 0x1000_0000,
            "GR" => 0x8000_0000,
            "GW" => 0x4000_0000,
            "GX" => 0x2000_0000,
            "FA" => 0x1F_01FF,
            "FR" => 0x12_0089,
            "FW" => 0x12_0116,
            "FX" => 0x12_00A0,
            "SD" => 0x1_0000,
            "RC" => 0x2_0000,
            "WD" => 0x4_0000,
            "WO" => 0x8_0000,
            "CC" => 0x1,
            "DC" => 0x2,
            "LC" => 0x4,
            "SW" => 0x8,
            "RP" => 0x10,
            "WP" => 0x20,
            "DT" => 0x40,
            "LO" => 0x80,
            "CR" => 0x100,
            _ => return None,
        };
    }
    Some(mask)
}

/// Whether an ancestor directory (owner+DACL SDDL) is safe from other users (design W6): owner
/// is the user, Administrators or SYSTEM, and no allow-ACE that applies to the directory itself
/// gives anyone else replace or permission-change rights. Unknown rights count as unsafe.
pub fn ancestor_safe(sddl: &str, sid: &str) -> bool {
    // LA (the built-in Administrator, RID 500) is always a member of Administrators; GitHub's
    // Windows runners log on as that account, so their profile ACLs name it by this alias.
    let trusted = |who: &str| who == sid || matches!(who, "SY" | "BA" | "LA" | "S-1-5-18" | "S-1-5-32-544");
    let Some((owner, dacl)) = sddl.strip_prefix("O:").and_then(|r| r.split_once("D:")) else { return false };
    // A null DACL (`NO_ACCESS_CONTROL`) grants everyone full access.
    if !trusted(owner) || dacl.contains("NO_ACCESS_CONTROL") {
        return false;
    }
    let aces = dacl.split('(').skip(1).map(|a| a.trim_end_matches(')'));
    aces.into_iter().all(|ace| {
        let f: Vec<&str> = ace.split(';').collect();
        let (Some(kind), Some(flags), Some(rights), Some(who)) = (f.first(), f.get(1), f.get(2), f.last()) else {
            return false;
        };
        let applies = !flags.contains("IO");
        *kind != "A" || !applies || trusted(who) || rights_mask(rights).is_some_and(|m| m & REPLACE_RIGHTS == 0)
    })
}

/// Path text without the verbatim prefix (`\\?\C:\x` → `C:\x`), for comparisons.
pub fn strip_verbatim(path: &str) -> &str {
    path.strip_prefix(r"\\?\").filter(|p| !p.starts_with("UNC\\")).unwrap_or(path)
}

/// Whether two Windows paths name the same location textually (case-insensitive, verbatim
/// prefix and trailing separators ignored).
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| strip_verbatim(p).trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// Whether `path` lies inside `root` (both canonical; case-insensitive, component-wise).
pub fn inside(path: &str, root: &str) -> bool {
    let (p, r) = (strip_verbatim(path).to_lowercase(), strip_verbatim(root).trim_end_matches('\\').to_lowercase());
    p == r || p.strip_prefix(&r).is_some_and(|rest| rest.starts_with('\\'))
}

/// Whether `path` is a UNC (network) path: `\\server\share`, `\\?\UNC\…`. Device paths
/// (`\\.\…`) and long local paths (`\\?\C:\…`) are not UNC.
pub fn is_unc(path: &str) -> bool {
    let p = path.replace('/', "\\");
    match p.strip_prefix(r"\\") {
        Some(rest) if rest.starts_with(r"?\") => rest[2..].get(..4).is_some_and(|u| u.eq_ignore_ascii_case("UNC\\")),
        Some(rest) => !rest.starts_with(r".\"),
        None => false,
    }
}

/// Volume root of a local drive path (`C:\data` → `C:\`, `\\?\C:\x` → `C:\`); `None` otherwise.
pub fn drive_root(path: &str) -> Option<String> {
    let p = path.strip_prefix(r"\\?\").unwrap_or(path);
    let b = p.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| format!("{}:\\", char::from(b[0])))
}

/// Normalize a user-typed serial port name: `com12` and `\\.\COM12` become `COM12`. Names that
/// are not `COM<n>` are returned unchanged (other device paths).
pub fn com_name(name: &str) -> String {
    let bare = name.trim().strip_prefix(r"\\.\").unwrap_or(name.trim());
    match bare.get(..3) {
        Some(p) if p.eq_ignore_ascii_case("COM") && bare.len() > 3 && bare[3..].bytes().all(|b| b.is_ascii_digit()) => {
            format!("COM{}", &bare[3..])
        }
        _ => bare.to_owned(),
    }
}

/// Device path to open: `\\.\COMn` (required for COM10 and above).
pub fn com_device_path(name: &str) -> String {
    let n = com_name(name);
    if n.starts_with(r"\\") { n } else { format!(r"\\.\{n}") }
}

/// Win32 error codes the serial mapping cares about.
pub mod codes {
    pub const ERROR_FILE_NOT_FOUND: i32 = 2;
    pub const ERROR_PATH_NOT_FOUND: i32 = 3;
    pub const ERROR_ACCESS_DENIED: i32 = 5;
    pub const ERROR_BAD_COMMAND: i32 = 22;
    pub const ERROR_GEN_FAILURE: i32 = 31;
    pub const ERROR_OPERATION_ABORTED: i32 = 995;
    pub const ERROR_DEVICE_REMOVED: i32 = 1617;
}

/// Serial open failure. `serialport` folds "not found" and "access denied" into `NoDevice`;
/// COM ports are exclusive, so a port that is still enumerated is held by another program.
/// Never a Unix-group hint (design W3).
pub fn serial_open_error(path: &str, no_device: bool, still_listed: bool, detail: &str) -> rxt_core::Error {
    use rxt_core::Error;
    match (no_device, still_listed) {
        (true, true) => Error::Busy(path.into()),
        (true, false) => Error::NotFound(format!("serial port {path}")),
        _ => Error::Io(format!("opening {path}: {detail}")),
    }
}

/// Whether an I/O error code on an open port means the device went away.
pub fn serial_gone(code: Option<i32>) -> bool {
    use codes::*;
    matches!(
        code,
        Some(
            ERROR_DEVICE_REMOVED
                | ERROR_BAD_COMMAND
                | ERROR_OPERATION_ABORTED
                | ERROR_GEN_FAILURE
                | ERROR_ACCESS_DENIED
                | ERROR_FILE_NOT_FOUND
        )
    )
}

/// Local shell (local-shell spec): `pwsh.exe` found on `path`, else Windows PowerShell under
/// `system_root`, else `comspec`; PowerShell starts with `-NoLogo`. `None` when none exists.
pub fn local_shell(
    path: Option<&std::ffi::OsStr>,
    system_root: &std::path::Path,
    comspec: Option<&std::ffi::OsStr>,
    is_file: impl Fn(&std::path::Path) -> bool,
) -> Option<(std::path::PathBuf, Vec<String>)> {
    let ps = || vec!["-NoLogo".to_owned()];
    let pwsh = path.into_iter().flat_map(std::env::split_paths).map(|d| d.join("pwsh.exe")).find(|p| is_file(p));
    if let Some(p) = pwsh {
        return Some((p, ps()));
    }
    let windows_ps = system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if is_file(&windows_ps) {
        return Some((windows_ps, ps()));
    }
    comspec.map(std::path::PathBuf::from).filter(|p| is_file(p)).map(|p| (p, Vec::new()))
}

/// `CreateProcessW` command line: the executable, then every argument, each quoted by the
/// `CommandLineToArgvW` rules so a value with spaces or quotes (a WSL distribution, user or folder)
/// reaches the child as one argument and nothing goes through a shell.
pub fn command_line(exe: &str, args: &[String]) -> String {
    std::iter::once(quote_arg(exe)).chain(args.iter().map(|a| quote_arg(a))).collect::<Vec<_>>().join(" ")
}

/// Quote one argument by the `CommandLineToArgvW` rules: leave it bare when it has no space, tab,
/// newline or `"`; otherwise wrap it in `"`, doubling the backslashes that precede a `"` or the
/// closing quote and escaping each `"`.
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.bytes().any(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | b'"')) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                backslashes = 0;
                out.push('"');
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// Environment block for `CREATE_UNICODE_ENVIRONMENT`: `vars` with `TERM=xterm-256color` and
/// `COLORTERM=truecolor` (replacing any case variant), sorted case-insensitively as Windows
/// expects, each `key=value` NUL-terminated, plus a final NUL.
pub fn env_block(vars: impl Iterator<Item = (String, String)>) -> Vec<u16> {
    let ours = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];
    let mut env: Vec<(String, String)> =
        vars.filter(|(k, _)| !ours.iter().any(|(o, _)| k.eq_ignore_ascii_case(o))).collect();
    env.extend(ours.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
    env.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block: Vec<u16> =
        env.iter().flat_map(|(k, v)| format!("{k}={v}\0").encode_utf16().collect::<Vec<_>>()).collect();
    block.push(0);
    block
}

/// Removes ConPTY's start-up cursor position request (`ESC[6n`, sent once with
/// `PSEUDOCONSOLE_INHERIT_CURSOR`) from its output, across chunk boundaries; the adapter answers it.
#[derive(Default)]
pub struct DsrFilter {
    pending: Vec<u8>,
    done: bool,
}

impl DsrFilter {
    const DSR: &[u8] = b"\x1b[6n";

    /// Output to pass on, and whether the request was found in this chunk.
    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, bool) {
        if self.done {
            return (chunk.to_vec(), false);
        }
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        if let Some(i) = data.windows(Self::DSR.len()).position(|w| w == Self::DSR) {
            self.done = true;
            data.drain(i..i + Self::DSR.len());
            return (data, true);
        }
        // Keep a trailing partial request for the next chunk.
        let keep = (1..Self::DSR.len()).rev().find(|&n| data.ends_with(&Self::DSR[..n])).unwrap_or(0);
        self.pending = data.split_off(data.len() - keep);
        (data, false)
    }

    /// Whether the request was seen (answered) already.
    pub fn answered(&self) -> bool {
        self.done
    }
}

/// Parse a `win32:<hex HWND>` dialog parent id.
pub fn parse_parent(parent: &str) -> Option<std::num::NonZeroIsize> {
    let hex = parent.strip_prefix("win32:")?;
    usize::from_str_radix(hex, 16).ok().and_then(|v| std::num::NonZeroIsize::new(v as isize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxt_core::Error;

    const SID: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn local_shell_prefers_pwsh_then_powershell_then_comspec() {
        use std::path::Path;
        let root = Path::new(r"C:\Windows");
        let path = std::ffi::OsString::from(if cfg!(windows) {
            r"C:\Tools;C:\Program Files\PowerShell\7"
        } else {
            "/tools:/pwsh 7"
        });
        let pwsh_dir = if cfg!(windows) { r"C:\Program Files\PowerShell\7" } else { "/pwsh 7" };
        let ps = root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let cmd = std::ffi::OsStr::new(r"C:\Windows\System32\cmd.exe");
        let pick = |files: &[&Path]| local_shell(Some(&path), root, Some(cmd), |p| files.contains(&p));
        let pwsh = Path::new(pwsh_dir).join("pwsh.exe");
        assert_eq!(pick(&[&pwsh, &ps]), Some((pwsh.clone(), vec!["-NoLogo".into()])));
        assert_eq!(pick(&[&ps]), Some((ps.clone(), vec!["-NoLogo".into()])));
        assert_eq!(pick(&[Path::new(cmd)]), Some((cmd.into(), Vec::new())));
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn command_line_quotes_the_executable() {
        assert_eq!(
            command_line(r"C:\Program Files\PowerShell\7\pwsh.exe", &["-NoLogo".into()]),
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#
        );
    }

    #[test]
    fn command_line_quotes_every_argument() {
        // A WSL launch: simple flags stay bare, a folder with a space is quoted as one argument.
        assert_eq!(
            command_line(
                r"C:\Windows\System32\wsl.exe",
                &["-d".into(), "Debian".into(), "--cd".into(), "/srv/my app".into()]
            ),
            r#"C:\Windows\System32\wsl.exe -d Debian --cd "/srv/my app""#
        );
        // Quotes and trailing backslashes follow the CommandLineToArgvW rules.
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r"a b\"), r#""a b\\""#);
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(""), r#""""#);
    }

    #[test]
    fn env_block_sets_term_and_sorts() {
        let vars = [("Path", "C:\\x"), ("term", "dumb"), ("ALLUSERSPROFILE", "C:\\P")];
        let block = env_block(vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "ALLUSERSPROFILE=C:\\P\0COLORTERM=truecolor\0Path=C:\\x\0TERM=xterm-256color\0\0");
    }

    #[test]
    fn dsr_filter_strips_the_first_request_across_chunks() {
        let mut f = DsrFilter::default();
        assert_eq!(f.feed(b"a\x1b["), (b"a".to_vec(), false));
        assert!(!f.answered());
        assert_eq!(f.feed(b"6nb"), (b"b".to_vec(), true));
        assert!(f.answered());
        assert_eq!(f.feed(b"\x1b[6n"), (b"\x1b[6n".to_vec(), false), "only the first one");
        let mut g = DsrFilter::default();
        assert_eq!(g.feed(b"\x1b[0m"), (b"\x1b[0m".to_vec(), false));
    }

    #[test]
    fn theme_value() {
        assert_eq!(prefers_dark(Some(0)), Some(true));
        assert_eq!(prefers_dark(Some(1)), Some(false));
        assert_eq!(prefers_dark(None), None);
    }

    #[test]
    fn names_and_descriptors() {
        assert_eq!(credential_target("rustxterm-db-1"), "rustxterm:rustxterm-db-1");
        assert_eq!(pipe_name(SID), r"\\.\pipe\rustxterm-S-1-5-21-1-2-3-1001-control");
        assert_eq!(pipe_sddl(SID), "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)");
        assert_eq!(private_dir_sddl(SID), "O:S-1-5-21-1-2-3-1001D:P(A;OICI;FA;;;S-1-5-21-1-2-3-1001)");
    }

    #[test]
    fn owner_only_readback() {
        let ok = |d: &str| sd_is_owner_only(&format!("O:{SID}{d}"), SID);
        assert!(sd_is_owner_only(&private_dir_sddl(SID), SID));
        assert!(ok("D:PAI(A;OICI;FA;;;S-1-5-21-1-2-3-1001)"));
        assert!(ok("D:PAI(A;ID;FA;;;S-1-5-21-1-2-3-1001)"), "inherited owner-only ACE on a file");
        // Not protected with an explicit ACE: inherited entries could be added later.
        assert!(!ok("D:AI(A;OICI;FA;;;S-1-5-21-1-2-3-1001)"));
        // A new file below a protected directory: only inherited owner-only ACEs.
        assert!(ok("D:AI(A;ID;FA;;;S-1-5-21-1-2-3-1001)"));
        assert!(!ok("D:AI(A;ID;FA;;;S-1-5-21-1-2-3-1001)(A;;FA;;;WD)"));
        // A second principal (Users) or another user.
        assert!(!ok("D:P(A;OICI;FA;;;S-1-5-21-1-2-3-1001)(A;OICI;FR;;;BU)"));
        assert!(!ok("D:P(A;OICI;FA;;;S-1-5-21-1-2-3-1002)"));
        // Empty DACL, null DACL, garbage.
        assert!(!ok("D:P"));
        assert!(!ok("D:NO_ACCESS_CONTROL"));
        assert!(!sd_is_owner_only("", SID));
        // Administrators may own (elevated creation); another user may not.
        assert!(sd_is_owner_only("O:BAD:P(A;OICI;FA;;;S-1-5-21-1-2-3-1001)", SID));
        assert!(!sd_is_owner_only("O:S-1-5-21-1-2-3-1002D:P(A;OICI;FA;;;S-1-5-21-1-2-3-1001)", SID));
        // The built-in Administrator reads back as `LA`, and only that account may match it.
        assert!(sd_is_owner_only("O:LAD:PAI(A;OICI;FA;;;LA)", "S-1-5-21-1-2-3-500"));
        assert!(sd_is_owner_only("O:BAD:PAI(A;OICI;FA;;;LA)", "S-1-5-21-1-2-3-500"));
        assert!(!sd_is_owner_only("O:LAD:PAI(A;OICI;FA;;;LA)", SID));
        assert!(owner_is(&private_dir_sddl(SID), SID));
        assert!(owner_is("O:BAD:P", SID));
        assert!(!owner_is("O:S-1-5-21-9D:P", SID));
    }

    #[test]
    fn paths_compare_without_links() {
        assert!(same_path(r"\\?\C:\Users\Vagrant\AppData", r"c:\users\vagrant\appdata\"));
        assert!(!same_path(r"\\?\D:\real\data", r"C:\Users\vagrant\link\data"));
        assert!(inside(r"\\?\C:\Users\vagrant\AppData\Local\rustxterm", r"\\?\C:\Users\vagrant"));
        assert!(inside(r"C:\Users\vagrant", r"C:\Users\vagrant\"));
        assert!(!inside(r"C:\Users\vagrant2\x", r"C:\Users\vagrant"));
        assert!(!inside(r"C:\shared\rustxterm", r"C:\Users\vagrant"));
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share"), r"\\?\UNC\srv\share");
    }

    #[test]
    fn ancestor_acls() {
        let me = SID;
        // Default profile ACL: SYSTEM, Administrators, the user; CREATOR OWNER inherit-only.
        let profile = format!("O:{me}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{me})(A;OICIIO;GA;;;CO)");
        assert!(ancestor_safe(&profile, me));
        // Runner TEMP: Administrators own it, the built-in Administrator (LA) has full control.
        assert!(ancestor_safe("O:BAD:(A;OICIID;FA;;;SY)(A;OICIID;FA;;;BA)(A;OICIID;FA;;;LA)", me));
        // Users may read and list.
        assert!(ancestor_safe(&format!("O:{me}D:AI(A;;FA;;;{me})(A;;0x1200a9;;;BU)(A;;FRFX;;;AU)"), me));
        // Users with full control, modify (DELETE), delete-child or WRITE_DAC: unsafe.
        assert!(!ancestor_safe(&format!("O:{me}D:AI(A;;FA;;;{me})(A;OICI;FA;;;BU)"), me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;0x1301bf;;;S-1-5-21-9-9-9-1002)"), me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;DT;;;WD)"), me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;WD;;;AU)"), me));
        // Another user owns it, or rights cannot be decoded.
        assert!(!ancestor_safe("O:S-1-5-21-9-9-9-1002D:(A;;FA;;;SY)", me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;ZZ;;;BU)"), me));
        // Write data / attributes (reparse-point creation) and generic write.
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;DC;;;BU)"), me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;0x100;;;BU)"), me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;LC;;;BU)"), me), "append / add subdirectory");
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;RP;;;BU)"), me), "write extended attributes");
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;GW;;;BU)"), me));
        assert!(!ancestor_safe(&format!("O:{me}D:(A;;FW;;;AU)"), me));
        // Null DACL.
        assert!(!ancestor_safe(&format!("O:{me}D:NO_ACCESS_CONTROL"), me));
        // Deny entries never widen access.
        assert!(ancestor_safe(&format!("O:{me}D:(D;;FA;;;BU)(A;;FA;;;{me})"), me));
    }

    #[test]
    fn server_elevation() {
        let me = Identity { sid: SID.into(), integrity: 0x2000, ..Identity::default() };
        assert_eq!(server_allowed(Some(&me), &me), Ok(()));
        let elevated = Identity { integrity: 0x3000, ..me.clone() };
        assert!(server_allowed(Some(&elevated), &me).unwrap_err().contains("elevation"));
        assert!(server_allowed(Some(&me), &elevated).unwrap_err().contains("elevation"));
        let other = Identity { sid: "S-1-5-21-9".into(), ..me.clone() };
        assert!(server_allowed(Some(&other), &me).unwrap_err().contains("not served"));
        assert!(server_allowed(None, &me).is_err());
        // UAC filtering without an integrity difference (W39): same message in both directions.
        let filtered = Identity { enabled: vec![], deny_only: vec!["S-1-5-32-544".into()], ..me.clone() };
        let admin = Identity { enabled: vec!["S-1-5-32-544".into()], ..me.clone() };
        assert_eq!(server_allowed(Some(&filtered), &admin), Err(DIFFERENT_ELEVATION));
        assert_eq!(server_allowed(Some(&admin), &filtered), Err(DIFFERENT_ELEVATION));
        assert_eq!(peer_refusal(Some(&filtered), &admin), Some(DIFFERENT_ELEVATION));
    }

    #[test]
    fn refusal_reasons() {
        let me = Identity { sid: SID.into(), integrity: 0x3000, ..Identity::default() };
        let with = |f: fn(&mut Identity)| {
            let mut p = me.clone();
            f(&mut p);
            peer_refusal(Some(&p), &me)
        };
        assert_eq!(with(|_| {}), None);
        assert!(peer_refusal(None, &me).is_some_and(|m| m.contains("identify")));
        assert!(with(|p| p.sid = "S-1-5-18".into()).is_some_and(|m| m.contains("only the user")));
        assert!(with(|p| p.restricted = true).is_some_and(|m| m.contains("AppContainer")));
        let me_users = Identity { enabled: vec!["S-1-5-32-545".into()], ..me.clone() };
        let sandbox = Identity { deny_only: vec!["S-1-5-32-545".into()], ..me_users.clone() };
        assert!(peer_refusal(Some(&sandbox), &me_users).is_some_and(|m| m.contains("filtered")));
        assert!(with(|p| p.integrity = 0x1000).is_some_and(|m| m.contains("low-integrity")));
        assert_eq!(with(|p| p.integrity = 0x2000), Some(DIFFERENT_ELEVATION), "non-elevated client, elevated GUI");
    }

    #[test]
    fn filtered_peers() {
        let me = Identity {
            sid: SID.into(),
            integrity: 0x2000,
            enabled: vec!["S-1-5-32-545".into()],
            deny_only: vec!["S-1-5-32-544".into()],
            ..Identity::default()
        };
        // Same UAC filtering as the server: accepted.
        assert!(peer_allowed(Some(&me), &me));
        // A sandbox that made Users deny-only: refused.
        let sandbox = Identity { deny_only: vec!["S-1-5-32-545".into()], ..me.clone() };
        assert!(!peer_allowed(Some(&sandbox), &me));
        // An elevated peer (Administrators enabled, high integrity): accepted.
        let elevated = Identity {
            integrity: 0x3000,
            enabled: vec!["S-1-5-32-545".into(), "S-1-5-32-544".into()],
            deny_only: vec![],
            ..me.clone()
        };
        assert!(peer_allowed(Some(&elevated), &me));
    }

    #[test]
    fn pipe_peers() {
        let server = Identity { sid: SID.into(), integrity: 0x2000, ..Identity::default() };
        let with = |sid: &str, restricted, integrity| Identity {
            sid: sid.into(),
            restricted,
            integrity,
            ..Identity::default()
        };
        assert!(peer_allowed(Some(&server), &server));
        assert!(peer_allowed(Some(&with(SID, false, 0x3000)), &server), "elevated client of a normal server");
        assert!(!peer_allowed(Some(&with(SID, true, 0x2000)), &server), "restricted/AppContainer token");
        assert!(!peer_allowed(Some(&with(SID, false, 0x1000)), &server), "low integrity");
        assert!(!peer_allowed(Some(&with("S-1-5-18", false, 0x4000)), &server), "another user");
        assert!(!peer_allowed(None, &server), "unknown identity");
        assert_eq!(integrity_rid("S-1-16-8192"), Some(0x2000));
        assert_eq!(integrity_rid("S-1-5-18"), None);
    }

    #[test]
    fn network_and_drive_paths() {
        assert!(is_unc(r"\\server\share\rx"));
        assert!(is_unc("//server/share"));
        assert!(is_unc(r"\\?\UNC\server\share"));
        assert!(!is_unc(r"\\?\C:\data"));
        assert!(!is_unc(r"\\.\COM3"));
        assert!(!is_unc(r"C:\Users\a"));
        assert_eq!(drive_root(r"C:\Users\a").as_deref(), Some(r"C:\"));
        assert_eq!(drive_root(r"\\?\D:\x").as_deref(), Some(r"D:\"));
        assert_eq!(drive_root(r"\\server\share"), None);
        assert_eq!(drive_root("relative"), None);
    }

    #[test]
    fn com_port_names() {
        assert_eq!(com_name("COM3"), "COM3");
        assert_eq!(com_name("com12"), "COM12");
        assert_eq!(com_name(r"\\.\COM12"), "COM12");
        assert_eq!(com_name(" com4 "), "COM4");
        assert_eq!(com_name("COMX"), "COMX");
        assert_eq!(com_device_path("com12"), r"\\.\COM12");
        assert_eq!(com_device_path("COM3"), r"\\.\COM3");
    }

    #[test]
    fn serial_errors() {
        use codes::*;
        assert!(matches!(serial_open_error("COM3", true, true, ""), Error::Busy(_)));
        assert!(matches!(serial_open_error("COM3", true, false, ""), Error::NotFound(_)));
        let other = serial_open_error("COM3", false, true, "bad parameter");
        assert!(matches!(&other, Error::Io(m) if !m.contains("group")), "{other}");
        assert!(serial_gone(Some(ERROR_DEVICE_REMOVED)));
        assert!(serial_gone(Some(ERROR_OPERATION_ABORTED)));
        assert!(!serial_gone(Some(87)));
        assert!(!serial_gone(None));
    }

    #[test]
    fn dialog_parent_ids() {
        assert_eq!(parse_parent("win32:2b04e2").map(|h| h.get()), Some(0x2b04e2));
        assert_eq!(parse_parent("win32:0"), None);
        assert_eq!(parse_parent("x11:1f"), None);
        assert_eq!(parse_parent("win32:zz"), None);
    }
}
