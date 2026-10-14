//! Remote monitoring of SSH tabs (design M1–M5 of `add-remote-monitoring`).
//!
//! One long-lived exec channel per monitored tab runs [`SCRIPT`], a fixed POSIX `sh` loop that
//! blocks in `read` until the engine writes a request line (`\n`, or `d\n` to include `df`),
//! then answers with one frame of raw `/proc` excerpts between `@s` and `@e`. All logic (CPU
//! deltas, memory fallback, default-route interfaces, rates, disk filtering) is in Rust here.
//!
//! Pacing: the collector writes each sample into the tab's latest-sample slot ([`Monitor`]) and
//! requests the next one only when the interval elapsed *and* the UI took the slot (it does so
//! only while painting the bar), so hidden tabs and minimized windows pause collection. Closing
//! the channel (toggle off, disconnect, tab close: the future is dropped) gives the remote
//! `read` an EOF and the loop exits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rxt_core::{Disk, Error, Event, MetricsSample, MonitorState, TabId};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;
use tokio::time::{Instant, timeout};

use crate::transport::Link;

/// The fixed collector command (no interpolation, no secrets); `rxt-mon` marks it in `ps`.
pub(crate) const SCRIPT: &str = r#"exec sh -c 'echo "rxt-mon 1 $(uname -s) $(uname -n)"
while read -r k; do
  echo @s
  if [ -r /proc/stat ]; then
    head -n 1 /proc/stat
    grep -E "^(MemTotal|MemAvailable|MemFree|Buffers|Cached):" /proc/meminfo
    echo "@u $(cat /proc/uptime)"
    echo @r; grep -E "^[^[:space:]]+[[:space:]]+00000000[[:space:]]" /proc/net/route
    echo @n; head -n 130 /proc/net/dev
  fi
  if [ "$k" = d ]; then echo @d; df -Pkl 2>/dev/null || df -Pk; fi
  echo @e
done'"#;

/// Handshake and per-reply limit.
pub(crate) const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// `df` is requested at most this often.
pub(crate) const DISK_EVERY: Duration = Duration::from_secs(30);
const MAX_LINE: usize = 4 << 10;
const MAX_FRAME: usize = 64 << 10;
const MAX_IFACES: usize = 128;
const MAX_DISKS: usize = 8;
/// Consecutive failed replies that stop the collector.
const MAX_FAILURES: u32 = 3;
pub(crate) const DEFAULT_INTERVAL_SECS: u32 = 2;

/// Clamp a refresh interval to 1–30 s.
pub(crate) fn clamp_interval(secs: u32) -> u32 {
    secs.clamp(1, 30)
}

/// Per-tab monitoring control and latest-sample slot, shared by the collector and the UI.
pub(crate) struct Monitor {
    inner: Mutex<Inner>,
    /// Wakes the collector: slot taken, toggled, interval changed.
    wake: Notify,
    interval: Arc<AtomicU32>,
}

#[derive(Default)]
struct Inner {
    enabled: bool,
    sample: Option<MetricsSample>,
}

impl Monitor {
    pub fn new(interval: Arc<AtomicU32>) -> Self {
        Self { inner: Mutex::default(), wake: Notify::new(), interval }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().enabled = enabled;
        self.wake.notify_one();
    }

    pub fn enabled(&self) -> bool {
        self.inner.lock().enabled
    }

    /// Take the latest sample (UI, while painting the bar); wakes the collector.
    pub fn take(&self) -> Option<MetricsSample> {
        let sample = self.inner.lock().sample.take();
        if sample.is_some() {
            self.wake.notify_one();
        }
        sample
    }

    /// Re-read the interval now.
    pub fn kick(&self) {
        self.wake.notify_one();
    }

    fn interval(&self) -> Duration {
        Duration::from_secs(u64::from(clamp_interval(self.interval.load(Ordering::Relaxed))))
    }

    fn slot_empty(&self) -> bool {
        self.inner.lock().sample.is_none()
    }

    /// Store a sample; true when the slot was empty (emit `Event::Metrics`).
    fn put(&self, sample: MetricsSample) -> bool {
        self.inner.lock().sample.replace(sample).is_none()
    }

    fn clear(&self) {
        self.inner.lock().sample = None;
    }
}

pub(crate) type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Collector task of one SSH run; never returns (dropped with the run).
pub(crate) async fn run(mon: Arc<Monitor>, link: Arc<dyn Link>, tab: TabId, emit: Emit) {
    loop {
        if !mon.enabled() {
            emit(Event::Monitor { tab, state: MonitorState::Off });
            while !mon.enabled() {
                mon.wake.notified().await;
            }
        }
        emit(Event::Monitor { tab, state: MonitorState::On });
        let state = collect(&mon, link.as_ref(), tab, &emit).await;
        mon.clear();
        if state == MonitorState::Off {
            continue;
        }
        emit(Event::Monitor { tab, state });
        // Not retried until the user turns monitoring off and on again (or the tab reconnects).
        while mon.enabled() {
            mon.wake.notified().await;
        }
    }
}

/// One collector channel: until toggled off ([`MonitorState::Off`]) or failed.
async fn collect(mon: &Monitor, link: &dyn Link, tab: TabId, emit: &Emit) -> MonitorState {
    let (read, mut write) = match timeout(REPLY_TIMEOUT, link.exec_stream(SCRIPT)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(Error::Unsupported(e))) => return MonitorState::Unsupported(format!("unsupported: {e}")),
        Ok(Err(Error::Busy(e))) => return MonitorState::Unavailable(e),
        Ok(Err(e)) => return MonitorState::Unavailable(format!("the server refused an extra channel: {e}")),
        Err(_) => return MonitorState::Unavailable("the server did not open the monitoring channel".into()),
    };
    let mut reader = BufReader::new(read);
    let Ok(Some((os, host))) = timeout(REPLY_TIMEOUT, handshake(&mut reader)).await else {
        return MonitorState::Unsupported(
            "the server did not answer the monitoring handshake (no POSIX shell, forced command or \
             SFTP-only account)"
                .into(),
        );
    };
    let mut prev: Option<Raw> = None;
    let mut disks: Option<(Vec<Disk>, usize)> = None;
    let mut last_df: Option<Instant> = None;
    let mut last_req: Option<Instant> = None;
    let mut failures = 0;
    loop {
        // Wait until enabled, the slot was taken and the interval elapsed.
        loop {
            if !mon.enabled() {
                return MonitorState::Off;
            }
            if !mon.slot_empty() {
                mon.wake.notified().await;
                continue;
            }
            let due = last_req.map_or_else(Instant::now, |t| t + mon.interval());
            if Instant::now() >= due {
                break;
            }
            tokio::select! {
                () = mon.wake.notified() => {}
                () = tokio::time::sleep_until(due) => {}
            }
        }
        let df = last_df.is_none_or(|t| t.elapsed() >= DISK_EVERY);
        last_req = Some(Instant::now());
        if write.write_all(if df { b"d\n" } else { b"\n" }).await.is_err() || write.flush().await.is_err() {
            return MonitorState::Stopped("the monitoring channel closed".into());
        }
        let reply = tokio::select! {
            r = timeout(REPLY_TIMEOUT, read_frame(&mut reader)) => r,
            () = disabled(mon) => return MonitorState::Off,
        };
        match reply {
            Ok(Reply::Eof) => return MonitorState::Stopped("the monitoring channel closed".into()),
            Ok(Reply::Frame(lines)) => {
                failures = 0;
                let raw = parse_frame(&lines);
                if df {
                    last_df = Some(Instant::now());
                }
                if let Some(d) = &raw.disks {
                    disks = Some(d.clone());
                }
                let sample = compute(&host, &os, prev.as_ref(), &raw, disks.clone());
                prev = Some(raw);
                if mon.put(sample) {
                    emit(Event::Metrics { tab });
                }
            }
            Ok(Reply::TooLarge) | Err(_) => failures += 1,
        }
        if failures >= MAX_FAILURES {
            return MonitorState::Stopped("the server did not answer 3 monitoring requests in a row".into());
        }
    }
}

/// Resolves once monitoring was turned off.
async fn disabled(mon: &Monitor) {
    while mon.enabled() {
        mon.wake.notified().await;
    }
}

/// Wait for `rxt-mon 1 <os> <host>`, skipping login-script noise; `None` on EOF.
async fn handshake<R: AsyncBufRead + Unpin>(r: &mut R) -> Option<(String, String)> {
    let mut line = Vec::new();
    loop {
        if !next_line(r, &mut line).await.ok()?? {
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        if let Some(rest) = text.trim_end().strip_prefix("rxt-mon 1 ") {
            let (os, host) = rest.split_once(' ').unwrap_or((rest, ""));
            return Some((os.to_owned(), host.trim().to_owned()));
        }
    }
}

/// Read one `\n`-terminated line into `line` (without the newline). `Ok(None)`: EOF;
/// `Ok(Some(false))`: the line exceeded [`MAX_LINE`] and was dropped (its bytes are drained).
async fn next_line<R: AsyncBufRead + Unpin>(r: &mut R, line: &mut Vec<u8>) -> std::io::Result<Option<bool>> {
    line.clear();
    let mut over = false;
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok(None);
        }
        let (chunk, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[..i], i + 1),
            None => (buf, buf.len()),
        };
        if !over && line.len() + chunk.len() <= MAX_LINE {
            line.extend_from_slice(chunk);
        } else {
            over = true;
            line.clear();
        }
        let end = done > chunk.len();
        r.consume(done);
        if end {
            return Ok(Some(!over));
        }
    }
}

enum Reply {
    Frame(Vec<String>),
    /// Larger than [`MAX_FRAME`]; drained and discarded.
    TooLarge,
    Eof,
}

/// Read the next `@s` … `@e` frame; lines before `@s` (a late reply's tail) are skipped.
async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R) -> Reply {
    let mut line = Vec::new();
    let mut lines = Vec::new();
    let (mut started, mut bytes, mut too_large) = (false, 0usize, false);
    loop {
        match next_line(r, &mut line).await {
            Ok(Some(true)) => {}
            Ok(Some(false)) => continue,
            Ok(None) | Err(_) => return Reply::Eof,
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches('\r');
        if text == "@s" {
            (started, bytes, too_large) = (true, 0, false);
            lines.clear();
            continue;
        }
        if !started {
            continue;
        }
        if text == "@e" {
            return if too_large { Reply::TooLarge } else { Reply::Frame(lines) };
        }
        bytes += line.len() + 1;
        if bytes > MAX_FRAME {
            too_large = true;
            lines.clear();
        } else {
            lines.push(text.to_owned());
        }
    }
}

/// Counters of one frame.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Raw {
    /// (busy, user + nice + system, total) jiffies.
    cpu: Option<(u64, u64, u64)>,
    /// Used and total bytes.
    mem: Option<(u64, u64)>,
    /// Seconds since boot (the server's clock for rates).
    uptime: Option<f64>,
    /// Interfaces of the default route(s).
    routes: Vec<String>,
    /// (interface, rx bytes, tx bytes).
    net: Vec<(String, u64, u64)>,
    /// Kept disks and the number left out by the cap; `None` when the frame had no `df`.
    disks: Option<(Vec<Disk>, usize)>,
}

/// Parse one frame (see [`SCRIPT`]); unparsable lines are skipped.
pub(crate) fn parse_frame(lines: &[String]) -> Raw {
    let mut raw = Raw::default();
    let mut section = "";
    let (mut total, mut avail, mut free, mut buffers, mut cached) = (None, None, None, None, None);
    let mut rows: Option<Vec<Disk>> = None;
    for l in lines {
        if let Some(rest) = l.strip_prefix("@u ") {
            raw.uptime = rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
            continue;
        }
        match l.as_str() {
            "@r" | "@n" => section = l.as_str(),
            "@d" => {
                section = "@d";
                rows.get_or_insert_with(Vec::new);
            }
            _ => match section {
                "@r" => {
                    let mut f = l.split_whitespace();
                    if let (Some(iface), Some("00000000")) = (f.next(), f.next())
                        && !raw.routes.iter().any(|r| r == iface)
                    {
                        raw.routes.push(iface.to_owned());
                    }
                }
                "@n" => {
                    if raw.net.len() < MAX_IFACES
                        && let Some((name, rest)) = l.split_once(':')
                    {
                        let f: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                        if f.len() >= 9 {
                            raw.net.push((name.trim().to_owned(), f[0], f[8]));
                        }
                    }
                }
                "@d" => {
                    if let (Some(rows), Some(d)) = (rows.as_mut(), df_row(l)) {
                        rows.push(d);
                    }
                }
                _ => {
                    if let Some(rest) = l.strip_prefix("cpu ") {
                        raw.cpu = cpu_line(rest);
                    } else if let Some((k, v)) = l.split_once(':') {
                        let kib = v.split_whitespace().next().and_then(|n| n.parse::<u64>().ok());
                        match k {
                            "MemTotal" => total = kib,
                            "MemAvailable" => avail = kib,
                            "MemFree" => free = kib,
                            "Buffers" => buffers = kib,
                            "Cached" => cached = kib,
                            _ => {}
                        }
                    }
                }
            },
        }
    }
    // Kernels before 3.14 have no MemAvailable.
    let avail = avail.or_else(|| Some(free? + buffers.unwrap_or(0) + cached.unwrap_or(0)));
    raw.mem = total.map(|t| (t.saturating_sub(avail.unwrap_or(t)) * 1024, t * 1024));
    raw.disks = rows.map(filter_disks);
    raw
}

/// `cpu` line fields after the label: (busy, user + nice + system, total).
fn cpu_line(rest: &str) -> Option<(u64, u64, u64)> {
    let f: Vec<u64> = rest.split_whitespace().map_while(|v| v.parse().ok()).collect();
    if f.len() < 4 {
        return None;
    }
    // guest/guest_nice are already part of user/nice.
    let total: u64 = f.iter().take(8).sum();
    let idle = f[3] + f.get(4).copied().unwrap_or(0);
    Some((total.saturating_sub(idle), f[0] + f[1] + f[2], total))
}

/// One `df -P` row: `source blocks used available capacity mount…` (1 KiB blocks).
fn df_row(l: &str) -> Option<Disk> {
    let f: Vec<&str> = l.split_whitespace().collect();
    if f.len() < 6 {
        return None;
    }
    let used: u64 = f[2].parse().ok()?;
    let avail: u64 = f[3].parse().ok()?;
    let _blocks: u64 = f[1].parse().ok()?;
    let size = used + avail;
    let percent = if size == 0 { 0 } else { u8::try_from((used * 100).div_ceil(size)).unwrap_or(100) };
    Some(Disk { source: f[0].to_owned(), mount: f[5..].join(" "), used_kib: used, size_kib: size, percent })
}

/// Keep `/` and `/dev/` devices (not loop), one per source (shortest mount), `/` first, capped.
fn filter_disks(rows: Vec<Disk>) -> (Vec<Disk>, usize) {
    let mut kept: Vec<Disk> = Vec::new();
    for d in rows {
        let local = d.mount == "/" || (d.source.starts_with("/dev/") && !d.source.starts_with("/dev/loop"));
        if !local {
            continue;
        }
        match kept.iter_mut().find(|k| k.source == d.source) {
            Some(k) if d.mount.len() < k.mount.len() => *k = d,
            Some(_) => {}
            None => kept.push(d),
        }
    }
    kept.sort_by(|a, b| (a.mount != "/", &a.mount).cmp(&(b.mount != "/", &b.mount)));
    let more = kept.len().saturating_sub(MAX_DISKS);
    kept.truncate(MAX_DISKS);
    (kept, more)
}

/// Sample from the current frame and the previous one (CPU and network deltas).
pub(crate) fn compute(
    host: &str,
    os: &str,
    prev: Option<&Raw>,
    cur: &Raw,
    disks: Option<(Vec<Disk>, usize)>,
) -> MetricsSample {
    let mut s = MetricsSample {
        host: host.to_owned(),
        os: os.to_owned(),
        mem: cur.mem,
        uptime_secs: cur.uptime.map(|u| u as u64),
        ..MetricsSample::default()
    };
    if let Some((d, more)) = disks {
        s.disks = Some(d);
        s.disks_more = more;
    }
    // ponytail: default-route interfaces only (all but `lo` without one); per-interface choice
    // if users ask for multi-homed servers.
    let ifaces: Vec<&str> = if cur.routes.iter().any(|r| cur.net.iter().any(|n| n.0 == *r)) {
        cur.routes.iter().map(String::as_str).collect()
    } else {
        cur.net.iter().map(|n| n.0.as_str()).filter(|n| *n != "lo").collect()
    };
    s.net_ifaces = ifaces.iter().map(|i| (*i).to_owned()).collect();
    let Some(prev) = prev else { return s };
    if let (Some((b0, u0, t0)), Some((b1, u1, t1))) = (prev.cpu, cur.cpu)
        && t1 > t0
        && b1 >= b0
        && u1 >= u0
    {
        let pct = |d: u64| (d as f64 / (t1 - t0) as f64 * 100.0).clamp(0.0, 100.0) as f32;
        s.cpu = Some(pct(b1 - b0));
        s.cpu_user_system = Some(pct(u1 - u0));
    }
    if let (Some(up0), Some(up1)) = (prev.uptime, cur.uptime)
        && up1 > up0
    {
        let (mut rx, mut tx, mut ok) = (0u64, 0u64, !ifaces.is_empty());
        for i in &ifaces {
            let find = |r: &Raw| r.net.iter().find(|n| n.0 == *i).map(|n| (n.1, n.2));
            match (find(prev), find(cur)) {
                // A counter that went backwards (reset, 32-bit wrap) skips the sample.
                (Some((r0, t0)), Some((r1, t1))) if r1 >= r0 && t1 >= t0 => {
                    rx += r1 - r0;
                    tx += t1 - t0;
                }
                _ => ok = false,
            }
        }
        if ok {
            let dt = up1 - up0;
            s.net = Some(((tx as f64 * 8.0 / dt) as u64, (rx as f64 * 8.0 / dt) as u64));
        }
    }
    s
}

#[cfg(test)]
mod tests;
