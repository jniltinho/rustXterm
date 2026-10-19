//! Session data-stream capture (add-diagnostic-logging D3). A [`Tap`] per connection records
//! traffic after authentication into `<dir>/<session>-<yyyymmdd-hhmmss>-<n>.log` through one
//! background [`Sink`]: received bytes always, sent bytes only with "include typed input", and for
//! RDP/VNC only event types and sizes. Off at every start; the UI turns it on with [`configure`].

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use rxt_core::{DesktopEvent, DesktopInput, Session};
use rxt_store::logfile::Sink;

/// What [`configure`] turns on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamConfig {
    /// Folder of the stream files (created on demand).
    pub dir: PathBuf,
    /// Also record what the user sends (may contain passwords typed with echo off).
    pub include_input: bool,
}

struct Capture {
    cfg: StreamConfig,
    sink: Sink,
    /// Keys with an open file, closed when capture stops or changes.
    open: Mutex<HashSet<u64>>,
}

static CAPTURE: RwLock<Option<Capture>> = RwLock::new(None);
/// Bumped by [`configure`]: a tap opened under an older epoch reopens (new folder) or stops.
static EPOCH: AtomicU64 = AtomicU64::new(1);
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);
static SINK: OnceLock<Option<Sink>> = OnceLock::new();

/// Starts (`Some`) or stops (`None`) stream capture for every connection, including running ones.
/// Returns the writer's last error, if any.
pub fn configure(cfg: Option<StreamConfig>) -> Option<String> {
    // The folder is prepared before taking the lock, so a slow disk never stalls the taps.
    if let Some(c) = &cfg
        && let Err(e) = std::fs::create_dir_all(&c.dir)
    {
        return Some(format!("cannot create {}: {e}", c.dir.display()));
    }
    let mut slot = CAPTURE.write();
    // Closing only queues the request: nothing here waits for the disk.
    if let Some(old) = slot.take() {
        for key in old.open.lock().drain() {
            old.sink.close(key);
        }
    }
    EPOCH.fetch_add(1, Ordering::AcqRel);
    let cfg = cfg?;
    let sink = SINK.get_or_init(|| Sink::start("rustxterm-stream").ok()).clone();
    let Some(sink) = sink else { return Some("the stream writer did not start".into()) };
    let error = sink.take_error();
    *slot = Some(Capture { cfg, sink, open: Mutex::default() });
    error
}

/// Waits until every record queued so far is on disk (tests, and the end of the application).
pub fn flush() {
    if let Some(sink) = SINK.get().and_then(Option::as_ref) {
        sink.flush();
    }
}

/// Stops capture and writes out what is queued (application exit).
pub fn shutdown() {
    configure(None);
    flush();
}

/// Whether capture is on (status-bar chip).
pub fn active() -> bool {
    CAPTURE.read().is_some()
}

/// Whether typed input is recorded.
pub fn includes_input() -> bool {
    CAPTURE.read().as_ref().is_some_and(|c| c.cfg.include_input)
}

/// The writer's last error, cleared by reading it.
pub fn take_error() -> Option<String> {
    CAPTURE.read().as_ref().and_then(|c| c.sink.take_error())
}

/// One connection's stream file, opened on its first record while capture is on.
pub(crate) struct Tap {
    key: u64,
    name: String,
    /// Epoch the file was opened under; held while opening and queueing, so a concurrent record
    /// (RDP input and output run on different tasks) is never queued before its file's `Open`.
    opened: Mutex<u64>,
}

impl Tap {
    pub fn new(session: &Session) -> Self {
        let name: String = session
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
            .take(48)
            .collect();
        let name = if name.is_empty() { format!("{:?}", session.protocol).to_lowercase() } else { name };
        Self { key: NEXT_KEY.fetch_add(1, Ordering::Relaxed), name, opened: Mutex::new(0) }
    }

    /// Records the server's bytes.
    pub fn received(&self, bytes: &[u8]) {
        self.record(|out, _| {
            dump(out, "RX", bytes);
            true
        });
    }

    /// Records the user's bytes when typed input is included.
    pub fn sent(&self, bytes: &[u8]) {
        self.record(|out, with_input| {
            with_input && {
                dump(out, "TX", bytes);
                true
            }
        });
    }

    /// RDP/VNC: event type and size only (no pixels, no clipboard text).
    pub fn desktop_event(&self, event: &DesktopEvent) {
        let what = match event {
            DesktopEvent::Resized { width, height } => format!("Resized {width}x{height}"),
            DesktopEvent::Update { rect, rgba } => {
                format!("Update {}x{} at {},{} ({} bytes)", rect.width, rect.height, rect.x, rect.y, rgba.len())
            }
            DesktopEvent::Copy { dst, .. } => format!("Copy {}x{} to {},{}", dst.width, dst.height, dst.x, dst.y),
            DesktopEvent::Clipboard(text) => format!("Clipboard {} chars", text.chars().count()),
            DesktopEvent::Bell => "Bell".into(),
            DesktopEvent::Cursor(_) => "Cursor".into(),
        };
        self.record(|out, _| {
            line(out, "RX", &what);
            true
        });
    }

    /// RDP/VNC input: the kind only; keys only when typed input is included; clipboard text never
    /// (its length only).
    pub fn desktop_input(&self, input: &DesktopInput) {
        self.record(|out, with_input| {
            let what = match input {
                DesktopInput::Clipboard(text) => format!("Clipboard {} chars", text.chars().count()),
                DesktopInput::Key { .. } if with_input => format!("{input:?}"),
                DesktopInput::Key { down, .. } => format!("Key {}", if *down { "down" } else { "up" }),
                // Pointer moves would bury everything else; recorded with typed input only.
                DesktopInput::Pointer { .. } | DesktopInput::Wheel { .. } if !with_input => return false,
                other => format!("{other:?}"),
            };
            line(out, "TX", &what);
            true
        });
    }

    /// `f` formats the record, given whether typed input is included (read under the same lock as
    /// the write, so a switch in between cannot leak input), and returns false to skip it.
    fn record(&self, f: impl FnOnce(&mut String, bool) -> bool) {
        let guard = CAPTURE.read();
        let Some(c) = guard.as_ref() else { return };
        let mut out = String::new();
        if !f(&mut out, c.cfg.include_input) {
            return;
        }
        let epoch = EPOCH.load(Ordering::Acquire);
        let mut opened = self.opened.lock();
        if std::mem::replace(&mut *opened, epoch) != epoch {
            let file = c.cfg.dir.join(format!("{}-{}-{}.log", self.name, file_stamp(), self.key));
            c.sink.open(self.key, file);
            c.open.lock().insert(self.key);
            let mut head = String::new();
            let _ = writeln!(
                head,
                "# rustXterm data stream: {} (typed input {})",
                self.name,
                if c.cfg.include_input { "included" } else { "not recorded" }
            );
            c.sink.write(self.key, head.into_bytes());
        }
        c.sink.write(self.key, out.into_bytes());
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        if let Some(c) = CAPTURE.read().as_ref()
            && c.open.lock().remove(&self.key)
        {
            c.sink.close(self.key);
        }
    }
}

fn line(out: &mut String, dir: &str, what: &str) {
    let _ = writeln!(out, "{} {dir} {what}", timestamp());
}

/// `time dir N bytes`, then 16 bytes per line in hex and printable ASCII.
fn dump(out: &mut String, dir: &str, bytes: &[u8]) {
    line(out, dir, &format!("{} bytes", bytes.len()));
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let _ = write!(out, "  {:04x}  ", i * 16);
        for b in chunk {
            let _ = write!(out, "{b:02x} ");
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push_str(" |");
        out.extend(chunk.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }));
        out.push_str("|\n");
    }
}

/// UTC civil time from the Unix clock (days → y/m/d per H. Hinnant's algorithm).
fn civil() -> (i64, u32, u32, u64, u32) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let z = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, secs % 86_400, now.subsec_millis())
}

fn timestamp() -> String {
    let (y, m, d, s, ms) = civil();
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z", s / 3600, s / 60 % 60, s % 60)
}

fn file_stamp() -> String {
    let (y, m, d, s, _) = civil();
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Capture is process-wide: tests that turn it on run one at a time.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_shows_hex_and_text() {
        let mut out = String::new();
        dump(&mut out, "RX", b"hello\r\n");
        let body: Vec<&str> = out.lines().collect();
        assert!(body[0].ends_with("RX 7 bytes"), "{out}");
        assert!(body[1].starts_with("  0000  68 65 6c 6c 6f 0d 0a "), "{out}");
        assert!(body[1].ends_with("|hello..|"), "{out}");
    }

    #[test]
    fn capture_records_received_bytes_and_input_only_when_asked() {
        let _one = TEST_LOCK.lock();
        let dir = std::env::temp_dir().join(format!("rxt-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = Session::new(rxt_core::Protocol::Ssh, "web1", None, None);
        let read = |dir: &std::path::Path| -> String {
            std::fs::read_dir(dir).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect()
        };
        assert!(configure(Some(StreamConfig { dir: dir.clone(), include_input: false })).is_none());
        let tap = Tap::new(&session);
        tap.received(b"login ok\r\n");
        tap.sent(b"s3cret-typed\r");
        configure(None);
        flush();
        let text = read(&dir);
        assert!(text.contains("|login ok..|") && text.contains("typed input not recorded"), "{text}");
        assert!(!text.contains("s3cret") && !text.contains("73 33 63 72 65 74"), "{text}");
        tap.received(b"after off");
        assert!(!read(&dir).contains("after off"));

        std::fs::remove_dir_all(&dir).unwrap();
        configure(Some(StreamConfig { dir: dir.clone(), include_input: true }));
        tap.sent(b"ls\r");
        tap.desktop_input(&DesktopInput::Clipboard("clip-s3cret".into()));
        configure(None);
        flush();
        let text = read(&dir);
        assert!(text.contains("TX 3 bytes") && text.contains("TX Clipboard 11 chars"), "{text}");
        assert!(!text.contains("clip-s3cret"), "clipboard text is never recorded: {text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn timestamp_is_rfc3339_utc() {
        let t = timestamp();
        assert_eq!(t.len(), 24, "{t}");
        assert!(t.ends_with('Z') && t.as_bytes()[10] == b'T', "{t}");
        assert!(t.starts_with("20"), "{t}");
    }
}
