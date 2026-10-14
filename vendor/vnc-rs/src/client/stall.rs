//! rustXterm patch: fail a read that stalls in the middle of a server message.
//!
//! Waiting for the first byte of the next message is idle time and never times out; once a
//! message has started, every read must make progress within [`STALL_TIMEOUT`].

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::{sleep, Sleep};

/// Longest wait for more bytes of a message that already started.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct StallGuard<R> {
    inner: R,
    idle: bool,
    deadline: Option<Pin<Box<Sleep>>>,
}

impl<R> StallGuard<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            idle: true,
            deadline: None,
        }
    }

    /// The next byte starts a new message.
    pub(crate) fn message_boundary(&mut self) {
        self.idle = true;
        self.deadline = None;
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for StallGuard<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                if buf.filled().len() > before {
                    this.idle = false;
                }
                this.deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending if this.idle => Poll::Pending,
            Poll::Pending => {
                let deadline = this
                    .deadline
                    .get_or_insert_with(|| Box::pin(sleep(STALL_TIMEOUT)));
                match deadline.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "VNC server stalled in the middle of a message",
                    ))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}
