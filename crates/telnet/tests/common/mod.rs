//! Shared test helpers: session builders, a no-op prompter and an output collector.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use rxt_core::{BoxFut, PromptAnswer, PromptKind, Prompter, Protocol, Session};
use rxt_telnet::{Context, TelnetSession};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

/// Telnet never prompts through the engine; any prompt is answered with `Cancel`.
pub struct NoPrompt;

impl Prompter for NoPrompt {
    fn prompt(&self, _: PromptKind) -> BoxFut<'_, PromptAnswer> {
        Box::pin(async { PromptAnswer::Cancel })
    }
    fn host_key_changed(&self, _: &str, _: &str, _: &str) {}
}

pub fn session(port: u16, user: Option<&str>) -> Session {
    Session::new(Protocol::Telnet, "127.0.0.1", user.map(str::to_owned), Some(port))
}

pub fn ctx(password: Option<&str>) -> Context {
    Context {
        prompter: Arc::new(NoPrompt),
        password: password.map(SecretString::from),
        cancel: CancellationToken::new(),
    }
}

/// Read until `needle` appears in the accumulated output; returns everything read.
pub async fn read_until(t: &mut TelnetSession, needle: &[u8], within: Duration) -> Vec<u8> {
    let mut all = Vec::new();
    let found = tokio::time::timeout(within, async {
        while let Some(chunk) = t.read().await {
            all.extend(chunk);
            if all.windows(needle.len()).any(|w| w == needle) {
                return true;
            }
        }
        false
    })
    .await;
    assert!(
        matches!(found, Ok(true)),
        "{:?} not seen within {within:?}; got {:?}",
        String::from_utf8_lossy(needle),
        String::from_utf8_lossy(&all)
    );
    all
}
