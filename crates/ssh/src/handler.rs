//! russh client handler: the host-key gate and server-opened channel policy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use russh::client::{ChannelOpenHandle, Msg, Session};
use russh::compression::Compression;
use russh::keys::{PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelOpenFailure};
use rxt_core::{Error, HostKeyDecision, PromptAnswer, PromptKind, Result, SshInfo};

use crate::known_hosts::{KnownHosts, Verdict, evaluate, fingerprint, host_label};
use crate::x11::X11;
use crate::{Prompter, TrustStore};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Host-key gate for one hop.
pub(crate) struct Gate {
    pub host: String,
    pub port: u16,
    pub known_hosts: PathBuf,
    pub trust: Arc<TrustStore>,
    pub prompter: Arc<dyn Prompter>,
    /// Why the key was refused, for a precise error after the handshake fails.
    pub failure: Arc<Mutex<Option<Error>>>,
}

impl Gate {
    async fn check(&self, key: &PublicKey) -> Result<()> {
        let mut entries = KnownHosts::load(&self.known_hosts)?.matching(&self.host, self.port);
        entries.extend(self.trust.keys(&self.host, self.port).into_iter().map(|k| (false, k)));
        let label = host_label(&self.host, self.port);
        match evaluate(key, &entries) {
            Verdict::Trusted => Ok(()),
            Verdict::Revoked => Err(Error::HostKey(format!("{label}: server key {} is revoked", fingerprint(key)))),
            Verdict::Changed { old_fingerprint } => {
                let new = fingerprint(key);
                self.prompter.host_key_changed(&label, &old_fingerprint, &new);
                Err(Error::HostKey(format!("{label}: host key changed (was {old_fingerprint}, now {new})")))
            }
            Verdict::Unknown => {
                let kind = PromptKind::HostKeyUnknown {
                    host: self.host.clone(),
                    port: self.port,
                    algorithm: key.algorithm().as_str().to_owned(),
                    fingerprint: fingerprint(key),
                };
                match self.prompter.prompt(kind).await {
                    PromptAnswer::HostKey(HostKeyDecision::AcceptOnce) => Ok(()),
                    PromptAnswer::HostKey(HostKeyDecision::AcceptAndSave) => {
                        self.trust.add(&self.host, self.port, key.clone());
                        Ok(())
                    }
                    _ => Err(Error::HostKey(format!("{label}: host key not accepted"))),
                }
            }
        }
    }
}

/// Per-connection forwarding state shared with [`crate::Client`].
#[derive(Default)]
pub(crate) struct Shared {
    pub x11: Option<X11>,
    /// Set once X11 forwarding was requested on a channel.
    pub x11_requested: AtomicBool,
    pub agent_forwarding: bool,
    /// Remote forwards: bound server port → local destination.
    pub forwards: Mutex<HashMap<u32, (String, u16)>>,
    /// Connection facts for the session banner.
    pub info: Mutex<SshInfo>,
    /// Sanitized pre-auth banners (`SSH_MSG_USERAUTH_BANNER`), at most [`AUTH_BANNER_CAP`] bytes.
    pub auth_banner: Mutex<String>,
}

/// Largest pre-auth banner kept per connection.
pub const AUTH_BANNER_CAP: usize = 64 << 10;

/// Server text made safe for the terminal: control characters other than tab and newline are
/// dropped (no escape sequences), newlines become CRLF, and the result is at most `cap` bytes.
pub fn sanitize_banner(text: &str, cap: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let mut buf = [0; 4];
        let piece = match c {
            '\n' => "\r\n",
            '\t' => "\t",
            c if c.is_control() => continue,
            c => c.encode_utf8(&mut buf),
        };
        if out.len() + piece.len() > cap {
            break;
        }
        out.push_str(piece);
    }
    out
}

impl Shared {
    pub fn add_forward(&self, port: u32, host: String, dest_port: u16) {
        lock(&self.forwards).insert(port, (host, dest_port));
    }

    pub fn remove_forward(&self, port: u32) {
        lock(&self.forwards).remove(&port);
    }

    fn forward(&self, port: u32) -> Option<(String, u16)> {
        lock(&self.forwards).get(&port).cloned()
    }
}

pub(crate) struct ClientHandler {
    pub gate: Gate,
    pub shared: Arc<Shared>,
}

async fn reject(reply: ChannelOpenHandle) {
    reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
}

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let result = match server_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                {
                    let mut info = lock(&self.shared.info);
                    info.host_key_algorithm = key.algorithm().as_str().to_owned();
                    info.host_key_fingerprint = fingerprint(key);
                }
                self.gate.check(key).await
            }
            PublicKeyOrCertificate::Certificate(_) => Err(Error::HostKey("host certificates are not supported".into())),
        };
        match result {
            Ok(()) => Ok(true),
            Err(e) => {
                *lock(&self.gate.failure) = Some(e);
                Ok(false)
            }
        }
    }

    async fn auth_banner(&mut self, banner: &str, _: &mut Session) -> Result<(), Self::Error> {
        tracing::debug!("{}: pre-auth banner of {} bytes", self.gate.host, banner.len());
        let mut kept = lock(&self.shared.auth_banner);
        let room = AUTH_BANNER_CAP.saturating_sub(kept.len());
        let mut text = sanitize_banner(banner, room);
        if !text.is_empty() && !text.ends_with('\n') && text.len() + 2 <= room {
            text.push_str("\r\n");
        }
        kept.push_str(&text);
        Ok(())
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut info = lock(&self.shared.info);
        info.server_id = String::from_utf8_lossy(session.remote_sshid()).into_owned();
        info.kex = names.kex.as_ref().to_owned();
        info.cipher = names.cipher.as_ref().to_owned();
        info.compression =
            !matches!((&names.client_compression, &names.server_compression), (Compression::None, Compression::None));
        Ok(())
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some((host, port)) = self.shared.forward(connected_port) else {
            reject(reply).await;
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let r = async {
                let mut local = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
                tokio::io::copy_bidirectional(&mut local, &mut channel.into_stream()).await
            };
            if let Err(e) = r.await {
                tracing::debug!("remote forward to {host}:{port}: {e}");
            }
        });
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        match &self.shared.x11 {
            Some(x11) if self.shared.x11_requested.load(Ordering::Acquire) => {
                reply.accept().await;
                tokio::spawn(x11.clone().serve(channel));
            }
            _ => reject(reply).await,
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.shared.agent_forwarding {
            match crate::connect_agent().await {
                Ok(agent) => {
                    reply.accept().await;
                    tokio::spawn(async move {
                        let mut agent = agent.into_inner();
                        if let Err(e) = tokio::io::copy_bidirectional(&mut agent, &mut channel.into_stream()).await {
                            tracing::debug!("agent forwarding: {e}");
                        }
                    });
                    return Ok(());
                }
                Err(e) => tracing::debug!("agent forwarding: no local agent: {e}"),
            }
        }
        let _ = channel;
        reject(reply).await;
        Ok(())
    }

    async fn server_channel_open_session(
        &mut self,
        _: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reject(reply).await;
        Ok(())
    }

    async fn server_channel_open_direct_tcpip(
        &mut self,
        _: Channel<Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reject(reply).await;
        Ok(())
    }

    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _: Channel<Msg>,
        _: &str,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reject(reply).await;
        Ok(())
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _: Channel<Msg>,
        _: &str,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reject(reply).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_keeps_text_and_drops_control_sequences() {
        let raw = "Lab notice\r\n\x1b[2J\x1b]0;pwned\x07Authorized use only\n\tby fictional staff\x00\u{9b}31m";
        let clean = sanitize_banner(raw, AUTH_BANNER_CAP);
        assert_eq!(clean, "Lab notice\r\n[2J]0;pwnedAuthorized use only\r\n\tby fictional staff31m");
        assert!(!clean.chars().any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t')));
    }

    #[test]
    fn banner_is_capped_on_char_boundaries() {
        let clean = sanitize_banner(&"é".repeat(100), 9);
        assert_eq!(clean, "éééé");
        assert!(sanitize_banner(&"x".repeat(AUTH_BANNER_CAP * 2), AUTH_BANNER_CAP).len() == AUTH_BANNER_CAP);
    }
}
