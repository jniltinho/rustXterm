//! Serial console sessions (design S3/S4) over the platform [`SerialPorts`] contract.
//! No OS calls here: the platform adapter opens and drives the port.
//!
//! A blocking reader thread reads with the adapter's 100 ms timeout and hands chunks to
//! [`SerialSession::read`] through a small bounded channel, so the port is only read as fast
//! as the engine pulls (backpressure reaches the driver and its flow control). Enter
//! translation and local echo happen on [`SerialWriter::write`], LF→CR LF on read.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rxt_core::{Error, PortInfo, Result, SerialEnter, SerialOptions};
use rxt_platform::{SerialPorts, SerialStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Chunks buffered between the reader thread and the engine.
const READ_CHUNKS: usize = 2;
const READ_BUF: usize = 4096;
/// Device poll interval while waiting for an unplugged device to return (S4).
pub const REPLUG_POLL: Duration = Duration::from_secs(1);

/// An open serial session. Dropping it stops the reader and closes the port.
pub struct SerialSession {
    rx: mpsc::Receiver<Result<Vec<u8>>>,
    echo_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    writer: SerialWriter,
    lf_to_crlf: bool,
    cancel: CancellationToken,
    port: Option<PortInfo>,
}

/// Cloneable write half: data, break and modem lines.
#[derive(Clone)]
pub struct SerialWriter {
    stream: Arc<Mutex<Box<dyn SerialStream>>>,
    enter: SerialEnter,
    echo: Option<mpsc::UnboundedSender<Vec<u8>>>,
}

/// Open the configured port: line settings, initial DTR/RTS, then start the reader.
/// Opening blocks briefly (device open, termios); it needs no runtime, the async calls do.
pub fn open(ports: Arc<dyn SerialPorts>, opts: &SerialOptions, cancel: CancellationToken) -> Result<SerialSession> {
    let path = opts.port.trim();
    if path.is_empty() {
        return Err(Error::Invalid("no serial port configured".into()));
    }
    let mut stream = ports.open(path, &opts.line)?;
    if opts.dtr.is_some() || opts.rts.is_some() {
        stream.set_lines(opts.dtr, opts.rts)?;
    }
    let writer = stream.try_clone_writer()?;
    let port =
        ports.list().ok().and_then(|l| l.into_iter().find(|p| p.path == path || p.by_id.as_deref() == Some(path)));

    let cancel = cancel.child_token();
    let (tx, rx) = mpsc::channel(READ_CHUNKS);
    let stop = cancel.clone();
    std::thread::Builder::new()
        .name("rxt-serial-read".into())
        .spawn(move || reader(stream, &tx, &stop))
        .map_err(|e| Error::Io(format!("starting serial reader: {e}")))?;

    let (echo_tx, echo_rx) = mpsc::unbounded_channel();
    Ok(SerialSession {
        rx,
        echo_rx,
        writer: SerialWriter {
            stream: Arc::new(Mutex::new(writer)),
            enter: opts.enter,
            echo: opts.local_echo.then_some(echo_tx),
        },
        lf_to_crlf: opts.lf_to_crlf,
        cancel,
        port,
    })
}

/// After an unplug: poll the port list every [`REPLUG_POLL`] and reopen the same device (S4).
/// Matches `last` by stable identity (by-id link, USB serial number), else the configured
/// path; ports absent from the list (e.g. ptys) are retried by path. Returns
/// `Err(Error::Cancelled)` on cancel; permission and settings errors end the wait.
pub async fn reopen(
    ports: Arc<dyn SerialPorts>,
    opts: &SerialOptions,
    last: Option<&PortInfo>,
    cancel: CancellationToken,
) -> Result<SerialSession> {
    loop {
        let list = ports.list().unwrap_or_default();
        let path = find_port(&list, &opts.port, last).unwrap_or_else(|| opts.port.clone());
        let opts = SerialOptions { port: path, ..opts.clone() };
        match open(ports.clone(), &opts, cancel.clone()) {
            Err(Error::NotFound(_) | Error::Gone(_) | Error::Busy(_) | Error::Io(_)) => {}
            r => return r,
        }
        tokio::select! {
            () = cancel.cancelled() => return Err(Error::Cancelled),
            () = tokio::time::sleep(REPLUG_POLL) => {}
        }
    }
}

/// Path of the listed port that is the same device as `last` (by-id, then USB serial
/// number), else the one matching `wanted` by path or by-id link.
pub fn find_port(list: &[PortInfo], wanted: &str, last: Option<&PortInfo>) -> Option<String> {
    let same = |p: &PortInfo| {
        last.is_some_and(|l| {
            (l.by_id.is_some() && p.by_id == l.by_id)
                || (l.serial_number.is_some() && (p.vid, p.pid, &p.serial_number) == (l.vid, l.pid, &l.serial_number))
        })
    };
    list.iter()
        .find(|p| same(p))
        .or_else(|| list.iter().find(|p| p.path == wanted || p.by_id.as_deref() == Some(wanted)))
        .map(|p| p.path.clone())
}

fn reader(mut stream: Box<dyn SerialStream>, tx: &mpsc::Sender<Result<Vec<u8>>>, stop: &CancellationToken) {
    let mut buf = vec![0u8; READ_BUF];
    while !stop.is_cancelled() {
        match stream.read(&mut buf) {
            Ok(0) => {}
            // Blocks while the engine is not pulling: backpressure to the driver (S3).
            Ok(n) => {
                if tx.blocking_send(Ok(buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.blocking_send(Err(e));
                return;
            }
        }
    }
}

impl SerialSession {
    /// Next received bytes (local echo included). `None` when cancelled or closed;
    /// `Some(Err(Error::Gone))` when the device was removed (then `None`).
    pub async fn read(&mut self) -> Option<Result<Vec<u8>>> {
        let r = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return None,
            Some(echo) = self.echo_rx.recv() => return Some(Ok(echo)),
            r = self.rx.recv() => r?,
        };
        Some(r.map(|b| if self.lf_to_crlf { lf_to_crlf(&b) } else { b }))
    }

    pub fn writer(&self) -> SerialWriter {
        self.writer.clone()
    }

    /// The listed port this session opened (identity for [`reopen`]); `None` for unlisted paths.
    pub fn port_info(&self) -> Option<&PortInfo> {
        self.port.as_ref()
    }
}

impl Drop for SerialSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl SerialWriter {
    /// Send typed input; CR (the terminal's Enter) becomes the configured line ending.
    pub async fn write(&self, bytes: &[u8]) -> Result<()> {
        if let Some(echo) = &self.echo {
            let _ = echo.send(translate(bytes, b"\r\n"));
        }
        let out = translate(bytes, self.enter.bytes());
        self.call(move |s| s.write_all(&out)).await
    }

    /// Hold a break condition for `ms` milliseconds.
    pub async fn send_break(&self, ms: u64) -> Result<()> {
        self.call(move |s| s.send_break(Duration::from_millis(ms))).await
    }

    /// Set DTR and/or RTS; `None` leaves a line unchanged.
    pub async fn set_lines(&self, dtr: Option<bool>, rts: Option<bool>) -> Result<()> {
        self.call(move |s| s.set_lines(dtr, rts)).await
    }

    async fn call(&self, f: impl FnOnce(&mut dyn SerialStream) -> Result<()> + Send + 'static) -> Result<()> {
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || {
            let mut s = stream.lock().map_err(|_| Error::Io("serial writer lock poisoned".into()))?;
            f(s.as_mut())
        })
        .await
        .map_err(|e| Error::Io(format!("serial writer task: {e}")))?
    }
}

/// Replace each CR with `enter`.
fn translate(bytes: &[u8], enter: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if b == b'\r' { out.extend_from_slice(enter) } else { out.push(b) }
    }
    out
}

/// Received LF → CR LF.
fn lf_to_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 8);
    for &b in bytes {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests;
