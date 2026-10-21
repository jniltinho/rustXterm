//! MobaXterm.ini importer tests against hand-written fictional fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use rxt_core::{DataBits, FlowControl, FtpTls, LineConfig, Newline, Parity, Protocol, StopBits};
use rxt_store::moba_ini::{ImportReport, import_file};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/moba").join(name)
}

fn import(name: &str) -> ImportReport {
    import_file(&fixture(name)).unwrap()
}

#[test]
fn ten_ssh_in_two_folders_and_one_skipped_rsh() {
    let r = import("basic.ini");
    assert_eq!(r.sessions.len(), 10);
    assert!(r.sessions.iter().all(|(_, s)| s.protocol == Protocol::Ssh));
    assert_eq!(r.folders, ["Production", "Lab/Staging"]);
    assert_eq!(r.sessions.iter().filter(|(f, _)| f == "Production").count(), 5);
    assert_eq!(r.sessions.iter().filter(|(f, _)| f == "Lab/Staging").count(), 5);
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].name, "Old rsh box");
    assert_eq!(r.skipped[0].line, 8);
    assert!(r.skipped[0].reason.contains("rsh"));

    let (_, db) = r.sessions.iter().find(|(_, s)| s.name == "db-01").unwrap();
    assert_eq!((db.host.as_str(), db.port, db.user.as_deref()), ("db01.example", 2222, Some("bob")));
    let (_, mon) = r.sessions.iter().find(|(_, s)| s.name == "stage-mon").unwrap();
    assert_eq!(mon.user, None);
    assert!(!db.ssh.x11_forwarding && !db.ssh.compression && db.ssh.jump_hosts.is_empty());
}

#[test]
fn each_supported_type_and_ssh_fields() {
    let r = import("types.ini");
    let get = |n: &str| &r.sessions.iter().find(|(_, s)| s.name == n).unwrap().1;

    let ssh = get("full-ssh");
    assert_eq!((ssh.protocol, ssh.host.as_str(), ssh.port), (Protocol::Ssh, "app.example", 2022));
    assert_eq!(ssh.user.as_deref(), Some("alice"));
    assert!(ssh.ssh.x11_forwarding && ssh.ssh.compression);
    assert_eq!(ssh.ssh.remote_command.as_deref(), Some("tmux attach; echo 50% #1 | cat"));
    assert_eq!(ssh.ssh.key_file.as_deref(), Some(r"_ProfileDir_\.ssh\id_ed25519"));
    let hops: Vec<_> =
        ssh.ssh.jump_hosts.iter().map(|j| (j.host.as_str(), j.port, j.user.as_str(), j.key_file.as_deref())).collect();
    assert_eq!(
        hops,
        [("bastion.example", 2200, "bob", Some(r"_ProfileDir_\.ssh\gw_key")), ("hop2.example", 22, "carol", None)]
    );
    assert_eq!(ssh.terminal.font_family.as_deref(), Some("Hack"));
    assert_eq!(ssh.terminal.font_size, Some(12.0));
    // `MobaFont` is not available here: the default font is kept.
    assert_eq!(get("files").terminal.font_family, None);

    for (name, proto, host, port, user) in [
        ("files", Protocol::Sftp, "files.example", 22, Some("bob")),
        ("ftp-site", Protocol::Ftp, "ftp.example", 21, Some("alice")),
        ("ftps-explicit", Protocol::Ftps, "secure.example", 21, Some("alice")),
        ("ftps-implicit", Protocol::Ftps, "secure.example", 990, Some("alice")),
        ("windows", Protocol::Rdp, "win.example", 3390, Some("bob")),
        ("desktop", Protocol::Vnc, "vnc.example", 5901, None),
        ("ssh-icon", Protocol::Ssh, "icon.example", 22, Some("bob")),
    ] {
        let s = get(name);
        assert_eq!((s.protocol, s.host.as_str(), s.port, s.user.as_deref()), (proto, host, port, user), "{name}");
    }
    assert_eq!(get("ftp-site").ftp.startup_folder.as_deref(), Some("/pub"));
    assert_eq!(get("ftps-explicit").ftp.tls, FtpTls::Explicit);
    assert_eq!(get("ftps-implicit").ftp.tls, FtpTls::Implicit);

    let skipped: Vec<_> = r.skipped.iter().map(|s| (s.name.as_str(), s.reason.as_str())).collect();
    assert_eq!(skipped.len(), 2, "{skipped:?}");
    assert!(skipped[0].0 == "rsh" && skipped[0].1.contains("rsh"));
    assert!(skipped[1].0 == "intranet" && skipped[1].1.contains("browser"));
}

#[test]
fn wsl_bookmarks_import_distribution_and_options() {
    let r = import("wsl.ini");
    let get = |n: &str| &r.sessions.iter().find(|(_, s)| s.name == n).unwrap().1;
    // Real observed layout: a named distribution, and "Default" → the default distribution (None).
    let alpine = get("WSL-Alpine");
    assert_eq!((alpine.protocol, alpine.wsl.distribution.as_deref()), (Protocol::Wsl, Some("Alpine")));
    let default = get("WSL-Default");
    assert_eq!((default.protocol, default.wsl.distribution.as_deref()), (Protocol::Wsl, None));
    // Inferred fields: username, startup folder and startup commands.
    let full = get("wsl-full");
    assert_eq!(full.wsl.distribution.as_deref(), Some("Debian"));
    assert_eq!(full.wsl.user.as_deref(), Some("dev"));
    assert_eq!(full.wsl.start_folder.as_deref(), Some("/srv/app"));
    assert_eq!(full.wsl.startup_commands.as_deref(), Some("source .venv/bin/activate"));
    // A graphical desktop is out of scope: imported as the interactive shell, with a warning.
    let desktop = get("wsl-desktop");
    assert_eq!(desktop.wsl.distribution.as_deref(), Some("Ubuntu"));
    let warn = |n: &str| r.warnings.iter().filter(|w| w.name == n).map(|w| w.reason.as_str()).collect::<Vec<_>>();
    assert!(warn("wsl-desktop").iter().any(|w| w.contains("Full desktop")), "{:?}", warn("wsl-desktop"));
    assert!(warn("WSL-Alpine").is_empty() && warn("wsl-full").is_empty());
}

#[test]
fn telnet_bookmarks_import_without_auto_login() {
    let r = import("telnet_serial.ini");
    let get = |n: &str| &r.sessions.iter().find(|(_, s)| s.name == n).unwrap().1;
    assert_eq!(get("jump-box").protocol, Protocol::Ssh);
    let router = get("router");
    assert_eq!(
        (router.protocol, router.host.as_str(), router.port, router.user.as_deref()),
        (Protocol::Telnet, "router.example", 2323, Some("admin"))
    );
    assert!(!router.telnet.auto_login && !router.telnet.raw);
    assert_eq!(router.telnet.newline, Newline::CrLf);
    let switch = get("switch");
    assert_eq!((switch.port, switch.user.as_deref()), (23, None));
}

#[test]
fn serial_bookmarks_import_port_and_line_settings() {
    let r = import("telnet_serial.ini");
    let get = |n: &str| &r.sessions.iter().find(|(_, s)| s.name == n).unwrap().1;
    let com = get("console-com3");
    assert_eq!((com.protocol, com.serial.port.as_str(), com.serial.line.baud), (Protocol::Serial, "COM3", 9600));
    let usb = &get("console-usb").serial;
    let want = LineConfig {
        baud: 57600,
        data_bits: DataBits::Seven,
        parity: Parity::Even,
        stop_bits: StopBits::Two,
        flow: FlowControl::Hardware,
    };
    assert_eq!((usb.port.as_str(), usb.line), ("/dev/ttyUSB1", want));
    // Unparseable settings keep their defaults, the port is still imported.
    let odd = &get("console-odd").serial;
    assert_eq!((odd.port.as_str(), odd.line), ("/dev/ttyS0", LineConfig::default()));

    let warn = |n: &str| r.warnings.iter().filter(|w| w.name == n).map(|w| w.reason.as_str()).collect::<Vec<_>>();
    let com_warnings = warn("console-com3");
    assert!(com_warnings.len() == 1 && com_warnings[0].contains("Windows port name COM3"), "{com_warnings:?}");
    assert!(warn("console-usb").is_empty());
    let odd_warnings = warn("console-odd");
    for what in ["speed", "data bits", "parity", "stop bits", "flow control"] {
        assert!(odd_warnings.iter().any(|w| w.contains(what)), "{what}: {odd_warnings:?}");
    }
    let skipped: Vec<_> = r.skipped.iter().map(|s| (s.name.as_str(), s.reason.as_str())).collect();
    assert_eq!(skipped, [("no-port", "empty serial port")]);
}

#[test]
fn malformed_lines_are_skipped_with_line_numbers() {
    let r = import("malformed.ini");
    let names: Vec<_> = r.sessions.iter().map(|(_, s)| s.name.as_str()).collect();
    assert_eq!(names, ["good-1", "good-2"]);
    assert_eq!(r.sessions[1].1.port, 22);
    let lines: Vec<_> = r.skipped.iter().map(|s| (s.line, s.name.as_str())).collect();
    assert_eq!(
        lines,
        [(6, "short"), (7, ""), (8, "plain"), (9, "badtype"), (10, "badport"), (11, "nohost"), (12, "unknown")]
    );
    assert!(r.skipped[0].reason.contains("fewer fields"));
}

#[test]
fn password_and_credential_sections_are_never_read() {
    let r = import("secrets.ini");
    assert_eq!(r.sessions.len(), 1);
    let out = format!("{r:?}");
    assert!(!out.contains("FICTIONAL-SECRET"), "{out}");
}

#[test]
fn crlf_and_latin1() {
    let r = import("crlf.ini");
    assert_eq!(r.folders, ["Café"]);
    assert_eq!(r.sessions[0].1.name, "réseau");
    assert_eq!(r.sessions[0].1.host, "net.example");
    assert!(r.skipped.is_empty());
}

#[test]
fn source_file_is_byte_identical_after_import() {
    for name in ["basic.ini", "types.ini", "malformed.ini", "secrets.ini", "crlf.ini", "telnet_serial.ini", "wsl.ini"] {
        let path = fixture(name);
        let before = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        import_file(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before, "{name}");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified, "{name}");
    }
}
