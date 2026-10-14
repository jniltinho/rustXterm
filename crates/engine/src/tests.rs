//! Engine tests with an in-memory transport. Behavior is chosen by the session host:
//! `flood` (endless output), `exit` (prints then exits 0), `drop` (closes without exit status),
//! `prompt` (asks a password, accepts "pw"), `panic` (connect panics); anything else is quiet.
//! Serial sessions use their port as the key: `/dev/ttyUSB0` reports the device as removed on
//! its first read; serial links record break/DTR/RTS calls.
//! Desktops: `desk` (4x4 desktop, one update, clipboard), `huge` (oversized desktop); FTP tabs get [`FakeFs`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use rxt_core::fileops::Progress;
use rxt_core::{
    BoxFut, CertTrust, Command, ConnState, DesktopEvent, DesktopInput, Error, Event, FileEntry, FileKind, FileOps,
    PromptAnswer, PromptKind, Protocol, Rect, Result, Secret, Session, TabId, Tunnel,
};
use rxt_platform::SerialPorts;
use rxt_ssh::{Prompter, ShellMsg};
use tokio_util::sync::CancellationToken;

use crate::transport::{
    CertStore, DesktopPair, DesktopRead, DesktopSend, LineControl, Link, OnTunnelState, ShellPair, ShellRead,
    ShellWrite, Transport,
};
use crate::{Engine, EngineConfig, EngineHandle, Records};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct State {
    reads: AtomicUsize,
    writes: Mutex<Vec<(String, Instant, Vec<u8>)>>,
    answers: Mutex<Vec<PromptAnswer>>,
    desk_inputs: Mutex<Vec<DesktopInput>>,
    desk_sizes: Mutex<Vec<(u32, u32)>>,
    fs_calls: Mutex<Vec<String>>,
    /// Connect keys (host, or serial port) in order.
    connects: Mutex<Vec<String>>,
    /// Serial line-control calls.
    lines: Mutex<Vec<String>>,
    /// When set, serial connects fail with `NotFound` for ports it does not list.
    serial: Mutex<Option<rxt_platform::serial::FakeSerial>>,
    /// Folder of the fake login shell (PID 102) for `follow` hosts; empty: `/proc` unreadable.
    cwd: Mutex<String>,
    /// Exec commands run on links.
    execs: Mutex<Vec<String>>,
    /// Monitoring channels opened (`mon*` hosts) and closed by the collector.
    mon_opens: AtomicUsize,
    mon_closed: AtomicUsize,
}

impl State {
    fn writes_to(&self, host: &str) -> Vec<Vec<u8>> {
        self.writes.lock().unwrap().iter().filter(|w| w.0 == host).map(|w| w.2.clone()).collect()
    }
}

struct Fake(Arc<State>);

impl Transport for Fake {
    fn connect(
        &self,
        session: Session,
        prompter: Arc<dyn Prompter>,
        _cancel: CancellationToken,
    ) -> BoxFut<'static, Result<Arc<dyn Link>>> {
        let state = self.0.clone();
        Box::pin(async move {
            let key =
                if session.protocol == Protocol::Serial { session.serial.port.clone() } else { session.host.clone() };
            state.connects.lock().unwrap().push(key.clone());
            let plugged =
                state.serial.lock().unwrap().as_ref().map(|f| f.list().unwrap().iter().any(|p| p.path == key));
            if session.protocol == Protocol::Serial && plugged == Some(false) {
                return Err(Error::NotFound(key));
            }
            match session.host.as_str() {
                "panic" => panic!("fake transport panic"),
                "prompt" => {
                    for retry in [false, true] {
                        let kind = PromptKind::Password { user: "alice".into(), host: session.host.clone(), retry };
                        let answer = prompter.prompt(kind).await;
                        state.answers.lock().unwrap().push(answer.clone());
                        if answer == PromptAnswer::Secret(Secret("pw".into())) {
                            break;
                        }
                        if retry || answer == PromptAnswer::Cancel {
                            return Err(Error::Auth("bad password".into()));
                        }
                    }
                }
                _ => {}
            }
            Ok(Arc::new(FakeLink { state, host: key }) as Arc<dyn Link>)
        })
    }

    fn connect_files(
        &self,
        _: Session,
        _: Arc<dyn Prompter>,
        _: CancellationToken,
    ) -> BoxFut<'static, Result<Arc<dyn FileOps>>> {
        let fs = Arc::new(FakeFs(self.0.clone()));
        Box::pin(async move { Ok(fs as Arc<dyn FileOps>) })
    }

    fn connect_desktop(
        &self,
        session: Session,
        _: Arc<dyn Prompter>,
        _: CancellationToken,
        size: (u32, u32),
    ) -> BoxFut<'static, Result<DesktopPair>> {
        let state = self.0.clone();
        state.desk_sizes.lock().unwrap().push(size);
        let mut script = match session.host.as_str() {
            "huge" => vec![DesktopEvent::Resized { width: 9000, height: 10 }],
            _ => vec![
                DesktopEvent::Resized { width: 4, height: 4 },
                DesktopEvent::Update { rect: Rect { x: 1, y: 1, width: 1, height: 1 }, rgba: vec![7; 4] },
                DesktopEvent::Clipboard("copied".into()),
            ],
        };
        script.reverse();
        let send: DesktopSend = Arc::new(move |i| {
            state.desk_inputs.lock().unwrap().push(i);
            Ok(())
        });
        Box::pin(async move { Ok((Box::new(FakeDesk(script)) as Box<dyn DesktopRead>, send)) })
    }
}

struct FakeDesk(Vec<DesktopEvent>);

impl DesktopRead for FakeDesk {
    fn next(&mut self) -> BoxFut<'_, Option<Result<DesktopEvent>>> {
        Box::pin(async move {
            match self.0.pop() {
                Some(ev) => Some(Ok(ev)),
                None => std::future::pending().await,
            }
        })
    }
}

/// FTP stand-in: home is `/home/alice`; every call is recorded.
struct FakeFs(Arc<State>);

impl FakeFs {
    fn call(&self, c: String) -> BoxFut<'_, Result<()>> {
        self.0.fs_calls.lock().unwrap().push(c);
        Box::pin(async { Ok(()) })
    }
}

impl FileOps for FakeFs {
    fn home(&self) -> BoxFut<'_, Result<String>> {
        Box::pin(async { Ok("/home/alice".to_owned()) })
    }
    fn list<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<Vec<FileEntry>>> {
        self.0.fs_calls.lock().unwrap().push(format!("list {path}"));
        let entry = FileEntry {
            name: "a.txt".into(),
            kind: FileKind::File,
            size: 1,
            modified: None,
            mode: None,
            owner: None,
            group: None,
        };
        Box::pin(async move { Ok(vec![entry]) })
    }
    fn stat<'a>(&'a self, _: &'a str) -> BoxFut<'a, Result<Option<FileEntry>>> {
        Box::pin(async { Ok(None) })
    }
    fn mkdir<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
        self.call(format!("mkdir {path}"))
    }
    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BoxFut<'a, Result<()>> {
        self.call(format!("rename {from} {to}"))
    }
    fn delete<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<()>> {
        self.call(format!("delete {path}"))
    }
    fn chmod<'a>(&'a self, path: &'a str, mode: u32) -> BoxFut<'a, Result<()>> {
        self.call(format!("chmod {path} {mode:o}"))
    }
    fn get<'a>(&'a self, _: &'a str, _: &'a std::path::Path, _: Progress<'a>) -> BoxFut<'a, Result<u64>> {
        Box::pin(async { Err(Error::Unsupported("fake get".into())) })
    }
    fn put<'a>(&'a self, _: &'a std::path::Path, _: &'a str, _: Progress<'a>) -> BoxFut<'a, Result<u64>> {
        Box::pin(async { Err(Error::Unsupported("fake put".into())) })
    }
    fn real_path<'a>(&'a self, path: &'a str) -> BoxFut<'a, Result<String>> {
        self.0.fs_calls.lock().unwrap().push(format!("realpath {path}"));
        let cwd = self.0.cwd.lock().unwrap().clone();
        Box::pin(async move {
            match path {
                "/proc/102/cwd" if !cwd.is_empty() => Ok(cwd),
                _ => Err(Error::NotFound(path.to_owned())),
            }
        })
    }
}

struct FakeLink {
    state: Arc<State>,
    host: String,
}

impl Link for FakeLink {
    fn shell<'a>(&'a self, _: &'a Session, _: u16, _: u16) -> BoxFut<'a, Result<ShellPair>> {
        let script = match self.host.as_str() {
            "exit" => vec![ShellMsg::Data(b"bye".to_vec()), ShellMsg::Eof, ShellMsg::ExitStatus(0)],
            "banner" => vec![ShellMsg::Data(b"$ ".to_vec())],
            _ => Vec::new(),
        };
        let reader = FakeReader { state: self.state.clone(), host: self.host.clone(), script, end: None };
        let writer = FakeWriter { state: self.state.clone(), host: self.host.clone() };
        Box::pin(async move { Ok((Box::new(reader) as Box<dyn ShellRead>, Box::new(writer) as Box<dyn ShellWrite>)) })
    }

    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>> {
        let fs = Arc::new(FakeFs(self.state.clone()));
        let sftp = matches!(self.host.as_str(), "follow" | "busybox");
        Box::pin(
            async move { if sftp { Ok(fs as Arc<dyn FileOps>) } else { Err(Error::Unsupported("fake sftp".into())) } },
        )
    }

    /// `follow`: a `ps -eo` table whose login shell is PID 102; `busybox`: `ps -eo` unsupported.
    fn exec<'a>(&'a self, command: &'a str) -> BoxFut<'a, Result<String>> {
        self.state.execs.lock().unwrap().push(command.to_owned());
        let out = match self.host.as_str() {
            "follow" => Ok("200\nTT PID PPID ELAPSED COMMAND\n? 101 1 05:00 sshd: alice@pts/0\n\
                            pts/0 102 101 04:58 -bash\n? 200 101 00:00 ps -eo tty,pid,ppid,etime,args\n"
                .to_owned()),
            "busybox" => Ok("200\n".to_owned()),
            _ => Err(Error::Unsupported("fake exec".into())),
        };
        Box::pin(async move { out })
    }

    fn start_tunnel<'a>(&'a self, _: &'a Tunnel, _: CancellationToken, _: OnTunnelState) -> BoxFut<'a, Result<()>> {
        Box::pin(async { Err(Error::Unsupported("fake tunnel".into())) })
    }

    /// `mon*` hosts: a monitoring peer answering every request with a memory-only frame.
    fn exec_stream<'a>(&'a self, _: &'a str) -> BoxFut<'a, Result<crate::transport::ExecStream>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let state = self.state.clone();
        let mon = self.host.starts_with("mon");
        Box::pin(async move {
            if !mon {
                return Err(Error::Unsupported("fake exec".into()));
            }
            state.mon_opens.fetch_add(1, Ordering::SeqCst);
            let (ours, theirs) = tokio::io::duplex(1 << 16);
            tokio::spawn(async move {
                let (r, mut w) = tokio::io::split(theirs);
                let mut r = tokio::io::BufReader::new(r);
                let _ = w.write_all(b"rxt-mon 1 Linux fake-host\n").await;
                let mut line = String::new();
                while r.read_line(&mut line).await.is_ok_and(|n| n > 0) {
                    line.clear();
                    let _ = w.write_all(b"@s\nMemTotal: 1000 kB\nMemAvailable: 250 kB\n@e\n").await;
                }
                state.mon_closed.fetch_add(1, Ordering::SeqCst);
            });
            let (r, w) = tokio::io::split(ours);
            Ok((Box::new(r) as Box<dyn tokio::io::AsyncRead + Send + Unpin>, Box::new(w) as _))
        })
    }

    fn ssh_info(&self) -> Option<rxt_core::SshInfo> {
        (self.host == "banner").then(|| rxt_core::SshInfo { server_id: "SSH-2.0-Fake".into(), ..Default::default() })
    }

    fn take_auth_banner(&self) -> String {
        if self.host == "banner" { "Fictional lab banner\r\n".into() } else { String::new() }
    }

    fn line_control(&self) -> Option<Arc<dyn LineControl>> {
        self.host.starts_with("/dev/").then(|| Arc::new(FakeLines(self.state.clone())) as Arc<dyn LineControl>)
    }
}

struct FakeLines(Arc<State>);

impl LineControl for FakeLines {
    fn send_break(&self, ms: u64) -> BoxFut<'_, Result<()>> {
        self.0.lines.lock().unwrap().push(format!("break {ms}"));
        Box::pin(async { Ok(()) })
    }

    fn set_lines(&self, dtr: Option<bool>, rts: Option<bool>) -> BoxFut<'_, Result<()>> {
        self.0.lines.lock().unwrap().push(format!("dtr {dtr:?} rts {rts:?}"));
        Box::pin(async { Ok(()) })
    }
}

struct FakeReader {
    state: Arc<State>,
    host: String,
    script: Vec<ShellMsg>,
    end: Option<Error>,
}

impl ShellRead for FakeReader {
    fn end_reason(&mut self) -> Option<Error> {
        self.end.take()
    }

    fn read(&mut self) -> BoxFut<'_, Option<ShellMsg>> {
        Box::pin(async move {
            match self.host.as_str() {
                "/dev/ttyUSB0" => {
                    self.end = Some(Error::Gone("/dev/ttyUSB0".into()));
                    None
                }
                "flood" => {
                    self.state.reads.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    Some(ShellMsg::Data(vec![b'x'; 256]))
                }
                "exit" | "drop" => (!self.script.is_empty()).then(|| self.script.remove(0)),
                "banner" if !self.script.is_empty() => Some(self.script.remove(0)),
                _ => std::future::pending().await,
            }
        })
    }
}

struct FakeWriter {
    state: Arc<State>,
    host: String,
}

impl ShellWrite for FakeWriter {
    fn write<'a>(&'a self, bytes: &'a [u8]) -> BoxFut<'a, Result<()>> {
        self.state.writes.lock().unwrap().push((self.host.clone(), Instant::now(), bytes.to_vec()));
        Box::pin(async { Ok(()) })
    }

    fn resize(&self, _: u16, _: u16) -> BoxFut<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

fn start(cap: usize) -> (EngineHandle, Arc<State>, Arc<AtomicUsize>) {
    start_config(EngineConfig { output_buffer_bytes: cap, ..EngineConfig::default() })
}

fn start_config(config: EngineConfig) -> (EngineHandle, Arc<State>, Arc<AtomicUsize>) {
    let state = Arc::new(State::default());
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let waker = Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    });
    let h = Engine::start_with(config, waker, Arc::new(Fake(state.clone()))).unwrap();
    (h, state, wakes)
}

fn wait_for<T>(h: &EngineHandle, mut f: impl FnMut(&Event) -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        match h.try_event() {
            Some(ev) => {
                if let Some(v) = f(&ev) {
                    return v;
                }
            }
            None => sleep(Duration::from_millis(1)),
        }
    }
    panic!("timed out waiting for event");
}

fn wait_until(mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for condition");
        sleep(Duration::from_millis(1));
    }
}

fn open_session(h: &EngineHandle, session: Session) -> TabId {
    let host = session.host.clone();
    h.send(Command::Open { session: Box::new(session) }).unwrap();
    wait_for(h, |ev| match ev {
        Event::TabOpened { tab, session } if session.host == host => Some(*tab),
        _ => None,
    })
}

fn open(h: &EngineHandle, host: &str) -> TabId {
    open_session(h, Session::new(Protocol::Ssh, host, None, None))
}

fn wait_state(h: &EngineHandle, tab: TabId, mut f: impl FnMut(&ConnState) -> bool) -> ConnState {
    wait_for(h, |ev| match ev {
        Event::State { tab: t, state } if *t == tab && f(state) => Some(state.clone()),
        _ => None,
    })
}

fn connect(h: &EngineHandle, host: &str) -> TabId {
    let tab = open(h, host);
    wait_state(h, tab, |s| *s == ConnState::Connected);
    tab
}

#[test]
fn ssh_info_precedes_connected_and_auth_banner_precedes_shell_output() {
    let (h, _state, _) = start(1 << 20);
    let tab = open(&h, "banner");
    let mut seen = Vec::new();
    wait_for(&h, |ev| match ev {
        Event::SshInfo { tab: t, info } if *t == tab => {
            seen.push(info.server_id.clone());
            None
        }
        Event::State { tab: t, state: ConnState::Connected } if *t == tab => Some(()),
        _ => None,
    });
    assert_eq!(seen, ["SSH-2.0-Fake"]);
    let mut out = Vec::new();
    wait_until(|| {
        h.drain_output(tab, usize::MAX, &mut out);
        out.ends_with(b"$ ")
    });
    assert_eq!(out, b"Fictional lab banner\r\n$ ");
}

#[test]
fn backpressure_stops_reader_until_drained() {
    let (h, state, wakes) = start(1024);
    let tab = connect(&h, "flood");
    wait_until(|| h.pending_output().contains(&tab));
    sleep(Duration::from_millis(100));
    let stalled = state.reads.load(Ordering::SeqCst);
    sleep(Duration::from_millis(100));
    assert_eq!(state.reads.load(Ordering::SeqCst), stalled, "reader must stop while the buffer is full");
    assert!(stalled <= 1024 / 256 + 1, "reads: {stalled}");

    let mut out = Vec::new();
    assert_eq!(h.drain_output(tab, 512, &mut out), 512);
    wait_until(|| state.reads.load(Ordering::SeqCst) > stalled);
    out.clear();
    wait_until(|| {
        h.drain_output(tab, usize::MAX, &mut out);
        out.len() > 10 * 1024
    });
    assert!(out.iter().all(|&b| b == b'x'));
    assert!(wakes.load(Ordering::SeqCst) > 0);
    h.shutdown(WAIT);
}

#[test]
fn sibling_input_latency_under_flood() {
    let (h, state, _) = start(64 * 1024);
    let flood = connect(&h, "flood");
    let quiet = connect(&h, "quiet");
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drainer = {
        let (h, stop) = (h.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut out = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                out.clear();
                h.drain_output(flood, 1 << 20, &mut out);
            }
        })
    };
    let reads_before = state.reads.load(Ordering::SeqCst);
    let mut worst = Duration::ZERO;
    for i in 0..50 {
        let sent = Instant::now();
        h.send(Command::Input { tab: quiet, bytes: vec![b'a'] }).unwrap();
        wait_until(|| state.writes_to("quiet").len() > i);
        let at = state.writes.lock().unwrap().iter().rev().find(|w| w.0 == "quiet").map(|w| w.1).unwrap();
        worst = worst.max(at - sent);
        sleep(Duration::from_millis(2));
    }
    stop.store(true, Ordering::SeqCst);
    drainer.join().unwrap();
    assert!(state.reads.load(Ordering::SeqCst) > reads_before + 100, "flood must keep running");
    assert!(worst < Duration::from_millis(100), "worst input latency {worst:?}");
    h.shutdown(WAIT);
}

#[test]
fn multiexec_fans_out_only_when_enabled() {
    let (h, state, _) = start(1024);
    let a = connect(&h, "a");
    let b = connect(&h, "b");
    let c = connect(&h, "c");
    h.send(Command::SelectMultiExec { tab: a, selected: true }).unwrap();
    h.send(Command::SelectMultiExec { tab: b, selected: true }).unwrap();
    h.send(Command::SetMultiExec { enabled: true }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::MultiExec { enabled: true, selected } if *selected == [a, b]).then_some(()));

    h.send(Command::Input { tab: a, bytes: b"uptime\r".to_vec() }).unwrap();
    h.send(Command::Input { tab: c, bytes: b"c\r".to_vec() }).unwrap();
    wait_until(|| state.writes_to("b").len() == 1 && state.writes_to("c").len() == 1);
    assert_eq!(state.writes_to("a"), [b"uptime\r".to_vec()]);
    assert_eq!(state.writes_to("b"), [b"uptime\r".to_vec()]);
    assert_eq!(state.writes_to("c"), [b"c\r".to_vec()]);

    h.send(Command::SetMultiExec { enabled: false }).unwrap();
    h.send(Command::Input { tab: a, bytes: b"x".to_vec() }).unwrap();
    wait_until(|| state.writes_to("a").len() == 2);
    sleep(Duration::from_millis(50));
    assert_eq!(state.writes_to("b").len(), 1, "no broadcast after MultiExec is off");
    h.shutdown(WAIT);
}

#[test]
fn shutdown_while_readers_blocked() {
    let (h, _, _) = start(1024);
    let flood = connect(&h, "flood");
    connect(&h, "quiet");
    open(&h, "prompt");
    wait_until(|| h.pending_output().contains(&flood));
    sleep(Duration::from_millis(50));
    let t = Instant::now();
    h.shutdown(Duration::from_secs(2));
    assert!(t.elapsed() < Duration::from_secs(2), "shutdown took {:?}", t.elapsed());
    assert!(h.send(Command::Close { tab: flood }).is_err());
    assert!(h.send(Command::Input { tab: flood, bytes: vec![1] }).is_err());
}

#[test]
fn prompt_round_trip_and_cancel_on_close() {
    let (h, state, _) = start(1024);
    let tab = open(&h, "prompt");
    let id = wait_for(&h, |ev| match ev {
        Event::Prompt { id, tab: Some(t), kind: PromptKind::Password { .. } } if *t == tab => Some(*id),
        _ => None,
    });
    h.send(Command::Answer { prompt: id, answer: PromptAnswer::Secret(Secret("pw".into())) }).unwrap();
    wait_state(&h, tab, |s| *s == ConnState::Connected);

    let tab = open(&h, "prompt");
    wait_for(&h, |ev| matches!(ev, Event::Prompt { tab: Some(t), .. } if *t == tab).then_some(()));
    h.send(Command::Close { tab }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::Closed { tab: t } if *t == tab).then_some(()));
    // Closing the tab cancels the pending prompt (resolved as Cancel, or dropped with the connect).
    wait_until(|| h.shared.prompts.lock().is_empty());
    let answers = state.answers.lock().unwrap().clone();
    assert!(answers.len() == 1 || answers[1] == PromptAnswer::Cancel, "{answers:?}");
    h.shutdown(WAIT);
}

#[test]
fn panicking_session_becomes_error_and_engine_survives() {
    let (h, state, _) = start(1024);
    let bad = open(&h, "panic");
    let st = wait_state(&h, bad, |s| matches!(s, ConnState::Error(_)));
    assert!(matches!(st, ConnState::Error(m) if m.contains("panicked")));
    let ok = connect(&h, "quiet");
    h.send(Command::Input { tab: ok, bytes: b"ls\r".to_vec() }).unwrap();
    wait_until(|| state.writes_to("quiet").len() == 1);
    h.shutdown(WAIT);
}

#[test]
fn exit_keeps_tab_drop_schedules_reconnect_command_closes() {
    let (h, _, _) = start(1024);
    let tab = connect(&h, "exit");
    let st = wait_state(&h, tab, |s| matches!(s, ConnState::Disconnected { .. }));
    assert_eq!(st, ConnState::Disconnected { reason: "exited with status 0".into(), reconnect_at: None });
    let mut out = Vec::new();
    h.drain_output(tab, 100, &mut out);
    assert_eq!(out, b"bye");

    let mut s = Session::new(Protocol::Ssh, "drop", None, None);
    s.ssh.auto_reconnect = true;
    let tab = open_session(&h, s);
    let st = wait_state(&h, tab, |s| matches!(s, ConnState::Disconnected { .. }));
    assert!(matches!(st, ConnState::Disconnected { reconnect_at: Some(_), .. }));
    h.send(Command::Reconnect { tab }).unwrap();
    wait_state(&h, tab, |s| *s == ConnState::Connected);

    let mut s = Session::new(Protocol::Ssh, "exit", None, None);
    s.ssh.remote_command = Some("uname -a".into());
    s.name = "cmd".into();
    h.send(Command::Open { session: Box::new(s) }).unwrap();
    let tab = wait_for(&h, |ev| match ev {
        Event::TabOpened { tab, session } if session.name == "cmd" => Some(*tab),
        _ => None,
    });
    wait_for(&h, |ev| matches!(ev, Event::Closed { tab: t } if *t == tab).then_some(()));
    h.shutdown(WAIT);
}

fn connect_proto(h: &EngineHandle, protocol: Protocol, host: &str) -> TabId {
    let tab = open_session(h, Session::new(protocol, host, None, None));
    wait_state(h, tab, |s| *s == ConnState::Connected);
    tab
}

#[test]
fn desktop_events_reach_framebuffer_and_input_is_routed() {
    let (h, state, _) = start(1024);
    let tab = connect_proto(&h, Protocol::Vnc, "desk");
    wait_until(|| h.with_framebuffer(tab, |fb| fb.width == 4 && fb.dirty.len() == 2).unwrap());
    let dirty = h.with_framebuffer(tab, |fb| {
        assert_eq!(&fb.pixels[(4 + 1) * 4..(4 + 2) * 4], &[7; 4]);
        fb.take_dirty()
    });
    assert_eq!(dirty.unwrap()[1], Rect { x: 1, y: 1, width: 1, height: 1 });
    assert!(h.with_framebuffer(tab, |fb| fb.dirty.is_empty()).unwrap());
    wait_until(|| h.shared.tabs.lock()[&tab].clipboard.is_some());
    assert_eq!(h.take_clipboard(tab).as_deref(), Some("copied"));
    assert_eq!(h.take_clipboard(tab), None);
    assert!(h.with_framebuffer(TabId(9999), |_| ()).is_none());

    let key = DesktopInput::Key { keysym: 0x61, down: true };
    h.send(Command::DesktopInput { tab, input: key.clone() }).unwrap();
    h.send(Command::SetVisible { tab, visible: false }).unwrap();
    assert_eq!(*state.desk_inputs.lock().unwrap(), [key, DesktopInput::Visible(false)]);
    assert_eq!(*state.desk_sizes.lock().unwrap(), [(1280, 800)]);

    // The last requested size is the initial size of the next connection; hidden stays hidden.
    h.send(Command::DesktopInput { tab, input: DesktopInput::Resize { width: 800, height: 600 } }).unwrap();
    h.send(Command::Reconnect { tab }).unwrap();
    wait_until(|| state.desk_sizes.lock().unwrap().len() == 2);
    assert_eq!(state.desk_sizes.lock().unwrap()[1], (800, 600));
    wait_until(|| state.desk_inputs.lock().unwrap().len() == 4);
    assert_eq!(state.desk_inputs.lock().unwrap()[3], DesktopInput::Visible(false));
    h.shutdown(WAIT);
}

#[test]
fn oversized_desktop_ends_tab_with_error() {
    let (h, _, _) = start(1024);
    let tab = open_session(&h, Session::new(Protocol::Rdp, "huge", None, None));
    let st = wait_state(&h, tab, |s| matches!(s, ConnState::Error(_)));
    assert!(matches!(st, ConnState::Error(m) if m.contains("9000x10")));
    h.shutdown(WAIT);
}

#[test]
fn ftp_tab_routes_file_commands() {
    let (h, state, _) = start(1024);
    let tab = connect_proto(&h, Protocol::Ftps, "ftp");
    h.send(Command::List { tab, path: "~".into() }).unwrap();
    let path = wait_for(&h, |ev| match ev {
        Event::Listing { tab: t, path, entries } if *t == tab && entries.len() == 1 => Some(path.clone()),
        _ => None,
    });
    assert_eq!(path, "/home/alice");
    h.send(Command::Mkdir { tab, path: "/srv/new".into() }).unwrap();
    let path = wait_for(&h, |ev| match ev {
        Event::Listing { path, .. } => Some(path.clone()),
        _ => None,
    });
    assert_eq!(path, "/srv");
    assert_eq!(*state.fs_calls.lock().unwrap(), ["list /home/alice", "mkdir /srv/new", "list /srv"]);
    h.shutdown(WAIT);
}

#[test]
fn cert_store_keeps_latest_fingerprint_per_host_port() {
    let store = CertStore::default();
    assert_eq!(store.get("ftp.lab", 990), None);
    store.save("FTP.lab", 990, "SHA256:a");
    store.save("ftp.lab", 990, "SHA256:b");
    assert_eq!(store.get("ftp.LAB", 990).as_deref(), Some("SHA256:b"));
    assert_eq!(store.get("ftp.lab", 21), None);
}

/// Records double: one saved password per session id.
#[derive(Default)]
struct MemRecords(Mutex<std::collections::HashMap<u64, String>>);

impl Records for MemRecords {
    fn credential(&self, session: &Session, kind: &PromptKind) -> Option<Secret> {
        assert!(matches!(kind, PromptKind::Password { retry: false, .. }));
        self.0.lock().unwrap().get(&session.id.0).cloned().map(Secret)
    }
    fn save_credential(&self, session: &Session, _: &PromptKind, secret: &Secret) {
        self.0.lock().unwrap().insert(session.id.0, secret.0.clone());
    }
    fn forget_credential(&self, session: &Session, _: &PromptKind) {
        self.0.lock().unwrap().remove(&session.id.0);
    }
    fn host_keys(&self, _: &str, _: u16) -> Vec<String> {
        Vec::new()
    }
    fn save_host_key(&self, _: &str, _: u16, _: &str) {}
    fn certificate(&self, _: &str, _: u16) -> Option<String> {
        None
    }
    fn save_certificate(&self, _: &str, _: u16, _: &str) {}
}

#[test]
fn saved_password_skips_prompt_and_rejected_one_is_forgotten() {
    let records = Arc::new(MemRecords::default());
    let (h, state, _) = start_config(EngineConfig { records: Some(records.clone()), ..EngineConfig::default() });
    let session = Session { id: rxt_core::SessionId(7), ..Session::new(Protocol::Ssh, "prompt", None, None) };
    let password_prompt = |h: &EngineHandle, tab: TabId| {
        wait_for(h, |ev| match ev {
            Event::Prompt { id, tab: Some(t), kind: PromptKind::Password { retry, .. } } if *t == tab => {
                Some((*id, *retry))
            }
            _ => None,
        })
    };

    // Typed without "save password": nothing stored.
    let tab = open_session(&h, session.clone());
    let (id, _) = password_prompt(&h, tab);
    h.send(Command::Answer { prompt: id, answer: PromptAnswer::Secret(Secret("pw".into())) }).unwrap();
    wait_state(&h, tab, |s| *s == ConnState::Connected);
    assert!(records.0.lock().unwrap().is_empty());

    // Saved only after a successful login: the wrong first answer is not kept.
    let tab = open_session(&h, session.clone());
    let (id, _) = password_prompt(&h, tab);
    h.save_secret(id);
    h.send(Command::Answer { prompt: id, answer: PromptAnswer::Secret(Secret("nope".into())) }).unwrap();
    let (id, retry) = password_prompt(&h, tab);
    assert!(retry);
    h.save_secret(id);
    h.send(Command::Answer { prompt: id, answer: PromptAnswer::Secret(Secret("pw".into())) }).unwrap();
    wait_state(&h, tab, |s| *s == ConnState::Connected);
    assert_eq!(records.0.lock().unwrap().get(&7).map(String::as_str), Some("pw"));

    // Next connection uses it without prompting.
    let tab = open_session(&h, session.clone());
    wait_for(&h, |ev| match ev {
        Event::Prompt { .. } => panic!("prompted despite a saved password"),
        Event::State { tab: t, state: ConnState::Connected } if *t == tab => Some(()),
        _ => None,
    });

    // A rejected saved password is forgotten and the user is asked.
    records.0.lock().unwrap().insert(7, "stale".into());
    let tab = open_session(&h, session);
    let (id, retry) = password_prompt(&h, tab);
    assert!(retry);
    assert!(records.0.lock().unwrap().is_empty());
    h.send(Command::Answer { prompt: id, answer: PromptAnswer::Cancel }).unwrap();
    let answers = state.answers.lock().unwrap().clone();
    assert!(answers.contains(&PromptAnswer::Secret(Secret("stale".into()))));
    h.shutdown(WAIT);
}

mod agent;

fn serial_session(port: &str) -> Session {
    Session::serial(port, rxt_core::LineConfig::default())
}

fn open_serial(h: &EngineHandle, port: &str) -> TabId {
    h.send(Command::Open { session: Box::new(serial_session(port)) }).unwrap();
    let port = port.to_owned();
    wait_for(h, |ev| match ev {
        Event::TabOpened { tab, session } if session.serial.port == port => Some(*tab),
        _ => None,
    })
}

#[test]
fn multiexec_reaches_telnet_and_serial_tabs() {
    let (h, state, _) = start(1024);
    let ssh = connect(&h, "a");
    let telnet = connect_proto(&h, Protocol::Telnet, "router");
    let serial = open_serial(&h, "/dev/ttyS1");
    wait_state(&h, serial, |s| *s == ConnState::Connected);
    for tab in [ssh, telnet, serial] {
        h.send(Command::SelectMultiExec { tab, selected: true }).unwrap();
    }
    h.send(Command::SetMultiExec { enabled: true }).unwrap();
    h.send(Command::Input { tab: ssh, bytes: b"uptime\r".to_vec() }).unwrap();
    wait_until(|| ["a", "router", "/dev/ttyS1"].iter().all(|k| state.writes_to(k).len() == 1));
    // Each protocol writer applies its own newline (Telnet CR LF, serial Enter setting).
    assert_eq!(state.writes_to("router"), [b"uptime\r".to_vec()]);
    h.shutdown(WAIT);
}

#[test]
fn serial_line_commands_reach_the_port_and_fail_on_other_tabs() {
    let (h, state, _) = start(1024);
    let serial = open_serial(&h, "/dev/ttyS1");
    wait_state(&h, serial, |s| *s == ConnState::Connected);
    h.send(Command::SerialBreak { tab: serial, ms: 250 }).unwrap();
    h.send(Command::SerialLines { tab: serial, dtr: Some(false), rts: None }).unwrap();
    wait_until(|| state.lines.lock().unwrap().len() == 2);
    // Break and line changes run on separate tasks: their order is not guaranteed.
    let mut lines = state.lines.lock().unwrap().clone();
    lines.sort();
    assert_eq!(lines, ["break 250", "dtr Some(false) rts None"]);
    let ssh = connect(&h, "a");
    h.send(Command::SerialBreak { tab: ssh, ms: 250 }).unwrap();
    let msg = wait_for(&h, |ev| match ev {
        Event::Notice { tab: Some(t), message } if *t == ssh => Some(message.clone()),
        _ => None,
    });
    assert!(msg.contains("not a connected serial session"), "{msg}");
    h.shutdown(WAIT);
}

#[test]
fn serial_port_list_and_unsupported_platform() {
    let (h, _, _) = start(1024);
    h.send(Command::ListSerialPorts).unwrap();
    let r = wait_for(&h, |ev| match ev {
        Event::SerialPorts { result } => Some(result.clone()),
        _ => None,
    });
    assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");
    h.shutdown(WAIT);

    let ports = rxt_platform::serial::FakeSerial::default();
    let info = rxt_core::PortInfo { path: "/dev/ttyUSB3".into(), ..Default::default() };
    ports.plug(info.clone());
    let (h, _, _) = start_config(EngineConfig { serial_ports: Some(Arc::new(ports)), ..EngineConfig::default() });
    h.send(Command::ListSerialPorts).unwrap();
    let r = wait_for(&h, |ev| match ev {
        Event::SerialPorts { result } => Some(result.clone()),
        _ => None,
    });
    assert_eq!(r, Ok(vec![info]));
    h.shutdown(WAIT);
}

#[test]
fn serial_unplug_shows_device_removed_then_reopens_by_identity() {
    let ports = rxt_platform::serial::FakeSerial::default();
    let by_id = Some("/dev/serial/by-id/usb-FTDI_FT232R_A1B2-if00-port0".to_owned());
    ports.plug(rxt_core::PortInfo { path: "/dev/ttyUSB0".into(), by_id: by_id.clone(), ..Default::default() });
    let config = EngineConfig { serial_ports: Some(Arc::new(ports.clone())), ..EngineConfig::default() };
    let (h, state, _) = start_config(config);
    *state.serial.lock().unwrap() = Some(ports.clone());
    let tab = open_serial(&h, "/dev/ttyUSB0");
    let removed = wait_state(&h, tab, |s| matches!(s, ConnState::Disconnected { .. }));
    assert_eq!(removed, ConnState::Disconnected { reason: "device removed".into(), reconnect_at: None });
    ports.unplug("/dev/ttyUSB0");
    wait_until(|| state.connects.lock().unwrap().len() >= 2);
    // Failed reopens while the device is away keep the "device removed" state.
    sleep(Duration::from_millis(1200));
    while let Some(ev) = h.try_event() {
        assert!(!matches!(ev, Event::State { tab: t, .. } if t == tab), "state changed while away: {ev:?}");
    }
    // Replugged under a new name: matched by its by-id identity.
    ports.plug(rxt_core::PortInfo { path: "/dev/ttyUSB1".into(), by_id, ..Default::default() });
    let started = Instant::now();
    wait_state(&h, tab, |s| *s == ConnState::Connected);
    assert!(started.elapsed() < Duration::from_secs(2), "reconnect within 2 s of the replug");
    let connects = state.connects.lock().unwrap().clone();
    assert!(connects[1..connects.len() - 1].iter().all(|c| c == "/dev/ttyUSB0"), "{connects:?}");
    assert_eq!(connects.last().map(String::as_str), Some("/dev/ttyUSB1"));
    h.shutdown(WAIT);
}

fn cwd_events(h: &EngineHandle, tab: TabId, within: Duration) -> Vec<String> {
    let deadline = Instant::now() + within;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        match h.try_event() {
            Some(Event::CwdChanged { tab: t, path }) if t == tab => out.push(path),
            Some(_) => {}
            None => sleep(Duration::from_millis(5)),
        }
    }
    out
}

#[test]
fn enter_reports_login_shell_folder_once_per_change() {
    let (h, state, _) = start(1024);
    *state.cwd.lock().unwrap() = "/home/alice".into();
    let tab = connect(&h, "follow");
    // The first check only records the start folder.
    wait_until(|| state.fs_calls.lock().unwrap().iter().any(|c| c == "realpath /proc/102/cwd"));
    assert_eq!(*state.execs.lock().unwrap(), [crate::follow::PS_COMMAND]);
    assert!(cwd_events(&h, tab, Duration::from_millis(100)).is_empty());
    *state.cwd.lock().unwrap() = "/etc".into();
    // A paste with several lines and a typed Enter right after: one debounced check.
    let calls = |s: &State| s.fs_calls.lock().unwrap().iter().filter(|c| c.starts_with("realpath")).count();
    let before = calls(&state);
    h.send(Command::Input { tab, bytes: b"cd /etc\rpwd\n".to_vec() }).unwrap();
    h.send(Command::Input { tab, bytes: b"\r".to_vec() }).unwrap();
    assert_eq!(cwd_events(&h, tab, Duration::from_millis(900)), ["/etc"]);
    assert_eq!(calls(&state), before + 1);
    // Unchanged folder: checked, not reported. Keys without Enter: not checked.
    h.send(Command::Input { tab, bytes: b"ls\r".to_vec() }).unwrap();
    h.send(Command::Input { tab, bytes: b"cd /tmp".to_vec() }).unwrap();
    assert!(cwd_events(&h, tab, Duration::from_millis(900)).is_empty());
    assert_eq!(calls(&state), before + 2);
    // Follow off: Enter is not checked; turning it on reports the folder right away.
    h.send(Command::SetFollowCwd { tab, enabled: false }).unwrap();
    *state.cwd.lock().unwrap() = "/tmp".into();
    h.send(Command::Input { tab, bytes: b"\r".to_vec() }).unwrap();
    assert!(cwd_events(&h, tab, Duration::from_millis(900)).is_empty());
    h.send(Command::SetFollowCwd { tab, enabled: true }).unwrap();
    assert_eq!(cwd_events(&h, tab, Duration::from_millis(900)), ["/tmp"]);
    h.shutdown(WAIT);
}

#[test]
fn follow_disabled_without_ps_or_proc_and_never_probed_when_off() {
    let (h, state, _) = start(1024);
    let tab = connect(&h, "busybox");
    wait_until(|| !state.execs.lock().unwrap().is_empty());
    h.send(Command::Input { tab, bytes: b"cd /etc\r".to_vec() }).unwrap();
    assert!(cwd_events(&h, tab, Duration::from_millis(900)).is_empty());
    assert!(state.fs_calls.lock().unwrap().is_empty(), "no realpath without a shell PID");

    // `/proc` unreadable: one re-detection, then the fallback stops.
    let (h2, state2, _) = start(1024);
    let tab2 = connect(&h2, "follow");
    wait_until(|| state2.execs.lock().unwrap().len() == 2);
    h2.send(Command::Input { tab: tab2, bytes: b"\r".to_vec() }).unwrap();
    assert!(cwd_events(&h2, tab2, Duration::from_millis(900)).is_empty());
    assert_eq!(state2.execs.lock().unwrap().len(), 2);
    assert_eq!(state2.fs_calls.lock().unwrap().len(), 2);

    // Session option off: no probe until it is turned on.
    let (h3, state3, _) = start(1024);
    *state3.cwd.lock().unwrap() = "/srv".into();
    let mut s = Session::new(Protocol::Ssh, "follow", None, None);
    s.ssh.follow_terminal_folder = false;
    let tab3 = open_session(&h3, s);
    wait_state(&h3, tab3, |s| *s == ConnState::Connected);
    h3.send(Command::Input { tab: tab3, bytes: b"cd /srv\r".to_vec() }).unwrap();
    assert!(cwd_events(&h3, tab3, Duration::from_millis(900)).is_empty());
    assert!(state3.execs.lock().unwrap().is_empty());
    h3.send(Command::SetFollowCwd { tab: tab3, enabled: true }).unwrap();
    assert_eq!(cwd_events(&h3, tab3, Duration::from_secs(2)), ["/srv"]);
    for h in [h, h2, h3] {
        h.shutdown(WAIT);
    }
}

fn wait_monitor(h: &EngineHandle, tab: TabId, want: &rxt_core::MonitorState) {
    wait_for(h, |ev| matches!(ev, Event::Monitor { tab: t, state } if *t == tab && state == want).then_some(()));
}

#[test]
fn monitoring_samples_toggle_reconnect_and_close_the_channel() {
    let (h, state, _) = start(1 << 20);
    let tab = connect(&h, "mon");
    wait_for(&h, |ev| matches!(ev, Event::Metrics { tab: t } if *t == tab).then_some(()));
    let s = h.take_metrics(tab).unwrap();
    assert_eq!((s.host.as_str(), s.mem), ("fake-host", Some((750 * 1024, 1000 * 1024))));

    h.send(Command::SetMonitoring { tab, enabled: false }).unwrap();
    wait_monitor(&h, tab, &rxt_core::MonitorState::Off);
    wait_until(|| state.mon_closed.load(Ordering::SeqCst) == 1);
    h.send(Command::SetMonitoring { tab, enabled: true }).unwrap();
    wait_for(&h, |ev| matches!(ev, Event::Metrics { tab: t } if *t == tab).then_some(()));
    assert_eq!(state.mon_opens.load(Ordering::SeqCst), 2);

    // Reconnect drops the run (and its channel); the new run monitors again.
    h.send(Command::Reconnect { tab }).unwrap();
    wait_until(|| state.mon_closed.load(Ordering::SeqCst) == 2);
    wait_until(|| state.mon_opens.load(Ordering::SeqCst) == 3);
    h.send(Command::SetMonitorInterval { secs: 0 }).unwrap();
    h.send(Command::Close { tab }).unwrap();
    wait_until(|| state.mon_closed.load(Ordering::SeqCst) == 3);
    assert_eq!(h.take_metrics(tab), None);
}

#[test]
fn monitoring_follows_settings_and_only_ssh_terminals() {
    let (h, state, _) = start(1 << 20);
    h.set_remote_monitoring(false);
    let off = connect(&h, "mon-off");
    wait_monitor(&h, off, &rxt_core::MonitorState::Off);
    // The session's choice overrides the global setting.
    let mut on = Session::new(Protocol::Ssh, "mon-on", None, None);
    on.ssh.remote_monitoring = Some(true);
    let on = open_session(&h, on);
    wait_for(&h, |ev| matches!(ev, Event::Metrics { tab: t } if *t == on).then_some(()));
    let mut telnet = Session::new(Protocol::Telnet, "mon-telnet", None, None);
    telnet.ssh.remote_monitoring = Some(true);
    let telnet = open_session(&h, telnet);
    wait_state(&h, telnet, |s| *s == ConnState::Connected);
    let mut cmd = Session::new(Protocol::Ssh, "mon-cmd", None, None);
    (cmd.ssh.remote_monitoring, cmd.ssh.remote_command) = (Some(true), Some("uptime".into()));
    let cmd = open_session(&h, cmd);
    wait_state(&h, cmd, |s| *s == ConnState::Connected);
    sleep(Duration::from_millis(200));
    assert_eq!(state.mon_opens.load(Ordering::SeqCst), 1, "only the SSH terminal with monitoring on");

    // Telnet links have no exec channel.
    struct NoPrompt;
    impl Prompter for NoPrompt {
        fn prompt(&self, _: PromptKind) -> BoxFut<'_, PromptAnswer> {
            Box::pin(async { PromptAnswer::Cancel })
        }
        fn host_key_changed(&self, _: &str, _: &str, _: &str) {}
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        use crate::transport::Real;
        let real = Real::new(std::path::PathBuf::new(), None, None);
        let session = Session::new(Protocol::Telnet, "router.lab.example", None, None);
        let link = real.connect(session, Arc::new(NoPrompt), CancellationToken::new()).await.unwrap();
        assert!(matches!(link.exec_stream("true").await, Err(Error::Unsupported(_))));
    });
}
