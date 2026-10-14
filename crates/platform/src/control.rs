//! Control-channel framing: 4-byte big-endian length, then a JSON document of at most
//! [`MAX_FRAME`] bytes. A larger frame is an error; the caller closes the connection.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub use rxt_core::control::MAX_FRAME;

use crate::{Error, Result};

/// Next frame; `Ok(None)` on a clean end of stream before a length prefix.
pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(Error::Invalid(format!("control frame of {len} bytes exceeds {MAX_FRAME}")));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(w: &mut W, data: &[u8]) -> Result<()> {
    let len = u32::try_from(data.len())
        .ok()
        .filter(|&n| n as usize <= MAX_FRAME)
        .ok_or_else(|| Error::Invalid(format!("control frame of {} bytes exceeds {MAX_FRAME}", data.len())))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(data).await?;
    w.flush().await?;
    Ok(())
}

/// Read and decode one JSON message (`Ok(None)` at end of stream).
pub async fn read_message<T: DeserializeOwned, R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<Option<T>> {
    let Some(frame) = read_frame(r).await? else { return Ok(None) };
    serde_json::from_slice(&frame).map(Some).map_err(|e| Error::Invalid(format!("control message: {e}")))
}

/// Encode and write one JSON message.
pub async fn write_message<T: Serialize, W: AsyncWrite + Unpin + ?Sized>(w: &mut W, msg: &T) -> Result<()> {
    let data = serde_json::to_vec(msg).map_err(|e| Error::Invalid(format!("control message: {e}")))?;
    write_frame(w, &data).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxt_core::control::{AgentOp, EntryPoint, Request};

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    #[test]
    fn round_trip_and_clean_eof() {
        block(async {
            let (mut a, mut b) = tokio::io::duplex(4096);
            let req = Request { id: 7, client: "t".into(), entry: EntryPoint::CliLive, op: AgentOp::ListTabs };
            write_message(&mut a, &req).await.unwrap();
            drop(a);
            assert_eq!(read_message::<Request, _>(&mut b).await.unwrap(), Some(req));
            assert_eq!(read_frame(&mut b).await.unwrap(), None);
        });
    }

    #[test]
    fn frame_limit() {
        block(async {
            let (mut a, mut b) = tokio::io::duplex(64);
            // Exactly MAX_FRAME is accepted by the writer; one more byte is not.
            assert!(write_frame(&mut tokio::io::sink(), &vec![0; MAX_FRAME]).await.is_ok());
            assert!(write_frame(&mut tokio::io::sink(), &vec![0; MAX_FRAME + 1]).await.is_err());
            // An oversized length prefix is rejected before any allocation of the body.
            a.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes()).await.unwrap();
            assert!(matches!(read_frame(&mut b).await, Err(Error::Invalid(_))));
        });
    }

    #[test]
    fn truncated_frame_is_an_error() {
        block(async {
            let (mut a, mut b) = tokio::io::duplex(64);
            a.write_all(&10u32.to_be_bytes()).await.unwrap();
            a.write_all(b"abc").await.unwrap();
            drop(a);
            assert!(read_frame(&mut b).await.is_err());
        });
    }
}
