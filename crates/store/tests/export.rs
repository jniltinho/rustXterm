//! TOML export/import round trip.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rxt_core::{
    CursorShape, FlowControl, Folder, FolderId, FtpOptions, FtpTls, JumpHost, LineConfig, Newline, Parity, Protocol,
    RdpOptions, RdpResolution, SerialOptions, Session, SessionId, SshOptions, TelnetOptions, TerminalPrefs, Tunnel,
    TunnelKind, VncOptions,
};
use rxt_store::export::Export;

fn jump(host: &str) -> JumpHost {
    JumpHost { host: host.into(), port: 2200, user: "bob".into(), key_file: Some("~/.ssh/id_bastion".into()) }
}

fn sample() -> Export {
    let folders = vec![
        Folder { id: FolderId(1), name: "Production".into(), parent: None, icon: None },
        Folder {
            id: FolderId(2),
            name: "Databases".into(),
            parent: Some(FolderId(1)),
            icon: Some("folder-green".into()),
        },
    ];
    let ssh = Session {
        id: SessionId(10),
        name: "app".into(),
        folder: Some(FolderId(2)),
        protocol: Protocol::Ssh,
        host: "app.example".into(),
        port: 2022,
        user: Some("alice".into()),
        icon: Some("server".into()),
        ssh: SshOptions {
            key_file: Some("~/.ssh/id_ed25519".into()),
            use_agent: false,
            allow_keyboard_interactive: false,
            allow_password: false,
            jump_hosts: vec![jump("bastion.example"), jump("inner.example")],
            x11_forwarding: true,
            agent_forwarding: true,
            compression: true,
            keepalive_secs: 30,
            remote_command: Some("tmux attach".into()),
            keep_open_after_command: true,
            startup_dir: Some("/srv".into()),
            env: vec![("FOO".into(), "bar".into())],
            auto_reconnect: true,
            follow_terminal_folder: false,
            remote_monitoring: Some(false),
            terminal_type: "vt100".into(),
        },
        ftp: FtpOptions { tls: FtpTls::Explicit, passive: false, ascii: true, startup_folder: Some("/pub".into()) },
        rdp: RdpOptions {
            domain: Some("EXAMPLE".into()),
            resolution: RdpResolution::Fixed { width: 1920, height: 1080 },
            keyboard_layout: 0x0409,
            clipboard: false,
            allow_legacy_tls: true,
            allow_standard_security: true,
        },
        vnc: VncOptions { view_only: true, scale_to_fit: false, clipboard: false, via_ssh: Some(jump("gw.example")) },
        telnet: TelnetOptions { raw: true, newline: Newline::CrNul, auto_login: true, ..TelnetOptions::default() },
        serial: SerialOptions {
            port: "/dev/ttyUSB0".into(),
            line: LineConfig { baud: 9600, parity: Parity::Even, flow: FlowControl::Hardware, ..LineConfig::default() },
            dtr: Some(false),
            ..SerialOptions::default()
        },
        wsl: rxt_core::WslOptions::default(),
        terminal: TerminalPrefs {
            font_family: Some("Hack".into()),
            font_size: Some(12.5),
            cursor_shape: Some(CursorShape::Beam),
            cursor_blink: Some(false),
            scheme: Some("solarized".into()),
            scrollback_lines: Some(5000),
            log_file: Some("/tmp/app.log".into()),
            log_raw: true,
        },
    };
    let rdp = Session::new(Protocol::Rdp, "win.example", Some("bob".into()), None);
    let tunnels = vec![
        Tunnel {
            name: "db".into(),
            kind: TunnelKind::Remote,
            via: SessionId(10),
            bind_addr: "0.0.0.0".into(),
            bind_port: 15432,
            dest_host: "db.internal".into(),
            dest_port: 5432,
            exposure_confirmed: true,
            autostart: true,
        },
        Tunnel { name: "socks".into(), kind: TunnelKind::Dynamic, bind_port: 1080, ..Tunnel::default() },
    ];
    Export::new(folders, vec![ssh, rdp], tunnels)
}

#[test]
fn round_trip_is_identical() {
    let export = sample();
    let text = export.to_toml().unwrap();
    assert!(text.starts_with("format = \"rustxterm-export\"\nversion = 1\n"), "{text}");
    assert_eq!(Export::from_toml(&text).unwrap(), export);
}

#[test]
fn wrong_format_or_version_is_refused() {
    assert!(Export::from_toml("format = \"other\"\nversion = 1\n").is_err());
    assert!(Export::from_toml("format = \"rustxterm-export\"\nversion = 2\n").is_err());
    assert!(Export::from_toml("not toml [").is_err());
    let empty = Export::from_toml("format = \"rustxterm-export\"\nversion = 1\n").unwrap();
    assert!(empty.sessions.is_empty());
}

#[test]
fn credential_keys_in_imported_file_are_dropped() {
    let text = "format = \"rustxterm-export\"\nversion = 1\n[[sessions]]\nname = \"a\"\nhost = \"a.example\"\npassword = \"FICTIONAL\"\n";
    let export = Export::from_toml(text).unwrap();
    assert!(!format!("{export:?}").contains("FICTIONAL"));
    assert!(!export.to_toml().unwrap().contains("FICTIONAL"));
}

#[test]
fn local_terminal_entries_are_skipped() {
    let text = "format = \"rustxterm-export\"\nversion = 1\n[[sessions]]\nname = \"me\"\nprotocol = \"local\"\n[[sessions]]\nname = \"a\"\nhost = \"a.example\"\n";
    let export = Export::from_toml(text).unwrap();
    assert_eq!(export.skipped_local, 1);
    assert_eq!(export.sessions.len(), 1);
    assert_eq!(export.sessions[0].name, "a");
}
