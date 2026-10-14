//! RDP client on IronRDP (design D13).
//!
//! Contract used by `engine` (keep stable; extend, don't break): [`connect`] returns a
//! [`Desktop`]; the engine pulls [`Desktop::next`] (bounded, so slow consumers apply
//! backpressure) and sends input through [`InputSender`]. Dropping `Desktop` cancels it.
//!
//! Order of a connection: TCP, X.224 negotiation, TLS (leaf captured; rustls, or OpenSSL down to
//! TLS 1.0 when `RdpOptions::allow_legacy_tls`), trust decision, password
//! prompt, CredSSP/NLA or TLS logon, capability exchange (desktop size checked), active stage.

mod keymap;
mod session;
mod tls;
mod trust;
mod wire;

use std::sync::Arc;

use rxt_core::{CertTrust, DesktopEvent, DesktopInput, Error, Prompter, Result, Session};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_util::sync::{CancellationToken, DropGuard};

pub use trust::fingerprint;
pub use wire::Limits;

/// Inputs for [`connect`].
#[derive(Clone)]
pub struct Context {
    pub prompter: Arc<dyn Prompter>,
    pub trust: Arc<dyn CertTrust>,
    pub cancel: CancellationToken,
}

type Queued = (Result<DesktopEvent>, Option<OwnedSemaphorePermit>);

/// A connected remote desktop.
pub struct Desktop {
    events: mpsc::UnboundedReceiver<Queued>,
    input: mpsc::Sender<DesktopInput>,
    _cancel: DropGuard,
}

/// Cloneable input handle; never blocks.
#[derive(Clone)]
pub struct InputSender {
    tx: mpsc::Sender<DesktopInput>,
}

/// Connect with the default [`Limits`]. `size` is the tab size in pixels (used by `FitToTab`).
pub async fn connect(session: &Session, ctx: Context, size: (u32, u32)) -> Result<Desktop> {
    connect_with_limits(session, ctx, size, Limits::default()).await
}

/// [`connect`] with explicit limits (tests use small ones).
pub async fn connect_with_limits(session: &Session, ctx: Context, size: (u32, u32), limits: Limits) -> Result<Desktop> {
    limits.check()?;
    let cancel = ctx.cancel.child_token();
    let driver = tokio::select! {
        () = cancel.cancelled() => return Err(Error::Cancelled),
        d = session::establish(session, &ctx, size, limits) => d?,
    };
    let (events_tx, events) = mpsc::unbounded_channel();
    let (input, input_rx) = mpsc::channel(1024);
    let task_cancel = cancel.clone();
    tokio::spawn(async move {
        let errors = events_tx.clone();
        tokio::select! {
            () = task_cancel.cancelled() => {}
            r = driver.run(session::Queue::new(events_tx, limits.queue_bytes), input_rx) => {
                if let Err(e) = r {
                    tracing::debug!(error = %e, "RDP session ended");
                    let _ = errors.send((Err(e), None));
                }
            }
        }
    });
    Ok(Desktop { events, input, _cancel: cancel.drop_guard() })
}

impl Desktop {
    /// Next event; `None` when the session ended.
    pub async fn next(&mut self) -> Option<Result<DesktopEvent>> {
        self.events.recv().await.map(|(event, _permit)| event)
    }

    pub fn input(&self) -> InputSender {
        InputSender { tx: self.input.clone() }
    }
}

impl InputSender {
    pub fn send(&self, input: DesktopInput) -> Result<()> {
        self.tx.try_send(input).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => Error::Protocol("RDP input queue full".into()),
            mpsc::error::TrySendError::Closed(_) => Error::Connection("RDP session closed".into()),
        })
    }
}
