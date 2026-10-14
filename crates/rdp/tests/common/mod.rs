//! Shared RDP test helpers: scripted prompter and in-memory trust store.
//! Lab accounts and passwords are fictional (see `lab/README.md`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rxt_core::{
    BoxFut, CertTrust, DesktopEvent, HostKeyDecision, PromptAnswer, PromptKind, Prompter, Protocol, Secret, Session,
};
use rxt_rdp::{Context, Desktop};
use tokio_util::sync::CancellationToken;

pub const ALICE_PASS: &str = "lab-alice-pass";

/// Fixed certificate decision, one password, and a record of every prompt.
pub struct Script {
    pub cert: PromptAnswer,
    pub password: Option<String>,
    pub prompts: Mutex<Vec<PromptKind>>,
    pub changed: Mutex<Vec<(String, String, String)>>,
}

impl Script {
    pub fn new(cert: HostKeyDecision, password: Option<&str>) -> Arc<Self> {
        Self::answer(PromptAnswer::HostKey(cert), password)
    }

    pub fn answer(cert: PromptAnswer, password: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            cert,
            password: password.map(str::to_owned),
            prompts: Mutex::new(Vec::new()),
            changed: Mutex::new(Vec::new()),
        })
    }

    pub fn prompts(&self) -> Vec<PromptKind> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn password_prompts(&self) -> usize {
        self.prompts().iter().filter(|p| matches!(p, PromptKind::Password { .. })).count()
    }

    pub fn cert_prompts(&self) -> usize {
        self.prompts().iter().filter(|p| matches!(p, PromptKind::CertificateUnknown { .. })).count()
    }
}

impl Prompter for Script {
    fn prompt(&self, kind: PromptKind) -> BoxFut<'_, PromptAnswer> {
        self.prompts.lock().unwrap().push(kind.clone());
        let answer = match kind {
            PromptKind::CertificateUnknown { .. } => self.cert.clone(),
            PromptKind::Password { .. } => match &self.password {
                Some(p) => PromptAnswer::Secret(Secret(p.clone())),
                None => PromptAnswer::Cancel,
            },
            _ => PromptAnswer::Cancel,
        };
        Box::pin(async move { answer })
    }

    fn host_key_changed(&self, host: &str, old: &str, new: &str) {
        self.changed.lock().unwrap().push((host.to_owned(), old.to_owned(), new.to_owned()));
    }
}

#[derive(Default)]
pub struct Trust(Mutex<HashMap<(String, u16), String>>);

impl CertTrust for Trust {
    fn get(&self, host: &str, port: u16) -> Option<String> {
        self.0.lock().unwrap().get(&(host.to_owned(), port)).cloned()
    }

    fn save(&self, host: &str, port: u16, fingerprint: &str) {
        self.0.lock().unwrap().insert((host.to_owned(), port), fingerprint.to_owned());
    }
}

pub fn session(user: &str, port: u16) -> Session {
    Session::new(Protocol::Rdp, "127.0.0.1", Some(user.into()), Some(port))
}

pub fn ctx(script: Arc<Script>, trust: Arc<Trust>) -> Context {
    Context { prompter: script, trust, cancel: CancellationToken::new() }
}

/// Next event within `within`; panics on timeout, error or end of session.
pub async fn next(d: &mut Desktop, within: Duration) -> DesktopEvent {
    tokio::time::timeout(within, d.next())
        .await
        .expect("no desktop event in time")
        .expect("session ended")
        .expect("session error")
}

/// Skip events until an `Update` arrives within `within`.
pub async fn next_update(d: &mut Desktop, within: Duration) -> DesktopEvent {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let e = next(d, left).await;
        if matches!(e, DesktopEvent::Update { .. }) {
            return e;
        }
    }
}
