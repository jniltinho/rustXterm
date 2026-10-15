//! Local shells with a real `/bin/sh` (Docker build image): output, input, resize, exit and
//! process containment on close.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use rxt_core::LaunchInfo;
use rxt_platform::{LocalPty, LocalShell, PtyReader};
use rxt_platform_linux::LinuxPty;

fn sh() -> LaunchInfo {
    LaunchInfo {
        shell: "/bin/sh".into(),
        cwd: std::env::temp_dir().display().to_string(),
        ..LinuxPty.prepare().unwrap()
    }
}

/// Reader thread collecting output; `until` waits for a needle.
struct Out(mpsc::Receiver<Vec<u8>>, String);

impl Out {
    fn start(mut reader: Box<dyn PtyReader>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self(rx, String::new())
    }

    fn until(&mut self, needle: &str) -> &str {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.1.contains(needle) {
            let left = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("no {needle:?} in {:?}", self.1));
            if let Ok(b) = self.0.recv_timeout(left) {
                self.1.push_str(&String::from_utf8_lossy(&b));
            }
        }
        &self.1
    }
}

fn start(cols: u16, rows: u16) -> (LocalShell, Out) {
    let LocalShell { reader, writer, control } = LinuxPty.spawn(&sh(), cols, rows, (0, 0)).unwrap();
    (LocalShell { reader: Box::new(NoReader), writer, control }, Out::start(reader))
}

struct NoReader;
impl PtyReader for NoReader {
    fn read(&mut self, _: &mut [u8]) -> rxt_platform::Result<usize> {
        Ok(0)
    }
}

fn alive(pattern: &str) -> bool {
    std::fs::read_dir("/proc").unwrap().flatten().any(|e| {
        std::fs::read(e.path().join("cmdline"))
            .is_ok_and(|c| String::from_utf8_lossy(&c).replace('\0', " ").contains(pattern))
    })
}

#[test]
fn prepare_resolves_shell_folder_user_and_host() {
    let info = LinuxPty.prepare().unwrap();
    assert!(std::path::Path::new(&info.shell).is_file(), "{info:?}");
    assert!(std::path::Path::new(&info.cwd).is_dir(), "{info:?}");
    assert!(!info.user.is_empty() && !info.host.is_empty(), "{info:?}");
}

#[test]
fn term_folder_input_and_resize() {
    let (mut sh, mut out) = start(80, 24);
    sh.writer.write_all(b"echo T=$TERM; pwd; stty size\n").unwrap();
    out.until("T=xterm-256color");
    out.until(&std::env::temp_dir().display().to_string());
    out.until("24 80");
    sh.control.resize(100, 30).unwrap();
    sh.writer.write_all(b"stty size\n").unwrap();
    out.until("30 100");
}

#[test]
fn exit_reports_the_code() {
    let (mut sh, mut out) = start(80, 24);
    sh.writer.write_all(b"exit 3\n").unwrap();
    out.until("exit 3");
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(c) = sh.control.wait_exit(Duration::from_millis(100)) {
            break c;
        }
        assert!(Instant::now() < deadline, "shell did not exit");
    };
    assert_eq!(code, 3);
}

#[test]
fn close_ends_background_and_hup_ignoring_jobs() {
    let (mut sh, mut out) = start(80, 24);
    // Distinct durations so each process is identifiable in /proc.
    sh.writer.write_all(b"sleep 1001 & (trap '' HUP; sleep 1002) & sleep 1003 | cat & echo STARTED\n").unwrap();
    out.until("STARTED");
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive("sleep 1001") && alive("sleep 1002") && alive("sleep 1003"));
    let t = Instant::now();
    sh.control.close();
    assert!(t.elapsed() < Duration::from_millis(500), "close returns at once");
    let deadline = Instant::now() + Duration::from_secs(5);
    while (alive("sleep 1001") || alive("sleep 1002") || alive("sleep 1003")) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive("sleep 1001") && !alive("sleep 1002") && !alive("sleep 1003"));
}

#[test]
fn close_ends_a_blocked_writer() {
    let (sh, mut out) = start(80, 24);
    let LocalShell { writer: mut w, control, .. } = sh;
    w.write_all(b"echo READY; kill -STOP $$\n").unwrap();
    out.until("READY");
    std::thread::sleep(Duration::from_millis(200));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let chunk = vec![b'x'; 64 * 1024];
        let mut r = Ok(());
        for _ in 0..16 {
            r = w.write_all(&chunk);
            if r.is_err() {
                break;
            }
        }
        let _ = tx.send(r.is_err());
    });
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    control.close();
    assert!(t.elapsed() < Duration::from_secs(2));
    // The stopped shell is killed; the blocked write then ends (error or drained).
    assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "writer thread still blocked");
}
