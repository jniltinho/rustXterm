//! Lab tests over `socat` pty pairs (design S7), run in the toolchain container by
//! `cargo xtask lab test`. Side A is opened through the real Linux adapter, side B is
//! driven by the test. ptys ignore line settings on the wire, so settings are verified by
//! reading the termios back (`stty`, `TCGETS2`); the pty driver forces CS8 without parity and
//! has no modem lines (DTR/RTS report Unsupported), both covered by the hardware checklist.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rxt_core::{DataBits, Error, FlowControl, LineConfig, Parity, SerialEnter, SerialOptions, StopBits};
use rxt_platform::{SerialPorts, SerialStream};
use rxt_platform_linux::LinuxSerial;
use tokio_util::sync::CancellationToken;

/// `O_NOCTTY` (Linux): the test process must not adopt the pty as controlling terminal.
const O_NOCTTY: i32 = 0o400;

struct Pair {
    dir: PathBuf,
    socat: Child,
}

impl Pair {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rxt-serial-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socat = Self::spawn(&dir);
        Self { dir, socat }
    }

    fn spawn(dir: &Path) -> Child {
        let link = |s: &str| format!("pty,raw,echo=0,link={}", dir.join(s).display());
        let child = Command::new("socat")
            .args([link("a"), link("b")])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("socat is installed in the toolchain image (Dockerfile)");
        let start = Instant::now();
        while !(dir.join("a").exists() && dir.join("b").exists()) {
            assert!(start.elapsed() < Duration::from_secs(5), "socat links not created");
            std::thread::sleep(Duration::from_millis(20));
        }
        child
    }

    fn a(&self) -> String {
        self.dir.join("a").to_string_lossy().into_owned()
    }

    fn b(&self) -> String {
        self.dir.join("b").to_string_lossy().into_owned()
    }

    /// Simulated unplug: socat exits, side A hangs up.
    fn unplug(&mut self) {
        let _ = self.socat.kill();
        let _ = self.socat.wait();
        let _ = std::fs::remove_file(self.dir.join("a"));
        let _ = std::fs::remove_file(self.dir.join("b"));
    }

    /// Simulated replug: same links, new ptys.
    fn replug(&mut self) {
        self.socat = Self::spawn(&self.dir);
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.unplug();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ports() -> Arc<dyn SerialPorts> {
    Arc::new(LinuxSerial::default())
}

fn opts(port: &str) -> SerialOptions {
    SerialOptions { port: port.into(), ..SerialOptions::default() }
}

/// Side B through the adapter as well (raw pty, 100 ms read timeout).
fn open_b(pair: &Pair) -> Box<dyn SerialStream> {
    ports().open(&pair.b(), &LineConfig::default()).unwrap()
}

fn read_until(s: &mut dyn SerialStream, want: &[u8]) -> Vec<u8> {
    let (mut got, mut buf, start) = (Vec::new(), [0u8; 256], Instant::now());
    while !got.ends_with(want) {
        assert!(start.elapsed() < Duration::from_secs(5), "got {got:?}, want {want:?}");
        let n = s.read(&mut buf).unwrap();
        got.extend_from_slice(&buf[..n]);
    }
    got
}

async fn session_read_until(s: &mut rxt_serial::SerialSession, want: &[u8]) -> Vec<u8> {
    let mut got = Vec::new();
    while !got.ends_with(want) {
        let chunk = tokio::time::timeout(Duration::from_secs(5), s.read()).await.expect("read timed out");
        got.extend(chunk.expect("session ended").unwrap());
    }
    got
}

/// `stty -a` of a tty through an fd opened before the exclusive adapter open.
fn stty(early: &File) -> String {
    let out = Command::new("stty").arg("-a").stdin(early.try_clone().unwrap()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Output baud from `TCGETS2` (`stty` with glibc < 2.42 shows 0 for termios2 `BOTHER` speeds).
fn baud(early: &File) -> u32 {
    let script = "import fcntl,struct;print(struct.unpack_from('I',fcntl.ioctl(0,0x802C542A,bytes(44)),40)[0])";
    let out = Command::new("python3").args(["-c", script]).stdin(early.try_clone().unwrap()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn early_fd(path: &str) -> File {
    OpenOptions::new().read(true).write(true).custom_flags(O_NOCTTY).open(path).unwrap()
}

fn has_flag(stty: &str, flag: &str) -> bool {
    stty.split(|c: char| c.is_whitespace() || c == ';').any(|w| w == flag)
}

#[tokio::test]
#[ignore = "lab"]
async fn serial_echo_both_directions_with_enter_variants_and_local_echo() {
    let pair = Pair::new("echo");
    let mut b = open_b(&pair);
    for (enter, want) in
        [(SerialEnter::Cr, &b"help\r"[..]), (SerialEnter::Lf, b"help\n"), (SerialEnter::CrLf, b"help\r\n")]
    {
        let o = SerialOptions { enter, ..opts(&pair.a()) };
        let mut s = rxt_serial::open(ports(), &o, CancellationToken::new()).unwrap();
        s.writer().write(b"help\r").await.unwrap();
        assert_eq!(read_until(b.as_mut(), want), want);
        b.write_all(b"ok\n").unwrap();
        assert_eq!(session_read_until(&mut s, b"ok\n").await, b"ok\n");
        // The reader thread closes the port within one 100 ms read timeout after drop.
        drop(s);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let o = SerialOptions { lf_to_crlf: true, local_echo: true, ..opts(&pair.a()) };
    let mut s = rxt_serial::open(ports(), &o, CancellationToken::new()).unwrap();
    s.writer().write(b"id\r").await.unwrap();
    assert_eq!(session_read_until(&mut s, b"id\r\n").await, b"id\r\n");
    assert_eq!(read_until(b.as_mut(), b"id\r"), b"id\r");
    b.write_all(b"uid=1\n").unwrap();
    assert_eq!(session_read_until(&mut s, b"uid=1\r\n").await, b"uid=1\r\n");
}

#[test]
#[ignore = "lab"]
fn serial_line_settings_8n1_and_7e1_read_back_with_stty() {
    let pair = Pair::new("stty");
    let early = early_fd(&pair.a());
    let s = ports().open(&pair.a(), &LineConfig::default()).unwrap();
    let t = stty(&early);
    assert_eq!(baud(&early), 115_200);
    for f in ["cs8", "-parenb", "-cstopb", "-crtscts", "-ixon", "-inpck"] {
        assert!(has_flag(&t, f), "{f} in {t}");
    }
    drop(s);
    let cfg = LineConfig {
        baud: 9600,
        data_bits: DataBits::Seven,
        parity: Parity::Even,
        stop_bits: StopBits::One,
        flow: FlowControl::Hardware,
    };
    let mut s = ports().open(&pair.a(), &cfg).unwrap();
    let t = stty(&early);
    assert_eq!(baud(&early), 9600);
    // The pty driver forces CS8 and clears PARENB (pty_set_termios); data bits and parity on
    // the wire are covered by the manual hardware checklist. Input parity checking shows 7E1 was applied.
    for f in ["inpck", "-cstopb", "crtscts"] {
        assert!(has_flag(&t, f), "{f} in {t}");
    }
    s.reconfigure(&LineConfig { baud: 57_600, stop_bits: StopBits::Two, flow: FlowControl::Software, ..cfg }).unwrap();
    let t = stty(&early);
    assert_eq!(baud(&early), 57_600);
    for f in ["cstopb", "-crtscts", "ixon", "ixoff"] {
        assert!(has_flag(&t, f), "{f} in {t}");
    }
}

#[tokio::test]
#[ignore = "lab"]
async fn serial_break_succeeds_and_pty_modem_lines_report_unsupported() {
    let pair = Pair::new("break");
    let s = rxt_serial::open(ports(), &opts(&pair.a()), CancellationToken::new()).unwrap();
    let w = s.writer();
    w.send_break(50).await.unwrap();
    let e = w.set_lines(Some(true), Some(false)).await.unwrap_err();
    assert!(matches!(&e, Error::Unsupported(m) if m.contains("DTR")), "{e}");
    w.write(b"still open\r").await.unwrap();
}

#[test]
#[ignore = "lab"]
fn serial_busy_port_reported() {
    let pair = Pair::new("busy");
    let _first = ports().open(&pair.a(), &LineConfig::default()).unwrap();
    let e = ports().open(&pair.a(), &LineConfig::default()).err().unwrap();
    assert!(matches!(&e, Error::Busy(p) if *p == pair.a()), "{e}");
    assert_eq!(e.to_string(), format!("port in use: {}", pair.a()));
}

#[test]
#[ignore = "lab"]
fn serial_permission_denied_names_group() {
    let uid = Command::new("id").arg("-u").output().unwrap();
    assert_ne!(String::from_utf8_lossy(&uid.stdout).trim(), "0", "run as non-root (scripts/cargo-docker.sh does)");
    let pair = Pair::new("perm");
    let target = std::fs::canonicalize(pair.a()).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = ports().open(&pair.a(), &LineConfig::default()).err().unwrap();
    // devpts slaves belong to group `tty` in the container.
    let msg = e.to_string();
    assert!(matches!(e, Error::PermissionDenied(_)), "{msg}");
    assert!(
        msg.contains(&pair.a()) && msg.contains("group `tty`") && msg.contains("sudo usermod -aG tty $USER"),
        "{msg}"
    );
}

#[tokio::test]
#[ignore = "lab"]
async fn serial_unplug_reports_gone_then_replug_reconnects() {
    let mut pair = Pair::new("replug");
    let o = opts(&pair.a());
    let mut s = rxt_serial::open(ports(), &o, CancellationToken::new()).unwrap();
    let last = s.port_info().cloned();
    pair.unplug();
    let r = tokio::time::timeout(Duration::from_secs(5), s.read()).await.unwrap();
    assert!(matches!(&r, Some(Err(Error::Gone(p))) if *p == pair.a()), "{r:?}");
    assert_eq!(Error::Gone(pair.a()).to_string(), format!("device removed: {}", pair.a()));

    let cancel = CancellationToken::new();
    let waiting = tokio::spawn({
        let (o, cancel) = (o.clone(), cancel.clone());
        async move { rxt_serial::reopen(ports(), &o, last.as_ref(), cancel).await }
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!waiting.is_finished(), "reopened before the device returned");
    pair.replug();
    let start = Instant::now();
    let mut s = tokio::time::timeout(Duration::from_secs(3), waiting).await.unwrap().unwrap().unwrap();
    assert!(start.elapsed() < Duration::from_secs(2), "reconnect took {:?}", start.elapsed());
    let mut b = open_b(&pair);
    b.write_all(b"back\n").unwrap();
    assert_eq!(session_read_until(&mut s, b"back\n").await, b"back\n");
}
