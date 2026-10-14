//! MobaXterm.ini bookmark importer (design D9): preview report, never reads password sections,
//! never modifies the source file.
//!
//! Bookmark sections are `[Bookmarks]` and `[Bookmarks_N]`; `SubRep=` holds the folder path
//! (`\`-separated) and `ImgNum=` the folder icon (ignored). Each other line is
//! `name=#<icon>#<type>%<field>%...#<terminal field>%...#...`. `<icon>` is only the tree icon
//! (109 ssh, 140 sftp, ...); the session type is the first `%` field, numbered in the order of the
//! session editor's type row. `;`, `%`, `#` and `|` are escaped as `__PTVIRG__`, `__PERCENT__`,
//! `__DIEZE__` and `__PIPE__`; jump-host fields hold one `|`-joined entry per hop.
//! Field positions below are counted in the `%`-separated list that starts with `<type>`.

use std::path::Path;

use rxt_core::{DataBits, FlowControl, FtpTls, JumpHost, LineConfig, Parity, Protocol, Result, Session, StopBits};

/// One entry that was not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// 1-based line number in the source file.
    pub line: usize,
    /// Bookmark name (empty when the line has none).
    pub name: String,
    pub reason: String,
}

/// Import preview. Nothing is written anywhere until the caller confirms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// `(folder path with `/` separators, "" for the root, session)`. Session ids and folders are unset.
    pub sessions: Vec<(String, Session)>,
    /// Distinct non-empty folder paths, in file order.
    pub folders: Vec<String>,
    pub skipped: Vec<Skipped>,
    /// Imported entries that need attention (unparseable serial settings, Windows port names).
    pub warnings: Vec<Skipped>,
}

// Field positions, checked against files with distinct marker values per field; checkboxes are `-1` on, `0` off. VNC has no user field.
const TYPE: usize = 0;
const HOST: usize = 1;
const PORT: usize = 2;
const USER: usize = 3;
const SSH_X11: usize = 5;
const SSH_COMPRESSION: usize = 6;
const SSH_REMOTE_COMMAND: usize = 7;
const SSH_GATEWAY_HOST: usize = 8;
const SSH_GATEWAY_PORT: usize = 9;
const SSH_GATEWAY_USER: usize = 10;
const SSH_KEY_FILE: usize = 14;
const SSH_GATEWAY_KEY_FILE: usize = 15;
const FTP_STARTUP_FOLDER: usize = 5;
const FTP_FTPS: usize = 6;
const FTP_IMPLICIT_TLS: usize = 19;
// Serial (type 8): inferred, NOT yet confirmed against a real file (add-serial-sessions 1.7).
const SERIAL_PORT: usize = 1;
const SERIAL_SPEED: usize = 2;
const SERIAL_DATA_BITS: usize = 3;
const SERIAL_PARITY: usize = 4;
const SERIAL_STOP_BITS: usize = 5;
const SERIAL_FLOW: usize = 6;
const TERM_FONT: usize = 0;
const TERM_FONT_SIZE: usize = 1;

/// Read `path` (read-only) and build the preview. Windows-1252/Latin-1 files are decoded
/// byte-for-byte when they are not valid UTF-8.
pub fn import_file(path: &Path) -> Result<ImportReport> {
    let bytes = std::fs::read(path)?;
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        // ponytail: Latin-1 decoding; cp1252 bytes 0x80-0x9F (€, curly quotes) map to C1 controls.
        Err(e) => e.into_bytes().iter().map(|&b| char::from(b)).collect(),
    };
    Ok(parse(&text))
}

/// Parse ini text. Only bookmark sections are examined; sections whose name mentions
/// passwords or credentials are skipped without looking at their lines.
pub fn parse(text: &str) -> ImportReport {
    let mut report = ImportReport::default();
    let mut in_bookmarks = false;
    let mut folder = String::new();
    for (i, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = raw.trim();
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let lower = section.to_ascii_lowercase();
            let sensitive = lower.contains("pass") || lower.contains("cred");
            in_bookmarks = !sensitive && (lower == "bookmarks" || lower.starts_with("bookmarks_"));
            folder.clear();
            continue;
        }
        if !in_bookmarks || line.is_empty() || line.starts_with(';') {
            continue;
        }
        let skip = |name: &str, reason: String| Skipped { line: i + 1, name: name.to_owned(), reason };
        let Some((name, value)) = line.split_once('=') else {
            report.skipped.push(skip("", "line has no `=`".into()));
            continue;
        };
        let name = unescape(name.trim());
        match name.as_str() {
            // ponytail: SubRep applies to the lines after it (the app writes it first in each section).
            "SubRep" => {
                folder = value.trim().split('\\').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/");
                if !folder.is_empty() && !report.folders.contains(&folder) {
                    report.folders.push(folder.clone());
                }
            }
            "ImgNum" => {}
            _ => match bookmark(&name, value.trim()) {
                Ok((session, warnings)) => {
                    report.warnings.extend(warnings.into_iter().map(|w| skip(&name, w)));
                    report.sessions.push((folder.clone(), session));
                }
                Err(reason) => report.skipped.push(skip(&name, reason)),
            },
        }
    }
    report
}

fn unescape(s: &str) -> String {
    // ponytail: the app writes a literal `__PTVIRG__` unescaped, so this is lossy for that text.
    s.replace("__PTVIRG__", ";").replace("__PERCENT__", "%").replace("__DIEZE__", "#").replace("__PIPE__", "|")
}

/// Session plus import warnings.
fn bookmark(name: &str, value: &str) -> std::result::Result<(Session, Vec<String>), String> {
    let mut parts = value.strip_prefix('#').ok_or("not a bookmark (missing `#<icon>#`)")?.split('#');
    let _icon = parts.next();
    let fields: Vec<String> = parts.next().unwrap_or_default().split('%').map(unescape).collect();
    let code = fields[TYPE].trim();
    let code: u32 = code.parse().map_err(|_| format!("invalid session type `{code}`"))?;
    let mut protocol = match code {
        0 => Protocol::Ssh,
        4 => Protocol::Rdp,
        5 => Protocol::Vnc,
        6 => Protocol::Ftp,
        7 => Protocol::Sftp,
        1 => Protocol::Telnet,
        8 => return serial(name, &fields),
        2 => return Err("unsupported session type rsh (2)".into()),
        11 => return Err("unsupported session type browser (11)".into()),
        other => return Err(format!("unsupported session type {other}")),
    };
    if fields.len() <= PORT {
        return Err(format!("fewer fields than expected ({} < {})", fields.len(), PORT + 1));
    }
    let get = |i: usize| fields.get(i).map(|s| s.trim()).filter(|s| !s.is_empty());
    let flag = |i| get(i) == Some("-1");
    if protocol == Protocol::Ftp && flag(FTP_FTPS) {
        protocol = Protocol::Ftps;
    }
    let host = get(HOST).ok_or("empty host")?;
    let port = match get(PORT) {
        None => protocol.default_port(),
        Some(p) => p.parse().map_err(|_| format!("invalid port `{p}`"))?,
    };
    let user = get(USER).filter(|_| protocol != Protocol::Vnc).map(str::to_owned);
    let mut s = Session::new(protocol, host, user, Some(port));
    s.name = name.to_owned();

    match protocol {
        Protocol::Ssh => {
            s.ssh.x11_forwarding = flag(SSH_X11);
            s.ssh.compression = flag(SSH_COMPRESSION);
            s.ssh.remote_command = get(SSH_REMOTE_COMMAND).map(str::to_owned);
            s.ssh.key_file = get(SSH_KEY_FILE).map(str::to_owned);
            let hop =
                |i: usize, n: usize| get(i).and_then(|v| v.split('|').nth(n)).map(str::trim).filter(|v| !v.is_empty());
            let hops = get(SSH_GATEWAY_HOST).map_or(0, |v| v.split('|').count());
            for n in 0..hops {
                let Some(host) = hop(SSH_GATEWAY_HOST, n) else { continue };
                s.ssh.jump_hosts.push(JumpHost {
                    host: host.to_owned(),
                    port: hop(SSH_GATEWAY_PORT, n).and_then(|p| p.parse().ok()).unwrap_or(22),
                    user: hop(SSH_GATEWAY_USER, n).unwrap_or_default().to_owned(),
                    key_file: hop(SSH_GATEWAY_KEY_FILE, n).map(str::to_owned),
                });
            }
            let term: Vec<String> = parts.next().unwrap_or_default().split('%').map(unescape).collect();
            let tget = |i: usize| term.get(i).map(|s| s.trim()).filter(|s| !s.is_empty());
            // `MobaFont` is not available here: keep the default font instead.
            s.terminal.font_family = tget(TERM_FONT).filter(|f| !f.eq_ignore_ascii_case("MobaFont")).map(str::to_owned);
            s.terminal.font_size = tget(TERM_FONT_SIZE).and_then(|v| v.parse::<f32>().ok()).filter(|v| *v > 0.0);
        }
        Protocol::Ftp | Protocol::Ftps => {
            s.ftp.startup_folder = get(FTP_STARTUP_FOLDER).map(str::to_owned);
            s.ftp.tls = if flag(FTP_IMPLICIT_TLS) { FtpTls::Implicit } else { FtpTls::Explicit };
        }
        // Auto-login and passwords are never imported (telnet `auto_login` stays off).
        _ => {}
    }
    Ok((s, Vec::new()))
}

/// Serial bookmark: the port is always imported; unparseable line settings keep their
/// defaults and are reported.
fn serial(name: &str, fields: &[String]) -> std::result::Result<(Session, Vec<String>), String> {
    let get = |i: usize| fields.get(i).map(|s| s.trim()).filter(|s| !s.is_empty());
    // The app may append a description after the port name ("COM3  (USB Serial Port)").
    let port = get(SERIAL_PORT).and_then(|p| p.split_whitespace().next()).ok_or("empty serial port")?;
    let mut warnings = Vec::new();
    let mut line = LineConfig::default();
    let mut setting = |what: &str, i: usize, apply: &mut dyn FnMut(&str) -> bool| {
        if let Some(v) = get(i)
            && !apply(v)
        {
            warnings.push(format!("unrecognized {what} `{v}`; using the default"));
        }
    };
    setting("speed", SERIAL_SPEED, &mut |v| v.parse().ok().filter(|b| *b > 0).map(|b| line.baud = b).is_some());
    setting("data bits", SERIAL_DATA_BITS, &mut |v| {
        v.parse().ok().and_then(DataBits::from_bits).map(|d| line.data_bits = d).is_some()
    });
    setting("parity", SERIAL_PARITY, &mut |v| Parity::parse(v).map(|p| line.parity = p).is_some());
    setting("stop bits", SERIAL_STOP_BITS, &mut |v| {
        let stop = match v {
            "1" => Some(StopBits::One),
            "2" => Some(StopBits::Two),
            _ => None,
        };
        stop.map(|s| line.stop_bits = s).is_some()
    });
    setting("flow control", SERIAL_FLOW, &mut |v| FlowControl::parse(v).map(|f| line.flow = f).is_some());
    let windows =
        port.len() > 3 && port[..3].eq_ignore_ascii_case("COM") && port[3..].chars().all(|c| c.is_ascii_digit());
    if windows {
        warnings
            .push(format!("Windows port name {port} must be replaced with a Linux device path (e.g. /dev/ttyUSB0)"));
    }
    let mut s = Session::serial(port, line);
    s.name = name.to_owned();
    Ok((s, warnings))
}
