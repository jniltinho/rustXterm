//! Port-forwarding tunnels: local, remote and dynamic (SOCKS5), loopback by default.

use std::sync::Arc;
use std::time::Duration;

use russh::client::Handle;
use rxt_core::{Error, Result, Tunnel, TunnelKind, TunnelState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::handler::{ClientHandler, Shared};

pub type OnState = Arc<dyn Fn(TunnelState) + Send + Sync>;

/// Delay before automatic reconnect attempt `attempt` (0-based): 1, 2, 4 … capped at 60 s.
pub fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(1u64.checked_shl(attempt).unwrap_or(u64::MAX).min(60))
}

fn bind_error(port: u16, e: &std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        Error::Io(format!("port {port} is already in use"))
    } else {
        Error::Io(format!("cannot listen on port {port}: {e}"))
    }
}

pub(crate) async fn start(
    handle: Arc<Handle<ClientHandler>>,
    shared: Arc<Shared>,
    tunnel: &Tunnel,
    cancel: CancellationToken,
    on_state: OnState,
) -> Result<()> {
    on_state(TunnelState::Starting);
    let result = match tunnel.kind {
        TunnelKind::Local | TunnelKind::Dynamic => {
            start_listener(handle, tunnel, cancel.clone(), on_state.clone()).await
        }
        TunnelKind::Remote => start_remote(handle, shared, tunnel, cancel, on_state.clone()).await,
    };
    match &result {
        Ok(()) => on_state(TunnelState::Running),
        Err(e) => on_state(TunnelState::Error(e.to_string())),
    }
    result
}

async fn start_listener(
    handle: Arc<Handle<ClientHandler>>,
    tunnel: &Tunnel,
    cancel: CancellationToken,
    on_state: OnState,
) -> Result<()> {
    let port = tunnel.bind_port;
    let listener = TcpListener::bind((tunnel.effective_bind_addr(), port)).await.map_err(|e| bind_error(port, &e))?;
    let dest = (tunnel.kind == TunnelKind::Local).then(|| (tunnel.dest_host.clone(), tunnel.dest_port));
    tokio::spawn(async move {
        loop {
            let (stream, peer) = tokio::select! {
                _ = cancel.cancelled() => break,
                r = listener.accept() => match r {
                    Ok(x) => x,
                    Err(e) => { tracing::warn!("tunnel accept on port {port}: {e}"); continue; }
                },
            };
            let (handle, dest, cancel) = (handle.clone(), dest.clone(), cancel.clone());
            tokio::spawn(async move {
                let work = serve_local(handle, stream, peer, dest);
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    r = work => if let Err(e) = r { tracing::debug!("tunnel connection on port {port}: {e}") },
                }
            });
        }
        on_state(TunnelState::Stopped);
    });
    Ok(())
}

/// One local (fixed `dest`) or SOCKS5 (`dest == None`) connection.
async fn serve_local(
    handle: Arc<Handle<ClientHandler>>,
    mut stream: TcpStream,
    peer: std::net::SocketAddr,
    dest: Option<(String, u16)>,
) -> std::io::Result<()> {
    let socks = dest.is_none();
    let (host, port) = match dest {
        Some(d) => d,
        None => socks5_handshake(&mut stream).await?,
    };
    let channel = handle
        .channel_open_direct_tcpip(host.as_str(), u32::from(port), peer.ip().to_string(), u32::from(peer.port()))
        .await;
    let channel = match channel {
        Ok(c) => c,
        Err(e) => {
            if socks {
                stream.write_all(&socks5_reply(SOCKS_HOST_UNREACHABLE)).await?;
            }
            return Err(std::io::Error::other(format!("{host}:{port}: {e}")));
        }
    };
    if socks {
        stream.write_all(&socks5_reply(0)).await?;
    }
    let mut remote = channel.into_stream();
    tokio::io::copy_bidirectional(&mut stream, &mut remote).await.map(|_| ())
}

async fn start_remote(
    handle: Arc<Handle<ClientHandler>>,
    shared: Arc<Shared>,
    tunnel: &Tunnel,
    cancel: CancellationToken,
    on_state: OnState,
) -> Result<()> {
    let addr = tunnel.effective_bind_addr().to_owned();
    let requested = tunnel.bind_port;
    let bound = handle.tcpip_forward(addr.as_str(), u32::from(requested)).await.map_err(|e| {
        Error::Connection(format!("remote port {requested} could not be forwarded (in use or denied): {e}"))
    })?;
    // The server reports the port only when it chose it (requested 0); russh then yields 0.
    let port = if requested == 0 { bound } else { u32::from(requested) };
    shared.add_forward(port, tunnel.dest_host.clone(), tunnel.dest_port);
    tokio::spawn(async move {
        cancel.cancelled().await;
        shared.remove_forward(port);
        if let Err(e) = handle.cancel_tcpip_forward(addr, port).await {
            tracing::debug!("cancel remote forward {port}: {e}");
        }
        on_state(TunnelState::Stopped);
    });
    Ok(())
}

const SOCKS_GENERAL_FAILURE: u8 = 1;
const SOCKS_HOST_UNREACHABLE: u8 = 4;
const SOCKS_CMD_UNSUPPORTED: u8 = 7;
const SOCKS_ATYP_UNSUPPORTED: u8 = 8;

fn socks5_reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

/// Bytes still needed after the 4-byte request header, or the SOCKS error code.
fn socks5_rest_len(header: &[u8; 4], first_addr_byte: u8) -> std::result::Result<usize, u8> {
    if header[0] != 5 {
        return Err(SOCKS_GENERAL_FAILURE);
    }
    if header[1] != 1 {
        return Err(SOCKS_CMD_UNSUPPORTED);
    }
    match header[3] {
        1 => Ok(4 + 2),
        3 => Ok(1 + usize::from(first_addr_byte) + 2),
        4 => Ok(16 + 2),
        _ => Err(SOCKS_ATYP_UNSUPPORTED),
    }
}

/// Parse a complete SOCKS5 CONNECT request into `(host, port)`, or the SOCKS error code.
pub fn parse_socks5_request(req: &[u8]) -> std::result::Result<(String, u16), u8> {
    let header: &[u8; 4] = req.get(..4).and_then(|h| h.try_into().ok()).ok_or(SOCKS_GENERAL_FAILURE)?;
    let need = socks5_rest_len(header, req.get(4).copied().unwrap_or(0))?;
    let rest = req.get(4..).filter(|r| r.len() == need).ok_or(SOCKS_GENERAL_FAILURE)?;
    let (addr, port) = rest.split_at(need - 2);
    let port = u16::from_be_bytes([port[0], port[1]]);
    let host = match header[3] {
        1 => std::net::Ipv4Addr::from(<[u8; 4]>::try_from(addr).map_err(|_| SOCKS_GENERAL_FAILURE)?).to_string(),
        4 => std::net::Ipv6Addr::from(<[u8; 16]>::try_from(addr).map_err(|_| SOCKS_GENERAL_FAILURE)?).to_string(),
        _ => String::from_utf8(addr[1..].to_vec()).map_err(|_| SOCKS_GENERAL_FAILURE)?,
    };
    Ok((host, port))
}

/// No-auth greeting plus CONNECT request; on failure replies with the error before returning.
async fn socks5_handshake(s: &mut TcpStream) -> std::io::Result<(String, u16)> {
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).await?;
    let mut methods = vec![0u8; usize::from(hello[1])];
    s.read_exact(&mut methods).await?;
    if hello[0] != 5 || !methods.contains(&0) {
        s.write_all(&[5, 0xff]).await?;
        return Err(std::io::Error::other("SOCKS5: no acceptable auth method"));
    }
    s.write_all(&[5, 0]).await?;
    let mut req = vec![0u8; 5];
    s.read_exact(&mut req).await?;
    let header = [req[0], req[1], req[2], req[3]];
    let need = match socks5_rest_len(&header, req[4]) {
        Ok(n) => n,
        Err(code) => {
            s.write_all(&socks5_reply(code)).await?;
            return Err(std::io::Error::other(format!("SOCKS5 request refused ({code})")));
        }
    };
    req.resize(4 + need, 0);
    s.read_exact(&mut req[5..]).await?;
    match parse_socks5_request(&req) {
        Ok(dest) => Ok(dest),
        Err(code) => {
            s.write_all(&socks5_reply(code)).await?;
            Err(std::io::Error::other(format!("SOCKS5 request refused ({code})")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let secs: Vec<u64> = (0..8).map(|a| reconnect_delay(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(reconnect_delay(200).as_secs(), 60);
    }

    #[test]
    fn socks5_requests() {
        assert_eq!(parse_socks5_request(&[5, 1, 0, 1, 10, 0, 0, 7, 0x1f, 0x90]), Ok(("10.0.0.7".into(), 8080)));
        let mut dom = vec![5, 1, 0, 3, 11];
        dom.extend_from_slice(b"db.internal");
        dom.extend_from_slice(&5432u16.to_be_bytes());
        assert_eq!(parse_socks5_request(&dom), Ok(("db.internal".into(), 5432)));
        let mut v6 = vec![5, 1, 0, 4];
        v6.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        v6.extend_from_slice(&[0, 22]);
        assert_eq!(parse_socks5_request(&v6), Ok(("::1".into(), 22)));
        assert_eq!(parse_socks5_request(&[5, 2, 0, 1, 1, 2, 3, 4, 0, 1]), Err(SOCKS_CMD_UNSUPPORTED));
        assert_eq!(parse_socks5_request(&[5, 1, 0, 9, 1, 2, 3, 4, 0, 1]), Err(SOCKS_ATYP_UNSUPPORTED));
        assert_eq!(parse_socks5_request(&[4, 1, 0, 1, 1, 2, 3, 4, 0, 1]), Err(SOCKS_GENERAL_FAILURE));
        assert_eq!(parse_socks5_request(&[5, 1, 0, 1, 1, 2]), Err(SOCKS_GENERAL_FAILURE));
    }

    #[test]
    fn tunnel_bind_defaults_are_loopback() {
        for kind in [TunnelKind::Local, TunnelKind::Dynamic, TunnelKind::Remote] {
            let t = Tunnel { kind, ..Tunnel::default() };
            assert!(matches!(t.effective_bind_addr(), "127.0.0.1" | "localhost"));
            let exposed = Tunnel { kind, bind_addr: "0.0.0.0".into(), ..Tunnel::default() };
            assert_ne!(exposed.effective_bind_addr(), "0.0.0.0");
            let confirmed = Tunnel { exposure_confirmed: true, ..exposed };
            assert_eq!(confirmed.effective_bind_addr(), "0.0.0.0");
        }
    }

    #[tokio::test]
    async fn port_in_use_error_names_port() {
        let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let err = TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| bind_error(port, &e)).unwrap_err();
        assert!(err.to_string().contains(&port.to_string()), "{err}");
    }
}
