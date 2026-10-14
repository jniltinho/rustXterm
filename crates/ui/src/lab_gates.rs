//! Gate measurements through the real engine and the egui UI against the Docker lab:
//! M1 (task 3.4, `docs/gates/m1.md`): two SSH tabs, one flooding for 30 s while the kittest harness
//! renders frames. M2 (tasks 5.2/5.3, `docs/gates/m2.md`): tab switch while an RDP/VNC desktop
//! streams continuous updates. `#[ignore = "lab"]`; credentials are the fictional lab accounts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use rxt_core::{
    Command, ConnState, DesktopInput, Event, Framebuffer, HostKeyDecision, PromptAnswer, PromptKind, Protocol, Secret,
    Session, TabId,
};
use rxt_engine::{Engine, EngineConfig, EngineHandle};
use rxt_platform::fake::Fake;

use crate::app::{Action, App, Backend};

const FLOOD: &[u8] = b"yes \"$(head -c 200 /dev/urandom | base64 -w0)\"\n";
const FLOOD_FOR: Duration = Duration::from_secs(30);
/// Gates and e2e tests run one at a time: they share the CPU budget and xrdp keeps one session per user.
pub(crate) static ONE_GATE: Mutex<()> = Mutex::new(());

type Watch = Rc<RefCell<(Option<TabId>, String)>>;

/// Real engine that answers lab prompts itself, tees one tab's drained output and counts the
/// desktop updates (framebuffer dirty rects) the UI consumes.
struct LabEngine {
    inner: EngineHandle,
    watch: Watch,
    updates: Rc<Cell<(u64, u64)>>,
}

impl Backend for LabEngine {
    fn send(&self, command: Command) -> rxt_core::Result<()> {
        self.inner.send(command)
    }
    fn try_event(&self) -> Option<Event> {
        loop {
            match self.inner.try_event()? {
                Event::Prompt { id, kind, .. } => {
                    let answer = match kind {
                        PromptKind::HostKeyUnknown { .. } | PromptKind::CertificateUnknown { .. } => {
                            PromptAnswer::HostKey(HostKeyDecision::AcceptOnce)
                        }
                        // Fictional lab passwords: `alice` (SSH, RDP) and the VNC server (no user).
                        PromptKind::Password { user, .. } => {
                            let pass = if user.is_empty() { "lab-vnc1" } else { "lab-alice-pass" };
                            PromptAnswer::Secret(Secret(pass.into()))
                        }
                        _ => PromptAnswer::Cancel,
                    };
                    self.inner.send(Command::Answer { prompt: id, answer }).unwrap();
                }
                ev => return Some(ev),
            }
        }
    }
    fn drain_output(&self, tab: TabId, budget: usize, out: &mut Vec<u8>) -> usize {
        let start = out.len();
        let n = self.inner.drain_output(tab, budget, out);
        let mut w = self.watch.borrow_mut();
        if w.0 == Some(tab) {
            w.1.push_str(&String::from_utf8_lossy(&out[start..]));
        }
        n
    }
    fn pending_output(&self) -> Vec<TabId> {
        self.inner.pending_output()
    }
    fn with_framebuffer(&self, tab: TabId, f: &mut dyn FnMut(&mut Framebuffer)) -> bool {
        self.inner
            .with_framebuffer(tab, |fb| {
                let (n, px) = self.updates.get();
                self.updates
                    .set((n + fb.dirty.len() as u64, px + fb.dirty.iter().map(rxt_core::Rect::area).sum::<u64>()));
                f(fb);
            })
            .is_some()
    }
}

struct Lab {
    h: Harness<'static, App>,
    handle: EngineHandle,
    watch: Watch,
    updates: Rc<Cell<(u64, u64)>>,
}

/// Engine + UI harness; `sessions` are opened in order and must all connect.
fn lab(name: &str, sessions: &[Session]) -> Lab {
    let kh = std::env::temp_dir().join(format!("rxt-lab-gates-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&kh).unwrap();
    let config = EngineConfig { known_hosts: kh.join("known_hosts"), ..EngineConfig::default() };
    let handle = Engine::start(config, Arc::new(|| {})).unwrap();
    let (watch, updates) = (Rc::new(RefCell::new((None, String::new()))), Rc::new(Cell::new((0, 0))));
    let platform = Arc::new(Fake { root: kh.join("platform"), ..Fake::default() });
    let backend = LabEngine { inner: handle.clone(), watch: watch.clone(), updates: updates.clone() };
    let app = App::new(None, platform.platform(), backend);
    let mut h = Harness::builder().with_size(vec2(1180.0, 720.0)).build_ui_state(|ui, app: &mut App| app.show(ui), app);
    for s in sessions {
        h.state_mut().apply(Action::Open(Box::new(s.clone())));
        h.run_steps(2);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while h.state().tabs.len() < sessions.len() || h.state().tabs.iter().any(|t| t.state != ConnState::Connected) {
        assert!(Instant::now() < deadline, "tabs did not connect");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    Lab { h, handle, watch, updates }
}

fn rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

fn report(name: &str, samples: &mut [Duration], limit_ms: f64) -> f64 {
    samples.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let at = |q: f64| ms(samples[((samples.len() as f64 * q).ceil() as usize).clamp(1, samples.len()) - 1]);
    let (p50, p95, max) = (at(0.50), at(0.95), ms(samples[samples.len() - 1]));
    println!("GATE {name}: n={} p50={p50:.2}ms p95={p95:.2}ms max={max:.2}ms limit={limit_ms}ms", samples.len());
    p95
}

/// Step frames until `watch` contains `needle`.
fn step_until(h: &mut Harness<'static, App>, watch: &RefCell<(Option<TabId>, String)>, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !watch.borrow().1.contains(needle) {
        assert!(Instant::now() < deadline, "timed out waiting for {needle:?}");
        h.step();
    }
}

#[test]
#[ignore = "lab"]
fn ui_gate_flood_tab_switch_echo_memory_resize() {
    let _one = ONE_GATE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut s = Session::new(Protocol::Ssh, "127.0.0.1", Some("alice".into()), Some(2222));
    s.ssh.use_agent = false;
    let Lab { mut h, handle, watch, .. } = lab("ssh", &[s.clone(), s]);
    let (flood, sib) = (h.state().tabs[0].id, h.state().tabs[1].id);
    let cols = h.state().tabs[0].term.as_ref().map_or(80, rxt_term::Terminal::cols);
    watch.borrow_mut().0 = Some(sib);
    handle.send(Command::Input { tab: sib, bytes: b"stty -echo; echo READY\n".to_vec() }).unwrap();
    step_until(&mut h, &watch, "READY\r\n");
    h.render().unwrap();
    let idle_kib = rss_kib();

    handle.send(Command::Input { tab: flood, bytes: FLOOD.to_vec() }).unwrap();
    let (mut switch, mut echo, mut frames) = (Vec::new(), Vec::new(), 0u64);
    let start = Instant::now();
    let mut n = 0u32;
    while start.elapsed() < FLOOD_FOR {
        // (c) tab switch: apply the selection, then build and render the next frame.
        let target = if h.state().active == Some(flood) { sib } else { flood };
        let t = Instant::now();
        h.state_mut().apply(Action::SelectTab(Some(target)));
        h.step();
        h.render().unwrap();
        switch.push(t.elapsed());
        // (b) sibling echo: input command to the marker drained into the UI.
        n += 1;
        let marker = format!("E{}X", 1_000_000 + n);
        watch.borrow_mut().1.clear();
        let t = Instant::now();
        handle.send(Command::Input { tab: sib, bytes: format!("echo E$((1000000+{n}))X\n").into_bytes() }).unwrap();
        step_until(&mut h, &watch, &marker);
        echo.push(t.elapsed());
        // Keep rendering frames for a while, like an idle user watching the flood.
        let pause = Instant::now();
        while pause.elapsed() < Duration::from_millis(100) {
            h.step();
            frames += 1;
        }
    }
    let flood_kib = rss_kib();

    // Resize the sibling through the engine while the flood keeps running.
    let mut stty = Vec::new();
    for k in 0..10u16 {
        let (c, r) = (100 + k, 30 + k % 7);
        let t = Instant::now();
        handle.send(Command::Resize { tab: sib, cols: c, rows: r }).unwrap();
        loop {
            watch.borrow_mut().1.clear();
            handle.send(Command::Input { tab: sib, bytes: b"stty size; echo SZ\n".to_vec() }).unwrap();
            step_until(&mut h, &watch, "SZ\r\n");
            if watch.borrow().1.contains(&format!("{r} {c}")) {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "remote never reported {r} {c}");
        }
        stty.push(t.elapsed());
    }
    handle.send(Command::Input { tab: flood, bytes: b"\x03".to_vec() }).unwrap();
    handle.shutdown(Duration::from_secs(2));

    let bound_kib = (10_000 * cols as u64 * 32 + (64 << 20)) / 1024;
    let growth = flood_kib.saturating_sub(idle_kib);
    println!("GATE frames stepped between samples: {frames}; terminal cols {cols}");
    println!(
        "GATE rss: idle={idle_kib} KiB after-flood={flood_kib} KiB growth={growth} KiB bound={bound_kib} KiB (10000 x {cols} x 32 B + 64 MiB)"
    );
    let a = report("ui tab switch to rendered frame", &mut switch, 100.0);
    let b = report("ui sibling echo (engine + drain)", &mut echo, 100.0);
    let c = report("engine Command::Resize to remote stty size", &mut stty, 500.0);
    let worst = stty.last().unwrap().as_secs_f64() * 1000.0;
    // Frame latencies are gated on optimized builds (`--release`, the receipt run); an unoptimized
    // `lab test` build spends ~50 ms per frame feeding 1 MiB to the terminal, so it only reports them.
    if cfg!(debug_assertions) {
        println!("GATE debug build: tab-switch and echo thresholds reported, not asserted");
    } else {
        assert!(a <= 100.0, "tab switch p95 {a} ms");
        assert!(b <= 100.0, "sibling echo p95 {b} ms");
    }
    assert!(c <= 500.0 && worst <= 500.0, "stty max {worst} ms");
    assert!(growth <= bound_kib, "rss growth {growth} KiB > bound {bound_kib} KiB");
}

/// Type `text` into the remote desktop; unshifted characters only (RDP sends scancodes).
fn type_text(handle: &EngineHandle, tab: TabId, text: &str) {
    for c in text.chars() {
        let keysym = if c == '\n' { 0xff0d } else { u32::from(c) };
        for down in [true, false] {
            handle.send(Command::DesktopInput { tab, input: DesktopInput::Key { keysym, down } }).unwrap();
        }
    }
}

/// Tab switch between a desktop tab whose xterm runs a flood loop ("video") and an SSH tab:
/// `SelectTab`, then the next frame built and rendered. `focus` is a point inside the xterm.
fn desktop_tab_switch_gate(desktop: Session, focus: (u32, u32)) {
    const RUN_FOR: Duration = Duration::from_secs(20);
    let _one = ONE_GATE.lock().unwrap_or_else(PoisonError::into_inner);
    let name = format!("{:?}", desktop.protocol);
    let mut ssh = Session::new(Protocol::Ssh, "127.0.0.1", Some("alice".into()), Some(2222));
    ssh.ssh.use_agent = false;
    let Lab { mut h, handle, updates, .. } = lab(&name, &[ssh, desktop]);
    let (term, desk) = (h.state().tabs[0].id, h.state().tabs[1].id);
    h.state_mut().apply(Action::SelectTab(Some(desk)));
    let deadline = Instant::now() + Duration::from_secs(30);
    while updates.get().0 == 0 {
        assert!(Instant::now() < deadline, "{name}: no desktop update");
        h.step();
    }
    let (x, y) = focus;
    for buttons in [1, 0] {
        handle.send(Command::DesktopInput { tab: desk, input: DesktopInput::Pointer { x, y, buttons } }).unwrap();
    }
    type_text(&handle, desk, "while true; do find /usr; done\n");
    // Let the loop start, then measure.
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(2) {
        h.step();
    }

    let (mut switch, mut visible, mut frames) = (Vec::new(), Duration::ZERO, 0u64);
    let before = updates.get();
    let start = Instant::now();
    while start.elapsed() < RUN_FOR {
        let target = if h.state().active == Some(desk) { term } else { desk };
        let t = Instant::now();
        h.state_mut().apply(Action::SelectTab(Some(target)));
        h.step();
        h.render().unwrap();
        switch.push(t.elapsed());
        let pause = Instant::now();
        while pause.elapsed() < Duration::from_millis(150) {
            h.step();
            frames += 1;
        }
        if target == desk {
            visible += pause.elapsed();
        }
    }
    let after = updates.get();
    // Stop the loop (Ctrl-C) so the shared lab desktop goes idle again.
    h.state_mut().apply(Action::SelectTab(Some(desk)));
    h.step();
    for (keysym, down) in [(0xffe3, true), (u32::from(b'c'), true), (u32::from(b'c'), false), (0xffe3, false)] {
        handle.send(Command::DesktopInput { tab: desk, input: DesktopInput::Key { keysym, down } }).unwrap();
    }
    let stop = Instant::now();
    while stop.elapsed() < Duration::from_secs(1) {
        h.step();
    }
    handle.shutdown(Duration::from_secs(2));

    let (n, px) = (after.0 - before.0, after.1 - before.1);
    let secs = visible.as_secs_f64();
    println!(
        "GATE {name} updates while visible: {n} rects, {px} px in {secs:.1} s = {:.0} updates/s, {:.2} Mpx/s; frames stepped {frames}",
        n as f64 / secs,
        px as f64 / secs / 1e6
    );
    let p95 = report(&format!("{name} tab switch to rendered frame during remote video"), &mut switch, 100.0);
    // Busy remote: more than one 1024x768 screen of updated pixels per second.
    assert!(px as f64 / secs >= 1e6, "{name}: remote was not busy ({px} px in {secs:.1} s)");
    if cfg!(debug_assertions) {
        println!("GATE debug build: tab-switch threshold reported, not asserted");
    } else {
        assert!(p95 <= 100.0, "{name} tab switch p95 {p95} ms");
    }
}

#[test]
#[ignore = "lab"]
fn ui_gate_tab_switch_during_rdp_video() {
    let s = Session::new(Protocol::Rdp, "127.0.0.1", Some("alice".into()), Some(13389));
    desktop_tab_switch_gate(s, (100, 100));
}

#[test]
#[ignore = "lab"]
fn ui_gate_tab_switch_during_vnc_video() {
    // No window manager: focus follows the pointer into the xterm at the top left.
    let s = Session::new(Protocol::Vnc, "127.0.0.1", None, Some(5901));
    desktop_tab_switch_gate(s, (20, 20));
}
