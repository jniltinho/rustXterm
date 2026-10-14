//! Remote monitoring against the lab sshd (Ubuntu, OpenSSH). `carol` runs the monitored tab and a
//! second, unmonitored carol session counts `rxt-mon` processes with `ps`; `dave` has
//! `MaxSessions 1`, so the monitoring channel is refused. Run with `lab/lab.sh up && lab/lab.sh
//! test`. Fictional lab credentials.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rxt_core::{
    Command, ConnState, Event, HostKeyDecision, MetricsSample, MonitorState, PromptAnswer, PromptKind, Protocol,
    Secret, Session, TabId,
};
use rxt_engine::{Engine, EngineConfig, EngineHandle};

fn session(user: &str, monitoring: Option<bool>) -> Session {
    let mut s = Session::new(Protocol::Ssh, "127.0.0.1", Some(user.into()), Some(2222));
    s.ssh.use_agent = false;
    s.ssh.follow_terminal_folder = false;
    s.ssh.remote_monitoring = monitoring;
    s
}

/// Open `s`, answer the login prompts with `pass` and wait for the shell prompt. Events seen on
/// the way are returned.
fn connect(h: &EngineHandle, s: Session, pass: &str) -> (TabId, Vec<Event>) {
    h.send(Command::Open { session: Box::new(s) }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut tab, mut seen) = (None, Vec::new());
    loop {
        assert!(Instant::now() < deadline, "login timed out");
        let Some(ev) = h.try_event() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        match &ev {
            Event::TabOpened { tab: t, .. } => tab = Some(*t),
            Event::Prompt { id, kind, .. } => {
                let answer = match kind {
                    PromptKind::HostKeyUnknown { .. } => PromptAnswer::HostKey(HostKeyDecision::AcceptOnce),
                    PromptKind::Password { .. } => PromptAnswer::Secret(Secret(pass.into())),
                    PromptKind::KeyboardInteractive { prompts, .. } => {
                        PromptAnswer::Responses(prompts.iter().map(|_| Secret(pass.into())).collect())
                    }
                    _ => PromptAnswer::Cancel,
                };
                h.send(Command::Answer { prompt: *id, answer }).unwrap();
            }
            Event::State { tab: t, state: ConnState::Connected } if Some(*t) == tab => break,
            Event::State { state: ConnState::Error(e), .. } => panic!("login failed: {e}"),
            _ => seen.push(ev),
        }
    }
    let tab = tab.unwrap();
    output_until(h, tab, "$ ", Duration::from_secs(20), &mut seen);
    (tab, seen)
}

/// Drain `tab`'s output until it contains `needle`; events go to `seen`. Returns the output.
fn output_until(h: &EngineHandle, tab: TabId, needle: &str, within: Duration, seen: &mut Vec<Event>) -> String {
    let deadline = Instant::now() + within;
    let mut out = Vec::new();
    while !String::from_utf8_lossy(&out).contains(needle) {
        assert!(Instant::now() < deadline, "no {needle:?} in {:?}", String::from_utf8_lossy(&out));
        while let Some(ev) = h.try_event() {
            seen.push(ev);
        }
        if h.drain_output(tab, 1 << 20, &mut out) == 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run `cmd` in `tab` and return what it printed between `RXT<` and `>RXT`.
fn run(h: &EngineHandle, tab: TabId, cmd: &str) -> String {
    h.send(Command::Input { tab, bytes: format!("echo \"RXT<$({cmd})>RXT\"\r").into_bytes() }).unwrap();
    let out = output_until(h, tab, ">RXT\r\n", Duration::from_secs(10), &mut Vec::new());
    // The echoed command line has `RXT<$(`; the output line starts right after `RXT<`.
    let start = out.rfind("RXT<").unwrap() + 4;
    out[start..out.rfind(">RXT").unwrap()].to_owned()
}

/// `rxt-mon` loops of carol on the server (the loop's own `$(…)` subshells, which carry the
/// same command line, are not counted). No `!`: interactive bash would expand it.
fn monitors(h: &EngineHandle, check: TabId) -> usize {
    let cmd = "ps -u carol -o pid=,ppid=,args= | awk '/[r]xt-mon 1/ { m[$1] = $2 } \
               END { n = 0; for (p in m) if (m[p] in m) {} else n++; print n }'";
    run(h, check, cmd).trim().parse().unwrap()
}

fn wait_monitors(h: &EngineHandle, check: TabId, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let n = monitors(h, check);
        if n == want {
            return;
        }
        assert!(Instant::now() < deadline, "{n} rxt-mon processes, want {want} within 5 s");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Take samples of `tab` (like the UI painting its bar) until `done` holds for the merged view.
fn sample_until(
    h: &EngineHandle,
    tab: TabId,
    within: Duration,
    done: impl Fn(&MetricsSample) -> bool,
) -> MetricsSample {
    let deadline = Instant::now() + within;
    let mut last = MetricsSample::default();
    loop {
        while h.try_event().is_some() {}
        if let Some(s) = h.take_metrics(tab) {
            // Disks arrive every 30 s at most; keep the last ones like the bar does.
            let disks = s.disks.clone().or(last.disks.take());
            last = MetricsSample { disks, ..s };
            if done(&last) {
                return last;
            }
        }
        assert!(Instant::now() < deadline, "incomplete sample after {within:?}: {last:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "lab"]
fn ssh_monitoring_bar_data_then_no_leftover_process() {
    let h = Engine::start(EngineConfig::default(), Arc::new(|| {})).unwrap();
    let (check, _) = connect(&h, session("carol", Some(false)), "lab-carol-pass");
    let files = "find ~ /tmp -xdev -user carol -printf '%p %T@\\n' 2>/dev/null | sort | md5sum";
    let before = run(&h, check, files);
    assert_eq!(monitors(&h, check), 0);

    let (tab, _) = connect(&h, session("carol", None), "lab-carol-pass");
    let start = Instant::now();
    let s = sample_until(&h, tab, Duration::from_secs(5), |s| {
        s.cpu.is_some() && s.mem.is_some() && s.net.is_some() && s.uptime_secs.is_some() && s.disks.is_some()
    });
    assert!(start.elapsed() <= Duration::from_secs(5));
    assert!(!s.host.is_empty() && s.os == "Linux", "{s:?}");
    let (used, total) = s.mem.unwrap();
    assert!(used > 0 && used < total);
    assert!(s.disks.as_ref().unwrap().iter().any(|d| d.mount == "/"), "{:?}", s.disks);
    assert_eq!(monitors(&h, check), 1, "exactly one monitoring process for the tab");

    // Toggle off: the channel closes and the remote loop exits.
    h.send(Command::SetMonitoring { tab, enabled: false }).unwrap();
    wait_monitors(&h, check, 0);
    h.send(Command::SetMonitoring { tab, enabled: true }).unwrap();
    sample_until(&h, tab, Duration::from_secs(5), |s| s.mem.is_some());
    wait_monitors(&h, check, 1);

    // Disconnect (tab closed): no process left behind.
    h.send(Command::Close { tab }).unwrap();
    wait_monitors(&h, check, 0);
    assert_eq!(run(&h, check, files), before, "monitoring changed files under $HOME or /tmp");
    h.shutdown(Duration::from_secs(5));
}

#[test]
#[ignore = "lab"]
fn ssh_monitoring_refused_channel_is_unavailable_and_shell_works() {
    let h = Engine::start(EngineConfig::default(), Arc::new(|| {})).unwrap();
    let (tab, mut seen) = connect(&h, session("dave", None), "lab-dave-pass");
    let deadline = Instant::now() + Duration::from_secs(5);
    let reason = loop {
        if let Some(reason) = seen.iter().find_map(|e| match e {
            Event::Monitor { tab: t, state: MonitorState::Unavailable(r) } if *t == tab => Some(r.clone()),
            _ => None,
        }) {
            break reason;
        }
        assert!(Instant::now() < deadline, "monitoring not reported unavailable: {seen:?}");
        match h.try_event() {
            Some(ev) => seen.push(ev),
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(reason.contains("refused"), "{reason}");
    assert_eq!(run(&h, tab, "echo $((40 + 2))"), "42", "the shell keeps working");
    h.shutdown(Duration::from_secs(5));
}
