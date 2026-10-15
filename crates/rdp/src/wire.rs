//! PDU framing with the D13 limits: declared lengths above [`Limits::max_pdu`] drop the
//! connection before anything is buffered, and a PDU (or, while connecting, any expected
//! PDU) that does not complete within [`Limits::stall`] fails the session.

use std::time::Duration;

use bytes::BytesMut;
use ironrdp::pdu::{Action, PduHint, find_size};
use rxt_core::{Error, Result};

use crate::legacy::Security;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::time::{Instant, timeout, timeout_at};

/// Resource caps for one RDP session (design D13).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest declared PDU length accepted (TPKT, fast-path, CredSSP).
    pub max_pdu: usize,
    /// Largest fast-path reassembly (sum of fragments) accepted.
    pub max_fragmented: usize,
    /// Largest single `Update` event; bigger regions are split into row bands.
    pub max_update_bytes: usize,
    /// Pixel bytes queued for the consumer; when full the socket is not read.
    pub queue_bytes: usize,
    /// Mid-PDU (and connection-phase) stall timeout.
    pub stall: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pdu: 1 << 20,
            max_fragmented: 8 << 20,
            max_update_bytes: 16 << 20,
            queue_bytes: 64 << 20,
            stall: Duration::from_secs(30),
        }
    }
}

impl Limits {
    pub(crate) fn check(&self) -> Result<()> {
        let ok = self.max_update_bytes >= 4 * usize::try_from(rxt_core::desktop::MAX_EDGE).unwrap_or(usize::MAX)
            && self.max_update_bytes <= self.queue_bytes
            && u32::try_from(self.queue_bytes).is_ok();
        if ok { Ok(()) } else { Err(Error::Invalid(format!("inconsistent RDP limits {self:?}"))) }
    }
}

pub(crate) struct Wire<S> {
    io: S,
    buf: BytesMut,
    limits: Limits,
    /// When the PDU being received started (or the connection-phase wait began).
    since: Option<Instant>,
    /// Standard RDP Security, once the Security Exchange PDU is sent.
    pub(crate) legacy: Option<Box<Security>>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Wire<S> {
    pub(crate) fn new(io: S, limits: Limits) -> Self {
        Self { io, buf: BytesMut::new(), limits, since: None, legacy: None }
    }

    /// The stream, once no unread bytes are buffered (before a TLS upgrade).
    pub(crate) fn into_inner(self) -> Result<S> {
        if self.buf.is_empty() { Ok(self.io) } else { Err(Error::Protocol("unexpected data before TLS".into())) }
    }

    /// Read more bytes. Idle waits (nothing buffered, `idle_ok`) have no deadline.
    async fn fill(&mut self, idle_ok: bool) -> Result<()> {
        // Called only when no complete PDU is buffered, so the pending one is larger still
        // (also covers framings whose length cannot be parsed yet).
        if self.buf.len() >= self.limits.max_pdu {
            return Err(Error::Protocol(format!("PDU length exceeds {}", self.limits.max_pdu)));
        }
        let n = if self.buf.is_empty() && idle_ok {
            self.io.read_buf(&mut self.buf).await?
        } else {
            let since = *self.since.get_or_insert_with(Instant::now);
            timeout_at(since + self.limits.stall, self.io.read_buf(&mut self.buf))
                .await
                .map_err(|_| Error::Protocol("peer stalled mid-PDU".into()))??
        };
        if self.since.is_none() && !self.buf.is_empty() {
            self.since = Some(Instant::now());
        }
        if n == 0 { Err(Error::Connection("connection closed by peer".into())) } else { Ok(()) }
    }

    fn take(&mut self, len: usize) -> Result<Option<BytesMut>> {
        if len > self.limits.max_pdu {
            return Err(Error::Protocol(format!("PDU length {len} exceeds {}", self.limits.max_pdu)));
        }
        if self.buf.len() < len {
            return Ok(None);
        }
        let pdu = self.buf.split_to(len);
        self.since = (!self.buf.is_empty()).then(Instant::now);
        // Every PDU goes through the legacy layer, dropped ones too (the RC4 stream is shared).
        match &mut self.legacy {
            Some(security) => Ok(Some(BytesMut::from(&security.open(&pdu)?[..]))),
            None => Ok(Some(pdu)),
        }
    }

    /// Next active-stage PDU. Cancel-safe (state lives in `self`).
    pub(crate) async fn read_pdu(&mut self) -> Result<(Action, BytesMut)> {
        loop {
            if let Some(info) = find_size(&self.buf).map_err(|e| Error::Protocol(e.to_string()))?
                && let Some(pdu) = self.take(info.length)?
            {
                return Ok((info.action, pdu));
            }
            self.fill(true).await?;
        }
    }

    /// Next PDU matching `hint` (connection phase: always under the stall deadline).
    pub(crate) async fn read_by_hint(&mut self, hint: &dyn PduHint) -> Result<BytesMut> {
        loop {
            if let Some((matched, len)) = hint.find_size(&self.buf).map_err(|e| Error::Protocol(e.to_string()))?
                && let Some(pdu) = self.take(len)?
            {
                if matched {
                    return Ok(pdu);
                }
                tracing::debug!(len, "dropped an unexpected PDU");
                continue;
            }
            self.fill(false).await?;
        }
    }

    pub(crate) async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.write_sealed(data, false).await
    }

    /// Writes `data`; with Standard RDP Security, `has_header` marks slow-path PDUs that already
    /// carry a basic security header (Client Info, licensing).
    pub(crate) async fn write_sealed(&mut self, data: &[u8], has_header: bool) -> Result<()> {
        let sealed;
        let data = match &mut self.legacy {
            Some(security) => {
                sealed = security.seal(data, has_header)?;
                &sealed[..]
            }
            None => data,
        };
        let io = &mut self.io;
        timeout(self.limits.stall, async {
            io.write_all(data).await?;
            io.flush().await
        })
        .await
        .map_err(|_| Error::Protocol("peer stopped reading".into()))??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits { max_pdu: 100, stall: Duration::from_millis(200), ..Limits::default() }
    }

    #[tokio::test]
    async fn oversized_declared_length_is_rejected_before_buffering() {
        let (mut peer, io) = tokio::io::duplex(64);
        let mut w = Wire::new(io, limits());
        // TPKT header declaring 0x1000 bytes; only the header is ever sent.
        peer.write_all(&[3, 0, 0x10, 0]).await.unwrap();
        let e = w.read_pdu().await.unwrap_err();
        assert!(e.to_string().contains("exceeds"), "{e}");
    }

    #[tokio::test]
    async fn unparseable_length_cannot_grow_the_buffer() {
        let (mut peer, io) = tokio::io::duplex(1024);
        let mut w = Wire::new(io, limits());
        // A hint that never finds a size: bytes keep coming but are never a complete PDU.
        #[derive(Debug)]
        struct Never;
        impl PduHint for Never {
            fn find_size(&self, _: &[u8]) -> ironrdp::core::DecodeResult<Option<(bool, usize)>> {
                Ok(None)
            }
        }
        peer.write_all(&[0; 300]).await.unwrap();
        let e = w.read_by_hint(&Never).await.unwrap_err();
        assert!(e.to_string().contains("exceeds"), "{e}");
    }

    #[tokio::test]
    async fn mid_pdu_stall_times_out_but_idle_does_not() {
        let (mut peer, io) = tokio::io::duplex(64);
        let mut w = Wire::new(io, limits());
        // Idle: no deadline.
        assert!(timeout(Duration::from_millis(400), w.read_pdu()).await.is_err());
        // Partial TPKT: stall error after `stall`.
        peer.write_all(&[3, 0, 0, 20, 1]).await.unwrap();
        let start = Instant::now();
        let e = w.read_pdu().await.unwrap_err();
        assert!(e.to_string().contains("stalled"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn complete_pdu_is_returned() {
        let (mut peer, io) = tokio::io::duplex(64);
        let mut w = Wire::new(io, limits());
        peer.write_all(&[3, 0, 0, 7, 2, 0xf0, 0x80]).await.unwrap();
        let (action, pdu) = w.read_pdu().await.unwrap();
        assert_eq!((action, pdu.len()), (Action::X224, 7));
    }

    #[test]
    fn limits_must_be_consistent() {
        assert!(Limits::default().check().is_ok());
        assert!(Limits { queue_bytes: 1 << 20, ..Limits::default() }.check().is_err());
        assert!(Limits { max_update_bytes: 100, ..Limits::default() }.check().is_err());
    }
}
