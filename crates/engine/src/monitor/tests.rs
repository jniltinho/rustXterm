//! Parser fixtures (hand-written from lab containers, fictional host names) and collector
//! tests against a scripted peer under paused time.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rxt_core::{BoxFut, Error, Event, FileOps, MonitorState, Result, Session, TabId, Tunnel};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::transport::{ExecStream, OnTunnelState, ShellPair};

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

const NET_HEADER: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed";

/// Ubuntu 24.04 VM: one default route on `eth0`.
fn ubuntu(cpu: &str, uptime: &str, eth_rx: u64, eth_tx: u64) -> String {
    format!(
        "cpu  {cpu}
MemTotal:       16303428 kB
MemFree:         1240212 kB
MemAvailable:    9832440 kB
Buffers:          395596 kB
Cached:          8112256 kB
@u {uptime} 1388262.02
@r
eth0\t00000000\t0102A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
@n
{NET_HEADER}
    lo: 2776770   11307    0    0    0     0          0         0  2776770   11307    0    0    0     0       0          0
  eth0: {eth_rx}    2751    0    0    0     0          0         0  {eth_tx}    4324    0    0    0     0       0          0
@d
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/sda2         50620216 20480000  27542536      43% /
/dev/sda1           523248     6220    517028       2% /boot/efi
tmpfs              1630344     2000   1628344       1% /run
"
    )
}

#[test]
fn ubuntu_first_then_second_sample() {
    let a = parse_frame(&lines(&ubuntu("100 0 100 800 0 0 0 0 0 0", "1000.00", 1_000_000, 2_000_000)));
    let first = compute("web1", "Linux", None, &a, a.disks.clone());
    assert_eq!(first.cpu, None, "CPU needs two samples");
    assert_eq!(first.net, None);
    assert_eq!(first.mem, Some(((16_303_428 - 9_832_440) * 1024, 16_303_428 * 1024)));
    assert_eq!(first.uptime_secs, Some(1000));
    assert_eq!(first.net_ifaces, ["eth0"]);
    let disks = first.disks.as_ref().unwrap();
    assert_eq!(disks.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(), ["/", "/boot/efi"]);
    assert_eq!(disks[0].percent, 43);

    // +400 busy (300 user/system), +1000 total; 1 MB down, 2 MB up over 2 s.
    let b = parse_frame(&lines(&ubuntu("350 0 150 1400 0 0 100 0 0 0", "1002.00", 2_000_000, 4_000_000)));
    let second = compute("web1", "Linux", Some(&a), &b, None);
    assert_eq!(second.cpu, Some(40.0));
    assert_eq!(second.cpu_user_system, Some(30.0));
    assert_eq!(second.net, Some((8_000_000, 4_000_000)));
    assert_eq!(second.disks, None);
}

#[test]
fn debian_12_two_default_routes_summed() {
    let frame = format!(
        "cpu  5 0 5 90 0 0 0 0\nMemTotal: 2000 kB\nMemAvailable: 500 kB\n@u 50.5 10.0\n@r
ens3\t00000000\t0100000A\t0003\t0\t0\t0\t00000000\t0\t0\t0
ens4\t00000000\t0100000B\t0003\t0\t0\t200\t00000000\t0\t0\t0
ens3\t0000000A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0
@n\n{NET_HEADER}
  ens3: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0
  ens4: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0
docker0: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0
"
    );
    let raw = parse_frame(&lines(&frame));
    assert_eq!(raw.routes, ["ens3", "ens4"]);
    assert_eq!(raw.mem, Some((1500 * 1024, 2000 * 1024)));
    assert_eq!(raw.disks, None, "no df in this frame");
    let next = parse_frame(&lines(&frame.replace("ens4: 100", "ens4: 300").replace("@u 50.5", "@u 51.5")));
    let s = compute("db1", "Linux", Some(&raw), &next, None);
    assert_eq!(s.net, Some((0, 1600)));
    assert_eq!(s.net_ifaces, ["ens3", "ens4"]);
}

#[test]
fn old_kernel_without_memavailable_falls_back() {
    let raw = parse_frame(&lines("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 250 kB\n"));
    assert_eq!(raw.mem, Some((600 * 1024, 1000 * 1024)));
    // Without MemTotal there is no memory value.
    assert_eq!(parse_frame(&lines("MemFree: 100 kB\n")).mem, None);
}

#[test]
fn busybox_alpine_no_default_route_and_plain_df() {
    let frame = format!(
        "cpu  1 2 3 4\nMemTotal: 4000 kB\nMemFree: 1000 kB\n@u 77.10 70.00\n@r\n@n\n{NET_HEADER}
    lo:     500       5    0    0    0     0          0         0      500       5    0    0    0     0       0          0
  eth0:    1000      10    0    0    0     0          0         0     2000      20    0    0    0     0       0          0
@d
Filesystem           1024-blocks    Used Available Capacity Mounted on
overlay                 61255492  9831232  48282952  17% /
tmpfs                      65536        0     65536   0% /dev
shm                        65536        0     65536   0% /dev/shm
/dev/vda1               61255492  9831232  48282952  17% /etc/hosts
/dev/vda1               61255492  9831232  48282952  17% /etc/resolv.conf
nfs.lab.example:/export 1000 500 500 50% /mnt/share
"
    );
    let raw = parse_frame(&lines(&frame));
    assert_eq!(raw.cpu, Some((6, 6, 10)));
    let s = compute("alpine", "Linux", None, &raw, raw.disks.clone());
    assert_eq!(s.net_ifaces, ["eth0"], "all but lo without a default route");
    let disks = s.disks.unwrap();
    // overlay as `/`; the bind-mounted device once, under its shortest mount; NFS dropped.
    assert_eq!(disks.iter().map(|d| d.mount.as_str()).collect::<Vec<_>>(), ["/", "/etc/hosts"]);
}

#[test]
fn freebsd_like_server_has_host_and_disks_only() {
    let raw = parse_frame(&lines(
        "@d\nFilesystem 1024-blocks Used Avail Capacity Mounted on\n/dev/ada0p2 1000 250 750 25% /\n",
    ));
    let s = compute("bsd1", "FreeBSD", None, &raw, raw.disks.clone());
    assert_eq!((s.cpu, s.mem, s.net, s.uptime_secs), (None, None, None, None));
    assert_eq!(s.disks.unwrap()[0].percent, 25);
}

#[test]
fn counter_reset_and_wrap_skip_values() {
    let a = parse_frame(&lines(&ubuntu("100 0 100 800 0 0 0 0", "10.0", 4_000_000_000, 10)));
    let b = parse_frame(&lines(&ubuntu("50 0 50 400 0 0 0 0", "12.0", 100, 20)));
    let s = compute("h", "Linux", Some(&a), &b, None);
    assert_eq!(s.cpu, None, "jiffies went backwards");
    assert_eq!(s.net, None, "32-bit rx counter wrapped");
    assert!(s.mem.is_some());
}

#[test]
fn loop_tmpfs_overlay_filtered_and_capped() {
    let mut df = String::from("@d\nFilesystem 1024-blocks Used Available Capacity Mounted on\n");
    df.push_str("/dev/loop3 100 100 0 100% /snap/core/1\ndevtmpfs 10 0 10 0% /dev\n");
    df.push_str("overlay 100 10 90 10% /var/lib/docker/overlay2/x/merged\nproc 0 0 0 - /proc\n");
    for i in 0..10 {
        df.push_str(&format!("/dev/sd{i} 100 50 50 50% /data/{i}\n"));
    }
    df.push_str("/dev/root 1000 999 1 100% /\nnot a row\n");
    let (disks, more) = parse_frame(&lines(&df)).disks.unwrap();
    assert_eq!(disks.len(), 8);
    assert_eq!(more, 3);
    assert_eq!((disks[0].mount.as_str(), disks[0].percent), ("/", 100));
    assert!(disks.iter().all(|d| d.source.starts_with("/dev/sd") || d.mount == "/"));
    assert_eq!(df_row("/dev/sdb1 100 50 50 50% /mnt/my disk").unwrap().mount, "/mnt/my disk");
}

#[test]
fn missing_sections_hide_values() {
    let raw = parse_frame(&lines("cpu  garbage\n@u notanumber\n@n\n  eth0: 1 2\n"));
    assert_eq!(raw, Raw { disks: None, ..Raw::default() });
}

#[tokio::test]
async fn oversized_lines_dropped_and_header_after_noise() {
    let long = "x".repeat(MAX_LINE + 10);
    let input = format!("Welcome to fictional-lab!\n{long}\nrxt-mon 1 Linux web1\n@s\n{long}\nMemTotal: 1 kB\n@e\n");
    let mut r = BufReader::new(input.as_bytes());
    assert_eq!(handshake(&mut r).await, Some(("Linux".into(), "web1".into())));
    let Reply::Frame(f) = read_frame(&mut r).await else { panic!("frame expected") };
    assert_eq!(f, ["MemTotal: 1 kB"]);
    assert!(matches!(read_frame(&mut r).await, Reply::Eof));
    let big = format!("@s\n{}@e\n", "y\n".repeat(MAX_FRAME));
    assert!(matches!(read_frame(&mut BufReader::new(big.as_bytes())).await, Reply::TooLarge));
}

#[test]
fn script_is_fixed_and_marked() {
    assert!(SCRIPT.contains("rxt-mon 1"));
    assert!(!SCRIPT.contains('!'), "csh history expansion");
    assert_eq!(SCRIPT.matches('\'').count(), 2, "one single-quoted argument");
    assert_eq!(clamp_interval(0), 1);
    assert_eq!(clamp_interval(99), 30);
}

// --- Collector against a scripted peer ---

struct Peer {
    r: BufReader<ReadHalf<DuplexStream>>,
    w: WriteHalf<DuplexStream>,
}

impl Peer {
    async fn send(&mut self, text: &str) {
        self.w.write_all(text.as_bytes()).await.unwrap();
    }
    async fn hello(&mut self) {
        self.send("Last login: never\nrxt-mon 1 Linux web1\n").await;
    }
    /// Next request line; `None` once the collector closed the channel.
    async fn request(&mut self) -> Option<String> {
        let mut line = String::new();
        (self.r.read_line(&mut line).await.unwrap() > 0).then(|| line.trim_end().to_owned())
    }
    async fn reply(&mut self, body: &str) {
        self.send(&format!("@s\n{body}@e\n")).await;
    }
}

fn pair() -> (ExecStream, Peer) {
    let (a, b) = tokio::io::duplex(1 << 20);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    ((Box::new(ar), Box::new(aw)), Peer { r: BufReader::new(br), w: bw })
}

/// Hands out the queued streams; refuses the channel once they are used up.
#[derive(Default)]
struct PeerLink {
    streams: Mutex<Vec<ExecStream>>,
    opens: AtomicUsize,
}

impl Link for PeerLink {
    fn shell<'a>(&'a self, _: &'a Session, _: u16, _: u16) -> BoxFut<'a, Result<ShellPair>> {
        Box::pin(async { Err(Error::Unsupported("no shell".into())) })
    }
    fn file_ops(&self) -> BoxFut<'_, Result<Arc<dyn FileOps>>> {
        Box::pin(async { Err(Error::Unsupported("no sftp".into())) })
    }
    fn start_tunnel<'a>(&'a self, _: &'a Tunnel, _: CancellationToken, _: OnTunnelState) -> BoxFut<'a, Result<()>> {
        Box::pin(async { Err(Error::Unsupported("no tunnel".into())) })
    }
    fn exec_stream<'a>(&'a self, command: &'a str) -> BoxFut<'a, Result<ExecStream>> {
        assert_eq!(command, SCRIPT);
        self.opens.fetch_add(1, Ordering::SeqCst);
        let next = self.streams.lock().pop();
        Box::pin(async move { next.ok_or_else(|| Error::Busy("the server refused an extra channel".into())) })
    }
}

struct Run {
    mon: Arc<Monitor>,
    interval: Arc<AtomicU32>,
    events: mpsc::UnboundedReceiver<Event>,
    task: tokio::task::JoinHandle<()>,
    link: Arc<PeerLink>,
}

fn start(streams: Vec<ExecStream>) -> Run {
    let interval = Arc::new(AtomicU32::new(2));
    let mon = Arc::new(Monitor::new(interval.clone()));
    mon.set_enabled(true);
    let link = Arc::new(PeerLink { streams: Mutex::new(streams), opens: AtomicUsize::new(0) });
    let (tx, events) = mpsc::unbounded_channel();
    let emit: Emit = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let task = tokio::spawn(run(mon.clone(), link.clone(), TabId(1), emit));
    Run { mon, interval, events, task, link }
}

impl Run {
    /// Next `Metrics` or non-`On` monitor state event.
    async fn next(&mut self) -> Event {
        loop {
            match self.events.recv().await.unwrap() {
                Event::Monitor { state: MonitorState::On, .. } => {}
                e => return e,
            }
        }
    }
}

const FRAME: &str = "cpu  1 0 1 8\nMemTotal: 1000 kB\nMemAvailable: 500 kB\n@u 10.0 1.0\n";

#[tokio::test(start_paused = true)]
async fn cadence_pause_until_taken_and_live_interval() {
    let (stream, mut peer) = pair();
    let mut run = start(vec![stream]);
    peer.hello().await;
    assert_eq!(peer.request().await.as_deref(), Some("d"), "first request includes disks");
    peer.reply(FRAME).await;
    assert_eq!(run.next().await, Event::Metrics { tab: TabId(1) });

    // Slot not taken (bar not painted): no request for a minute.
    assert!(timeout(Duration::from_secs(60), peer.request()).await.is_err());
    let s = run.mon.take().unwrap();
    assert_eq!((s.host.as_str(), s.os.as_str(), s.mem), ("web1", "Linux", Some((500 * 1024, 1000 * 1024))));
    // The take wakes the collector: the interval already elapsed, so it asks right away.
    let taken = Instant::now();
    assert_eq!(peer.request().await.as_deref(), Some("d"), "60 s passed: disks are due again");
    assert!(taken.elapsed() < Duration::from_millis(10));
    peer.reply(&FRAME.replace("cpu  1 0 1 8", "cpu  6 0 1 13")).await;
    assert_eq!(run.next().await, Event::Metrics { tab: TabId(1) });
    assert_eq!(run.mon.take().unwrap().cpu, Some(50.0));

    // Taken promptly: the next request follows the interval.
    let t = Instant::now();
    assert_eq!(peer.request().await.as_deref(), Some(""));
    assert_eq!(t.elapsed().as_secs(), 2);
    peer.reply(FRAME).await;
    run.next().await;
    run.interval.store(10, Ordering::Relaxed);
    run.mon.kick();
    run.mon.take();
    let t = Instant::now();
    peer.request().await;
    assert_eq!(t.elapsed().as_secs(), 10, "interval applies without a restart");
}

#[tokio::test(start_paused = true)]
async fn disks_requested_at_most_every_30_s() {
    let (stream, mut peer) = pair();
    let mut run = start(vec![stream]);
    peer.hello().await;
    let t0 = Instant::now();
    let mut with_disks = Vec::new();
    for _ in 0..20 {
        if peer.request().await.unwrap() == "d" {
            with_disks.push(t0.elapsed().as_secs());
        }
        peer.reply(FRAME).await;
        run.next().await;
        run.mon.take();
    }
    assert_eq!(with_disks, [0, 30]);
}

#[tokio::test(start_paused = true)]
async fn toggle_off_closes_channel_and_refused_reopen_is_not_retried() {
    let (stream, mut peer) = pair();
    let mut run = start(vec![stream]);
    peer.hello().await;
    peer.request().await;
    run.mon.set_enabled(false);
    assert_eq!(peer.request().await, None, "remote loop reads EOF");
    assert_eq!(run.next().await, Event::Monitor { tab: TabId(1), state: MonitorState::Off });

    run.mon.set_enabled(true);
    let Event::Monitor { state: MonitorState::Unavailable(reason), .. } = run.next().await else {
        panic!("unavailable expected")
    };
    assert!(reason.contains("refused"), "{reason}");
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(run.link.opens.load(Ordering::SeqCst), 2, "no retry while unavailable");
}

#[tokio::test(start_paused = true)]
async fn three_timeouts_stop_and_close() {
    let (stream, mut peer) = pair();
    let mut run = start(vec![stream]);
    peer.hello().await;
    let t0 = Instant::now();
    for _ in 0..3 {
        assert!(peer.request().await.is_some());
    }
    let Event::Monitor { state: MonitorState::Stopped(_), .. } = run.next().await else { panic!("stopped expected") };
    assert_eq!(t0.elapsed().as_secs(), 30);
    assert_eq!(peer.request().await, None);
}

#[tokio::test(start_paused = true)]
async fn unsupported_header_closes_within_10_s() {
    let (stream, mut peer) = pair();
    let mut run = start(vec![stream]);
    peer.send("This service allows sftp connections only.\n").await;
    let t0 = Instant::now();
    let Event::Monitor { state: MonitorState::Unsupported(_), .. } = run.next().await else {
        panic!("unsupported expected")
    };
    assert_eq!(t0.elapsed().as_secs(), 10);
    assert_eq!(peer.request().await, None);
}

#[tokio::test(start_paused = true)]
async fn dropped_with_the_run_closes_channel() {
    let (stream, mut peer) = pair();
    let run = start(vec![stream]);
    peer.hello().await;
    peer.request().await;
    run.task.abort();
    assert_eq!(peer.request().await, None);
}
