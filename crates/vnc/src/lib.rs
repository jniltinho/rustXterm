//! VNC client on vnc-rs with wrapper limits (design D13).
//!
//! Contract used by `engine` (keep stable; extend, don't break): [`connect`] returns a
//! [`Desktop`]; the engine pulls [`Desktop::next`] (bounded, so slow consumers apply
//! backpressure) and sends input through [`InputSender`]. Dropping `Desktop` cancels it.
//!
//! Limits: vnc-rs rejects framebuffers and rectangles above 8192 px per edge or 8 294 400
//! pixels (so a decoded rectangle is at most ~32 MiB) and compressed payloads above 64 MiB;
//! the vendored patch (`vendor/vnc-rs`) adds the 30 s mid-message stall timeout and bounds
//! buffered network data to 4 MiB; this crate bounds queued events to [`MAX_QUEUE_BYTES`].

use std::collections::HashSet;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use rxt_core::desktop::{MAX_CURSOR_EDGE, RemoteCursor};
use rxt_core::{
    CertTrust, DesktopEvent, DesktopInput, Error, PromptAnswer, PromptKind, Prompter, Rect, Result, Session, VncOptions,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::{CancellationToken, DropGuard};
use vnc::{ClientKeyEvent, ClientMouseEvent, PixelFormat, VncClient, VncConnector, VncError, VncEvent, X11Event};

pub use vnc::VncEncoding;
pub use vnc::client::stall::STALL_TIMEOUT;

/// Bytes of decoded events queued between the session task and [`Desktop::next`].
pub const MAX_QUEUE_BYTES: usize = 64 << 20;
/// Encodings offered by [`connect`], in preference order (Raw is always accepted).
pub const DEFAULT_ENCODINGS: [VncEncoding; 4] =
    [VncEncoding::Zrle, VncEncoding::Tight, VncEncoding::CopyRect, VncEncoding::Raw];
// ponytail: fixed-rate incremental requests (vnc-rs has no end-of-update event); servers merge
// pending requests, so this costs ~300 B/s while visible.
const REFRESH: Duration = Duration::from_millis(33);
const INPUT_QUEUE: usize = 1024;
/// vnc-rs refuses cut text above 1 MiB.
const MAX_CLIPBOARD: usize = 1 << 20;
/// Minimum queue charge per event, so small events cannot pile up unbounded.
const EVENT_OVERHEAD: usize = 64;

/// Inputs for [`connect`].
#[derive(Clone)]
pub struct Context {
    pub prompter: Arc<dyn Prompter>,
    pub trust: Arc<dyn CertTrust>,
    pub cancel: CancellationToken,
}

/// Any byte stream (TCP or an SSH direct-tcpip channel).
pub trait Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Stream for T {}

type Queued = (Result<DesktopEvent>, OwnedSemaphorePermit);

/// A connected remote desktop.
pub struct Desktop {
    events: mpsc::UnboundedReceiver<Queued>,
    input: InputSender,
    _cancel: DropGuard,
}

/// Cloneable input handle; never blocks.
#[derive(Clone)]
pub struct InputSender {
    tx: mpsc::Sender<DesktopInput>,
}

/// `stream`: pre-connected transport (VNC over SSH); `None` connects TCP to `session.host:port`.
pub async fn connect(session: &Session, ctx: Context, stream: Option<Box<dyn Stream>>) -> Result<Desktop> {
    connect_with(session, ctx, stream, &DEFAULT_ENCODINGS).await
}

/// [`connect`] offering only `encodings` (plus DesktopSize); used by tests to force one encoding.
pub async fn connect_with(
    session: &Session,
    ctx: Context,
    stream: Option<Box<dyn Stream>>,
    encodings: &[VncEncoding],
) -> Result<Desktop> {
    let cancel = ctx.cancel.child_token();
    let client = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        client = handshake(session, &ctx, stream, encodings) => client?,
    };
    let (queue, events) = Queue::new(MAX_QUEUE_BYTES);
    let (tx, input) = mpsc::channel(INPUT_QUEUE);
    let opts = session.vnc.clone();
    let token = cancel.clone();
    tokio::spawn(async move {
        let result = tokio::select! {
            _ = token.cancelled() => Ok(()),
            result = run(&client, &opts, &queue, input) => result,
        };
        if let Err(e) = result {
            tokio::select! {
                _ = token.cancelled() => {}
                _ = queue.push(Err(e)) => {}
            }
        }
        let _ = client.close().await;
    });
    Ok(Desktop { events, input: InputSender { tx }, _cancel: cancel.drop_guard() })
}

impl Desktop {
    /// Next event; `None` when the session ended.
    pub async fn next(&mut self) -> Option<Result<DesktopEvent>> {
        self.events.recv().await.map(|(event, _permit)| event)
    }

    pub fn input(&self) -> InputSender {
        self.input.clone()
    }
}

impl InputSender {
    pub fn send(&self, input: DesktopInput) -> Result<()> {
        self.tx.try_send(input).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => Error::Io("VNC input queue is full".into()),
            mpsc::error::TrySendError::Closed(_) => Error::Connection("VNC session closed".into()),
        })
    }
}

async fn handshake(
    session: &Session,
    ctx: &Context,
    stream: Option<Box<dyn Stream>>,
    encodings: &[VncEncoding],
) -> Result<VncClient> {
    let stream: Box<dyn Stream> = match stream {
        Some(s) => s,
        None => Box::new(
            tokio::net::TcpStream::connect((session.host.as_str(), session.port))
                .await
                .map_err(|e| Error::Connection(format!("{}:{}: {e}", session.host, session.port)))?,
        ),
    };
    let prompted = Arc::new(AtomicBool::new(false));
    let (flag, prompter, host) = (prompted.clone(), ctx.prompter.clone(), session.host.clone());
    // vnc-rs needs a `Sync` future: run the prompt on its own task and await the handle.
    let password = async move {
        let answer = tokio::spawn(async move {
            prompter.prompt(PromptKind::Password { user: String::new(), host, retry: false }).await
        })
        .await
        .map_err(|_| VncError::NoPassword)?;
        flag.store(true, Ordering::Relaxed);
        match answer {
            PromptAnswer::Secret(s) => Ok(s.0),
            _ => Err(VncError::NoPassword),
        }
    };
    let mut connector = VncConnector::new(SyncStream(std::sync::Mutex::new(stream)))
        .set_auth_method(password)
        .allow_shared(true)
        .set_pixel_format(PixelFormat::rgba());
    // CursorPseudo: the UI draws the server cursor at the local mouse position (no round trip).
    for e in encodings.iter().chain(&[VncEncoding::DesktopSizePseudo, VncEncoding::CursorPseudo]) {
        connector = connector.add_encoding(*e);
    }
    let started = async { connector.build()?.try_start().await?.finish() }.await;
    started.map_err(|e| match e {
        VncError::General(m) if prompted.load(Ordering::Relaxed) => Error::Auth(m),
        e => vnc_err(e),
    })
}

fn vnc_err(e: VncError) -> Error {
    match e {
        VncError::NoPassword => Error::Cancelled,
        VncError::WrongPassword => Error::Auth("wrong VNC password".into()),
        VncError::IoError(e) => e.into(),
        e => Error::Protocol(e.to_string()),
    }
}

async fn run(
    client: &VncClient,
    opts: &VncOptions,
    queue: &Queue,
    mut input: mpsc::Receiver<DesktopInput>,
) -> Result<()> {
    let mut visible = true;
    let mut state = InputState::default();
    let mut tick = tokio::time::interval(REFRESH);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // Input errors are ignored: a dead connection surfaces through `recv_event`.
        tokio::select! {
            event = client.recv_event() => match event {
                Ok(event) => {
                    if let Some(event) = convert(event, opts)? {
                        queue.push(Ok(event)).await?;
                    }
                }
                Err(VncError::ClientNotRunning) => return Ok(()),
                Err(e) => return Err(vnc_err(e)),
            },
            Some(i) = input.recv() => {
                let messages = match i {
                    DesktopInput::Visible(v) => {
                        visible = v;
                        if v { vec![X11Event::FullRefresh] } else { Vec::new() }
                    }
                    other => map_input(other, opts, &mut state),
                };
                for m in messages {
                    let _ = client.input(m).await;
                }
            }
            _ = tick.tick(), if visible => {
                let _ = client.input(X11Event::Refresh).await;
            }
        }
    }
}

fn rect(r: vnc::Rect) -> Rect {
    Rect { x: r.x.into(), y: r.y.into(), width: r.width.into(), height: r.height.into() }
}

/// Cursor pseudo-rectangle (x/y = hotspot, RGBA with mask alpha) → cursor shape; empty hides
/// the pointer, oversized or malformed shapes fall back to the local arrow.
fn cursor(r: vnc::Rect, rgba: Vec<u8>) -> RemoteCursor {
    let (width, height) = (u32::from(r.width), u32::from(r.height));
    if width == 0 || height == 0 {
        return RemoteCursor::Hidden;
    }
    if width > MAX_CURSOR_EDGE || height > MAX_CURSOR_EDGE || rgba.len() as u64 != u64::from(width * height) * 4 {
        return RemoteCursor::Default;
    }
    RemoteCursor::Bitmap {
        width,
        height,
        hot_x: u32::from(r.x).min(width - 1),
        hot_y: u32::from(r.y).min(height - 1),
        rgba,
    }
}

fn convert(event: VncEvent, opts: &VncOptions) -> Result<Option<DesktopEvent>> {
    Ok(match event {
        VncEvent::SetResolution(s) => Some(DesktopEvent::Resized { width: s.width.into(), height: s.height.into() }),
        VncEvent::RawImage(r, mut rgba) => {
            let rect = rect(r);
            if rgba.len() as u64 != rect.area() * 4 {
                return Err(Error::Protocol("decoded rectangle has the wrong size".into()));
            }
            // The pixel format has no alpha channel: padding bytes are arbitrary.
            rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 0xff);
            Some(DesktopEvent::Update { rect, rgba })
        }
        VncEvent::Copy(dst, src) => {
            Some(DesktopEvent::Copy { src_x: src.x.into(), src_y: src.y.into(), dst: rect(dst) })
        }
        VncEvent::JpegImage(..) => {
            return Err(Error::Protocol("server sent Tight JPEG, which was not negotiated".into()));
        }
        VncEvent::Bell => Some(DesktopEvent::Bell),
        VncEvent::SetCursor(r, rgba) => Some(DesktopEvent::Cursor(cursor(r, rgba))),
        VncEvent::Text(t) if opts.clipboard => Some(DesktopEvent::Clipboard(t)),
        VncEvent::Error(m) => return Err(Error::Protocol(m)),
        _ => None,
    })
}

#[derive(Default)]
struct InputState {
    keys: HashSet<u32>,
    buttons: u8,
    x: u16,
    y: u16,
}

fn key(keycode: u32, down: bool) -> X11Event {
    X11Event::KeyEvent(ClientKeyEvent { keycode, down })
}

fn pointer(st: &InputState, buttons: u8) -> X11Event {
    X11Event::PointerEvent(ClientMouseEvent { position_x: st.x, position_y: st.y, bottons: buttons })
}

fn clamp(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// RFB messages for one input; empty in view-only mode.
fn map_input(input: DesktopInput, opts: &VncOptions, st: &mut InputState) -> Vec<X11Event> {
    if opts.view_only {
        return Vec::new();
    }
    match input {
        DesktopInput::Key { keysym, down } => {
            if down {
                st.keys.insert(keysym);
            } else {
                st.keys.remove(&keysym);
            }
            vec![key(keysym, down)]
        }
        DesktopInput::Pointer { x, y, buttons } => {
            (st.x, st.y, st.buttons) = (clamp(x), clamp(y), buttons & 0x07);
            vec![pointer(st, st.buttons)]
        }
        DesktopInput::Wheel { x, y, dx, dy } => {
            (st.x, st.y) = (clamp(x), clamp(y));
            // RFB buttons 4/5 scroll up/down, 6/7 left/right; one press+release per step.
            let mut out = Vec::new();
            for (steps, positive, negative) in [(dy, 3, 4), (dx, 6, 5)] {
                let bit = if steps > 0 { positive } else { negative };
                for _ in 0..steps.unsigned_abs().min(32) {
                    out.push(pointer(st, st.buttons | 1 << bit));
                    out.push(pointer(st, st.buttons));
                }
            }
            out
        }
        // ponytail: classic cut text only carries Latin-1 and vnc-rs sends UTF-8 bytes, so
        // non-ASCII is replaced; add the Extended Clipboard pseudo-encoding for full Unicode.
        DesktopInput::Clipboard(text) if opts.clipboard => {
            let text: String = text.chars().take(MAX_CLIPBOARD).map(|c| if c.is_ascii() { c } else { '?' }).collect();
            vec![X11Event::CopyText(text)]
        }
        DesktopInput::ReleaseAll => {
            let mut out: Vec<_> = st.keys.drain().map(|k| key(k, false)).collect();
            if st.buttons != 0 {
                st.buttons = 0;
                out.push(pointer(st, 0));
            }
            out
        }
        DesktopInput::Clipboard(_) | DesktopInput::Visible(_) | DesktopInput::Resize { .. } => Vec::new(),
    }
}

/// Event queue bounded by bytes: each event holds permits until [`Desktop::next`] takes it.
struct Queue {
    tx: mpsc::UnboundedSender<Queued>,
    budget: Arc<Semaphore>,
    cap: usize,
}

impl Queue {
    fn new(cap: usize) -> (Self, mpsc::UnboundedReceiver<Queued>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, budget: Arc::new(Semaphore::new(cap)), cap }, rx)
    }

    async fn push(&self, event: Result<DesktopEvent>) -> Result<()> {
        let cost = match &event {
            Ok(DesktopEvent::Update { rgba, .. }) => rgba.len(),
            Ok(DesktopEvent::Clipboard(t)) => t.len(),
            Ok(DesktopEvent::Cursor(RemoteCursor::Bitmap { rgba, .. })) => rgba.len(),
            _ => 0,
        }
        .max(EVENT_OVERHEAD);
        let permits = u32::try_from(cost)
            .ok()
            .filter(|_| cost <= self.cap)
            .ok_or_else(|| Error::Protocol(format!("{cost}-byte update exceeds the {}-byte queue", self.cap)))?;
        let permit = self.budget.clone().acquire_many_owned(permits).await.map_err(|_| Error::Cancelled)?;
        self.tx.send((event, permit)).map_err(|_| Error::Cancelled)
    }
}

/// vnc-rs requires a `Sync` stream; the mutex is only reached through `&mut`, never locked.
struct SyncStream(std::sync::Mutex<Box<dyn Stream>>);

impl SyncStream {
    fn inner(self: Pin<&mut Self>) -> Pin<&mut Box<dyn Stream>> {
        Pin::new(self.get_mut().0.get_mut().unwrap_or_else(PoisonError::into_inner))
    }
}

impl AsyncRead for SyncStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_read(cx, buf)
    }
}

impl AsyncWrite for SyncStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.inner().poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(bytes: usize) -> Result<DesktopEvent> {
        Ok(DesktopEvent::Update { rect: Rect::default(), rgba: vec![0; bytes] })
    }

    #[tokio::test]
    async fn queue_blocks_at_byte_cap_until_consumed() {
        let (queue, mut rx) = Queue::new(1000);
        queue.push(update(800)).await.unwrap();
        let blocked = tokio::time::timeout(Duration::from_millis(50), queue.push(update(800))).await;
        assert!(blocked.is_err(), "second push must wait for budget");
        drop(rx.recv().await.unwrap());
        tokio::time::timeout(Duration::from_secs(1), queue.push(update(800))).await.unwrap().unwrap();
        assert!(queue.push(update(1001)).await.is_err(), "events larger than the cap are refused");
    }

    fn all_inputs() -> Vec<DesktopInput> {
        vec![
            DesktopInput::Key { keysym: 0x61, down: true },
            DesktopInput::Pointer { x: 1, y: 2, buttons: 1 },
            DesktopInput::Wheel { x: 1, y: 2, dx: 1, dy: -1 },
            DesktopInput::Clipboard("x".into()),
            DesktopInput::ReleaseAll,
        ]
    }

    #[test]
    fn view_only_suppresses_all_input() {
        let opts = VncOptions { view_only: true, clipboard: true, ..VncOptions::default() };
        let mut st = InputState::default();
        for i in all_inputs() {
            assert!(map_input(i, &opts, &mut st).is_empty());
        }
    }

    #[test]
    fn input_mapping_tracks_keys_buttons_and_clipboard_toggle() {
        let mut st = InputState::default();
        let opts = VncOptions { clipboard: false, ..VncOptions::default() };
        assert_eq!(map_input(DesktopInput::Clipboard("x".into()), &opts, &mut st).len(), 0);
        let opts = VncOptions { clipboard: true, ..opts };
        let out = map_input(DesktopInput::Clipboard("é!".into()), &opts, &mut st);
        assert!(matches!(&out[..], [X11Event::CopyText(t)] if t == "?!"));
        map_input(DesktopInput::Key { keysym: 0x61, down: true }, &opts, &mut st);
        map_input(DesktopInput::Pointer { x: 70_000, y: 5, buttons: 0xff }, &opts, &mut st);
        assert_eq!((st.x, st.buttons), (u16::MAX, 0x07));
        assert_eq!(map_input(DesktopInput::Wheel { x: 1, y: 1, dx: 0, dy: 2 }, &opts, &mut st).len(), 4);
        let out = map_input(DesktopInput::ReleaseAll, &opts, &mut st);
        assert!(matches!(&out[..], [X11Event::KeyEvent(k), X11Event::PointerEvent(p)] if !k.down && p.bottons == 0));
    }

    #[test]
    fn convert_forces_alpha_and_checks_sizes() {
        let opts = VncOptions { clipboard: false, ..VncOptions::default() };
        let r = vnc::Rect { x: 1, y: 2, width: 2, height: 1 };
        let ev = convert(VncEvent::RawImage(r, vec![1, 2, 3, 0, 4, 5, 6, 0]), &opts).unwrap();
        assert_eq!(
            ev,
            Some(DesktopEvent::Update {
                rect: Rect { x: 1, y: 2, width: 2, height: 1 },
                rgba: vec![1, 2, 3, 255, 4, 5, 6, 255]
            })
        );
        assert!(convert(VncEvent::RawImage(r, vec![0; 7]), &opts).is_err());
        assert!(convert(VncEvent::JpegImage(r, vec![]), &opts).is_err());
        assert_eq!(convert(VncEvent::Text("t".into()), &opts).unwrap(), None);
        let opts = VncOptions { clipboard: true, ..opts };
        assert_eq!(convert(VncEvent::Text("t".into()), &opts).unwrap(), Some(DesktopEvent::Clipboard("t".into())));
    }

    #[test]
    fn cursor_pseudo_rect_maps_to_cursor() {
        let r = |x, y, width, height| vnc::Rect { x, y, width, height };
        assert_eq!(
            cursor(r(9, 1, 2, 1), vec![7; 8]),
            RemoteCursor::Bitmap { width: 2, height: 1, hot_x: 1, hot_y: 0, rgba: vec![7; 8] }
        );
        assert_eq!(cursor(r(0, 0, 0, 0), vec![]), RemoteCursor::Hidden);
        assert_eq!(cursor(r(0, 0, 2, 1), vec![7; 7]), RemoteCursor::Default);
        assert_eq!(cursor(r(0, 0, 300, 1), vec![7; 1200]), RemoteCursor::Default);
    }
}
