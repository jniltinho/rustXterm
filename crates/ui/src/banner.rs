//! Session info box written into a terminal when an SSH or Telnet tab connects, and the welcome
//! box of a local terminal before its shell starts. It is fed to the local terminal model only;
//! nothing is sent to the server or the shell.

use rxt_core::{LaunchInfo, Protocol, Session, SshInfo, TerminalScheme};

/// What the box describes.
pub(crate) struct BannerInfo<'a> {
    pub session: &'a Session,
    /// Connection facts of SSH tabs; `None` for Telnet.
    pub ssh: Option<&'a SshInfo>,
    /// What a local terminal starts (local terminals only).
    pub local: Option<&'a LaunchInfo>,
    /// Tips listed in a local terminal's box, e.g. `Ctrl+Shift+N: new session`.
    pub tips: Vec<String>,
    /// One line of tips for the bottom border, e.g. `Ctrl+wheel: zoom`.
    pub hint: String,
    /// Box-drawing characters and check marks; ASCII when the font lacks them.
    pub unicode: bool,
}

/// SGR colors by role: `(light scheme, dark scheme)`.
const BORDER: (u8, u8) = (33, 93);
const TITLE: (u8, u8) = (36, 96);
const TARGET: (u8, u8) = (35, 95);
const YES: (u8, u8) = (32, 92);
const NO: (u8, u8) = (31, 91);
const MUTED: (u8, u8) = (90, 90);
/// Left margin of the box (MobaXterm-like), dropped on narrow terminals.
const INDENT: usize = 4;

/// One styled piece of a line: (role color, text).
type Seg = (Option<(u8, u8)>, String);

fn is_dark(scheme: &TerminalScheme) -> bool {
    let b = scheme.background;
    u32::from(b.0) * 299 + u32::from(b.1) * 587 + u32::from(b.2) * 114 < 128_000
}

/// Server-provided text without control characters.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

fn host_port(host: &str, port: u16, default: u16) -> String {
    if port == 0 || port == default { clean(host) } else { format!("{}:{port}", clean(host)) }
}

/// The box as terminal bytes (CRLF line ends, ending with a blank line), at most `cols` wide.
// ponytail: one cell per char; wide (CJK) host names may overflow the right border.
pub(crate) fn render(info: &BannerInfo, cols: usize, scheme: &TerminalScheme) -> Vec<u8> {
    if cols < 6 {
        return Vec::new();
    }
    let dark = is_dark(scheme);
    let u = info.unicode;
    let (yes, no) = if u { ("✓", "✗") } else { ("+", "-") };
    let check = |on: bool, label: &str, note: String| -> Vec<Seg> {
        let mut line = vec![(Some(if on { YES } else { NO }), (if on { yes } else { no }).to_owned())];
        line.push((None, format!(" {label}")));
        if !note.is_empty() {
            line.push((Some(MUTED), format!(" ({note})")));
        }
        line
    };
    let s = info.session;
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    match s.protocol {
        Protocol::Local => {
            let l = info.local.cloned().unwrap_or_default();
            let target = format!("{}@{}", clean(&l.user), clean(&l.host));
            lines.push(vec![(None, "Local terminal on ".into()), (Some(TARGET), target)]);
            lines.push(vec![(Some(MUTED), "Shell:  ".into()), (None, clean(&l.shell))]);
            lines.push(vec![(Some(MUTED), "Folder: ".into()), (None, clean(&l.cwd))]);
            let bullet = if u { "• " } else { "* " };
            lines.extend(info.tips.iter().map(|t| vec![(Some(MUTED), format!("{bullet}{}", clean(t)))]));
        }
        Protocol::Telnet => {
            let target = host_port(&s.host, s.port, Protocol::Telnet.default_port());
            lines.push(vec![(None, "Telnet session to ".into()), (Some(TARGET), target)]);
            lines.push(check(false, "Encryption", "traffic and passwords are sent in clear text".into()));
        }
        _ => {
            let user = s.user.as_deref().map(|u| format!("{}@", clean(u))).unwrap_or_default();
            let target = format!("{user}{}", host_port(&s.host, s.port, 22));
            lines.push(vec![(None, "SSH session to ".into()), (Some(TARGET), target)]);
            let jumps = &s.ssh.jump_hosts;
            if !jumps.is_empty() {
                let arrow = if u { " → " } else { " -> " };
                let chain: Vec<String> = jumps
                    .iter()
                    .map(|j| {
                        let user = if j.user.is_empty() { String::new() } else { format!("{}@", clean(&j.user)) };
                        format!("{user}{}", host_port(&j.host, j.port, 22))
                    })
                    .collect();
                lines.push(vec![(Some(MUTED), format!("  via {}", chain.join(arrow)))]);
            }
            let ssh = info.ssh.cloned().unwrap_or_default();
            let route = match jumps.len() {
                0 => check(true, "Direct connection", String::new()),
                1 => check(true, "Gateway connection", "1 jump host".into()),
                n => check(true, "Gateway connection", format!("{n} jump hosts")),
            };
            lines.push(route);
            lines.push(check(
                ssh.compression,
                "Compression",
                if ssh.compression { "zlib".into() } else { String::new() },
            ));
            lines.push(match ssh.sftp {
                Some(true) => check(true, "SFTP browser", String::new()),
                Some(false) => check(false, "SFTP browser", "not offered by the server".into()),
                None => check(true, "SFTP browser", "opens on demand".into()),
            });
            let x11_note = match &ssh.x11_unavailable {
                Some(why) if s.ssh.x11_forwarding => clean(why),
                _ => String::new(),
            };
            lines.push(check(ssh.x11, "X11 forwarding", x11_note));
            lines.push(check(ssh.agent, "Agent forwarding", String::new()));
            let keep = s.ssh.keepalive_secs;
            lines.push(check(keep > 0, "Keepalive", if keep > 0 { format!("every {keep} s") } else { String::new() }));
            // Following the terminal folder moves the SFTP browser: off without SFTP.
            let no_sftp = ssh.sftp == Some(false);
            let follow_note = if no_sftp && s.ssh.follow_terminal_folder { "needs SFTP" } else { "" };
            lines.push(check(s.ssh.follow_terminal_folder && !no_sftp, "Follow terminal folder", follow_note.into()));
            if !ssh.server_id.is_empty() || !ssh.host_key_fingerprint.is_empty() {
                let mut text = clean(&ssh.server_id);
                if !ssh.host_key_fingerprint.is_empty() {
                    let fp = clean(&ssh.host_key_fingerprint);
                    let short: String = fp.chars().take(23).collect();
                    let ell = if short.len() < fp.len() { if u { "…" } else { "..." } } else { "" };
                    let sep = if text.is_empty() { "" } else { ", " };
                    text = format!("{text}{sep}{} {short}{ell}", clean(&ssh.host_key_algorithm));
                }
                lines.push(vec![(Some(MUTED), format!("Server: {text}"))]);
            }
        }
    }

    let width = |l: &[Seg]| l.iter().map(|(_, t)| t.chars().count()).sum::<usize>();
    let title = format!("rustXterm {}", env!("CARGO_PKG_VERSION"));
    let content = lines.iter().map(|l| width(l)).max().unwrap_or(0).max(title.chars().count() + 2);
    // `| ` + content + ` |` must fit in `cols`. The box is always drawn with ASCII dashes (owner
    // choice); `unicode` only selects the marks and arrows inside it.
    let pad = if cols >= 40 { " ".repeat(INDENT) } else { String::new() };
    let inner = content.min(cols - 4 - pad.len());
    let (h, v, tl, tr, bl, br) = ('-', '|', '+', '+', '+', '+');
    let sgr = |role: (u8, u8)| format!("\x1b[{}m", if dark { role.1 } else { role.0 });
    let border = sgr(BORDER);
    let reset = "\x1b[0m";
    let ell = if u { '…' } else { '~' };

    let mut out = String::new();
    // Top/bottom border with an embedded label: `+- label ---+`, total width inner + 4.
    let rule = |out: &mut String, left: char, right: char, label: &str, role: (u8, u8)| {
        let label = fit(label, inner.saturating_sub(2), ell);
        out.push_str(&pad);
        out.push_str(&border);
        out.push(left);
        out.push(h);
        let mut used = 1;
        if !label.is_empty() {
            out.push_str(&format!(" {}\x1b[1m{label}{reset}{border} ", sgr(role)));
            used += label.chars().count() + 2;
        }
        out.extend(std::iter::repeat_n(h, inner + 2 - used.min(inner + 2)));
        out.push(right);
        out.push_str(reset);
        out.push_str("\r\n");
    };
    rule(&mut out, tl, tr, &title, TITLE);
    for line in &lines {
        out.push_str(&format!("{pad}{border}{v}{reset} "));
        let mut left = inner;
        let full = width(line) <= inner;
        for (role, text) in line {
            if left == 0 {
                break;
            }
            // The segment that overflows ends with the ellipsis; later ones are dropped.
            let piece: String = if full || text.chars().count() < left {
                text.clone()
            } else {
                text.chars().take(left - 1).chain([ell]).collect()
            };
            left -= piece.chars().count();
            match role {
                Some(r) => out.push_str(&format!("{}{piece}{reset}", sgr(*r))),
                None => out.push_str(&piece),
            }
        }
        out.extend(std::iter::repeat_n(' ', left));
        out.push_str(&format!(" {border}{v}{reset}\r\n"));
    }
    // The hint is dropped rather than cut when it does not fit.
    let hint = if info.hint.chars().count() + 2 <= inner { info.hint.as_str() } else { "" };
    rule(&mut out, bl, br, hint, MUTED);
    out.push_str("\r\n");
    out.into_bytes()
}

/// `text` cut to `max` chars, ending with `ell` when shortened.
fn fit(text: &str, max: usize, ell: char) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    match max {
        0 => String::new(),
        _ => text.chars().take(max - 1).chain([ell]).collect(),
    }
}

/// Notice written into the terminal when its session stops: a rule, then the keys
/// the stopped tab answers to (see [`crate::tabs::ended_session_key`]).
pub(crate) fn stopped(cols: usize, scheme: &TerminalScheme, why: &str, next: Option<&str>) -> Vec<u8> {
    let pick = |c: (u8, u8)| if is_dark(scheme) { c.1 } else { c.0 };
    let (red, key, muted) = (pick(NO), pick(TARGET), pick(MUTED));
    // ASCII dashes in red whatever the font (owner choice, like the session box).
    let rule = "-".repeat(cols.saturating_sub(1).max(1));
    let mut out = format!("\r\n\x1b[{red}m{rule}\x1b[0m\r\n\x1b[{red}mSession stopped\x1b[0m");
    if !why.is_empty() {
        out += &format!(" \x1b[{muted}m({})\x1b[0m", clean(why));
    }
    out += "\r\n";
    for (k, what) in
        [("<Return>", "to exit tab"), ("R", "to restart session"), ("S", "to save terminal output to file")]
    {
        out += &format!("    - Press \x1b[{key}m{k}\x1b[0m {what}\r\n");
    }
    if let Some(next) = next {
        out += &format!("    - Next automatic attempt at {}\r\n", clean(next));
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use rxt_core::JumpHost;

    use super::*;

    fn ssh_session() -> Session {
        let mut s = Session::new(Protocol::Ssh, "lab.example", Some("alice".into()), Some(2222));
        s.ssh.keepalive_secs = 30;
        s
    }

    fn facts() -> SshInfo {
        SshInfo {
            server_id: "SSH-2.0-OpenSSH_9.6".into(),
            compression: true,
            host_key_algorithm: "ssh-ed25519".into(),
            host_key_fingerprint: "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG".into(),
            agent: true,
            ..SshInfo::default()
        }
    }

    fn draw(session: &Session, ssh: Option<&SshInfo>, cols: usize, unicode: bool, dark: bool) -> String {
        let info = BannerInfo { session, ssh, local: None, tips: Vec::new(), hint: "Ctrl+wheel: zoom".into(), unicode };
        String::from_utf8(render(&info, cols, &TerminalScheme::for_dark(dark))).unwrap()
    }

    /// Visible text per line (SGR sequences removed).
    fn plain(s: &str) -> Vec<String> {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                (false, c) => out.push(c),
            }
        }
        out.split("\r\n").map(str::to_owned).collect()
    }

    #[test]
    fn every_line_fits_the_width() {
        for cols in [40, 80, 200] {
            let text = draw(&ssh_session(), Some(&facts()), cols, true, false);
            let lines = plain(&text);
            assert!(lines.len() <= 13, "{lines:?}");
            let boxed: Vec<_> = lines.iter().filter(|l| !l.is_empty()).collect();
            let w = boxed[0].chars().count();
            assert!(w <= cols, "{cols}: {w}");
            assert!(boxed.iter().all(|l| l.chars().count() == w), "{cols}: {boxed:#?}");
            assert!(boxed[0].starts_with("    +- rustXterm ") && boxed[0].ends_with('+'));
            assert!(boxed.last().unwrap().starts_with("    +"));
        }
        let wide = plain(&draw(&ssh_session(), Some(&facts()), 200, true, false)).join("\n");
        for want in [
            "SSH session to alice@lab.example:2222",
            "✓ Direct connection",
            "✓ Compression (zlib)",
            "✓ SFTP browser",
            "✗ X11 forwarding",
            "✓ Agent forwarding",
            "✓ Keepalive (every 30 s)",
            "✓ Follow terminal folder",
            "Server: SSH-2.0-OpenSSH_9.6, ssh-ed25519 SHA256:abcdefghijklmnop…",
            "Ctrl+wheel: zoom",
        ] {
            assert!(wide.contains(want), "missing {want:?} in\n{wide}");
        }
        let narrow = plain(&draw(&ssh_session(), Some(&facts()), 40, true, false)).join("\n");
        assert!(narrow.contains('…'), "{narrow}");
        assert!(draw(&ssh_session(), None, 3, true, false).is_empty());
    }

    #[test]
    fn ascii_fallback_has_no_box_drawing_or_marks() {
        let text = draw(&ssh_session(), Some(&facts()), 80, false, false);
        assert!(plain(&text).iter().all(|l| l.is_ascii()), "{text}");
        assert!(plain(&text)[0].starts_with("    +- rustXterm "));
        let lines = plain(&text).join("\n");
        assert!(lines.contains("+ Direct connection") && lines.contains("- X11 forwarding"), "{lines}");
    }

    #[test]
    fn colors_follow_light_and_dark_schemes() {
        let light = draw(&ssh_session(), Some(&facts()), 80, true, false);
        let dark = draw(&ssh_session(), Some(&facts()), 80, true, true);
        assert!(light.contains("\x1b[32m✓") && light.contains("\x1b[31m✗") && light.contains("\x1b[33m+-"));
        assert!(dark.contains("\x1b[92m✓") && dark.contains("\x1b[91m✗") && dark.contains("\x1b[93m+-"));
        assert!(!light.contains("\x1b[92m") && !dark.contains("\x1b[32m"));
    }

    #[test]
    fn gateway_chain_and_x11_reason() {
        let mut s = ssh_session();
        s.ssh.jump_hosts = vec![
            JumpHost { host: "gw1".into(), port: 22, user: "bob".into(), key_file: None },
            JumpHost { host: "gw2".into(), port: 2200, user: String::new(), key_file: None },
        ];
        s.ssh.x11_forwarding = true;
        let info = SshInfo { x11_unavailable: Some("no usable $DISPLAY".into()), ..facts() };
        let text = plain(&draw(&s, Some(&info), 120, true, false)).join("\n");
        assert!(text.contains("via bob@gw1 → gw2:2200"), "{text}");
        assert!(text.contains("✓ Gateway connection (2 jump hosts)"), "{text}");
        assert!(text.contains("✗ X11 forwarding (no usable $DISPLAY)"), "{text}");
        let on = plain(&draw(&s, Some(&SshInfo { x11: true, ..facts() }), 120, true, false)).join("\n");
        assert!(on.contains("✓ X11 forwarding\u{20}"), "{on}");
    }

    #[test]
    fn sftp_line_follows_the_login_probe() {
        let s = ssh_session();
        let line = |sftp| plain(&draw(&s, Some(&SshInfo { sftp, ..facts() }), 120, true, false)).join("\n");
        assert!(line(Some(false)).contains("✗ SFTP browser (not offered by the server)"));
        assert!(line(Some(false)).contains("✗ Follow terminal folder (needs SFTP)"));
        assert!(line(Some(true)).contains("✓ Follow terminal folder"));
        assert!(line(Some(true)).contains("✓ SFTP browser\u{20}"));
        assert!(line(None).contains("✓ SFTP browser (opens on demand)"));
    }

    #[test]
    fn telnet_box_warns_and_server_text_is_cleaned() {
        let t = Session::new(Protocol::Telnet, "router\x1b[2J", None, None);
        let text = draw(&t, None, 80, true, false);
        let lines = plain(&text).join("\n");
        assert!(lines.contains("Telnet session to router[2J"), "{lines}");
        assert!(lines.contains("✗ Encryption"));
        assert!(!text.contains("\x1b[2J"));
    }

    #[test]
    fn local_welcome_box_narrow_and_wide() {
        let session = Session::new(Protocol::Local, "workstation", Some("alice".into()), None);
        let launch = LaunchInfo {
            shell: "/bin/bash".into(),
            cwd: "/home/alice".into(),
            user: "alice".into(),
            host: "workstation".into(),
            ..LaunchInfo::default()
        };
        let tips = vec!["Ctrl+Shift+N: new session".to_owned(), "Ctrl+right click: terminal menu".to_owned()];
        for cols in [40, 120] {
            let info = BannerInfo {
                session: &session,
                ssh: None,
                local: Some(&launch),
                tips: tips.clone(),
                hint: "Ctrl+wheel: zoom".into(),
                unicode: true,
            };
            let lines = plain(&String::from_utf8(render(&info, cols, &TerminalScheme::for_dark(false))).unwrap());
            let boxed: Vec<_> = lines.iter().filter(|l| !l.is_empty()).collect();
            assert!(boxed.iter().all(|l| l.chars().count() <= cols), "{cols}: {boxed:#?}");
            let text = lines.join("\n");
            assert!(text.contains(&format!("rustXterm {}", env!("CARGO_PKG_VERSION"))), "{text}");
            assert!(text.contains("Local terminal on alice@workstation") || cols < 60, "{text}");
            if cols >= 60 {
                for want in
                    ["Shell:  /bin/bash", "Folder: /home/alice", "• Ctrl+Shift+N: new session", "• Ctrl+right click"]
                {
                    assert!(text.contains(want), "missing {want:?} in\n{text}");
                }
            }
            assert!(!text.contains("SSH session"), "{text}");
        }
    }
}
