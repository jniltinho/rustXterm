//! WSL startup commands (wsl-sessions spec, design W3). The distribution starts the user's own
//! login shell; once a prompt shows, the saved startup lines are typed into it like keyboard input
//! (so what they set stays for the session and no quoting crosses Windows). A prompt is "the
//! terminal went quiet for [`QUIET`] after the first output and the cursor line ends with a usual
//! prompt end". When none shows within [`DEADLINE`], the UI offers to run them now or skip.

use std::sync::Arc;
use std::time::Duration;

use rxt_core::{Event, TabId};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use crate::{InputMsg, Shared};

/// Quiet period after output before the cursor line is checked for a prompt.
const QUIET: Duration = Duration::from_millis(300);
/// How long to wait for a prompt before offering the "run now?" bar.
const DEADLINE: Duration = Duration::from_secs(10);
/// Last bytes of (ANSI-stripped) output kept to test the cursor line.
const TAIL: usize = 256;
/// Usual prompt ends (with the trailing space the shell prints before the cursor).
const PROMPT_ENDS: [&str; 5] = ["$ ", "# ", "> ", "% ", "❯ "];

/// Tracks the ANSI-stripped tail of shell output to tell when the cursor sits at a prompt.
#[derive(Default)]
pub(crate) struct PromptWatch {
    tail: Vec<u8>,
    ansi: Ansi,
}

#[derive(Default, Clone, Copy)]
enum Ansi {
    #[default]
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

impl PromptWatch {
    /// Feed output, stripping ANSI escapes and control characters (CR/LF become a line break).
    pub(crate) fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.ansi = match (self.ansi, b) {
                (Ansi::Text, 0x1b) => Ansi::Esc,
                (Ansi::Text, b'\r' | b'\n') => {
                    self.tail.push(b'\n');
                    Ansi::Text
                }
                (Ansi::Text, b) if b < 0x20 || b == 0x7f => Ansi::Text,
                (Ansi::Text, b) => {
                    self.tail.push(b);
                    Ansi::Text
                }
                (Ansi::Esc, b'[') => Ansi::Csi,
                (Ansi::Esc, b']') => Ansi::Osc,
                (Ansi::Esc, _) => Ansi::Text,
                (Ansi::Csi, 0x40..=0x7e) => Ansi::Text,
                (Ansi::Csi, _) => Ansi::Csi,
                (Ansi::Osc, 0x07) => Ansi::Text,
                (Ansi::Osc, 0x1b) => Ansi::OscEsc,
                (Ansi::Osc, _) => Ansi::Osc,
                (Ansi::OscEsc, b'\\') => Ansi::Text,
                (Ansi::OscEsc, _) => Ansi::Osc,
            };
        }
        if self.tail.len() > TAIL {
            self.tail.drain(..self.tail.len() - TAIL);
        }
    }

    /// Whether the cursor line ends with a usual prompt end.
    pub(crate) fn at_prompt(&self) -> bool {
        let text = String::from_utf8_lossy(&self.tail);
        let line = text.rsplit('\n').next().unwrap_or(&text);
        PROMPT_ENDS.iter().any(|p| line.ends_with(p))
    }
}

/// Type the startup lines into the shell once a prompt shows, or (after [`DEADLINE`]) on the user's
/// go-ahead. `out` carries the tab's output chunks (a tee of the shell output); `input` is the
/// tab's keyboard input channel, so the lines echo like typed text and skip data-stream capture.
pub(crate) async fn run_startup(
    sh: &Arc<Shared>,
    tab: TabId,
    generation: u64,
    lines: Vec<String>,
    mut out: mpsc::Receiver<Vec<u8>>,
    input: mpsc::Sender<InputMsg>,
    cancel: &CancellationToken,
) {
    let mut watch = PromptWatch::default();
    let mut seen = false;
    let deadline = sleep(DEADLINE);
    let quiet = sleep(Duration::from_secs(3600));
    tokio::pin!(deadline, quiet);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            chunk = out.recv() => match chunk {
                Some(bytes) => {
                    watch.feed(&bytes);
                    seen = true;
                    quiet.as_mut().reset(Instant::now() + QUIET);
                }
                None => return, // output ended before a prompt
            },
            () = &mut quiet, if seen => {
                if watch.at_prompt() {
                    type_lines(&input, &lines).await;
                    return;
                }
                // Not a prompt; wait for the next output to re-arm the quiet timer.
                quiet.as_mut().reset(Instant::now() + Duration::from_secs(3600));
            }
            () = &mut deadline => {
                // No quiet prompt within the deadline (a profile may still be printing or asking a
                // question): never type into that, offer the Run/Skip bar instead.
                if offer_and_wait(sh, tab, generation, cancel).await {
                    type_lines(&input, &lines).await;
                }
                return;
            }
        }
    }
}

/// Send each line followed by Enter, as keyboard input.
async fn type_lines(input: &mpsc::Sender<InputMsg>, lines: &[String]) {
    for line in lines {
        let mut bytes = line.clone().into_bytes();
        bytes.push(b'\r');
        if input.send(InputMsg::Bytes(bytes)).await.is_err() {
            return;
        }
    }
}

/// Emit the "run now?" offer and wait for the answer (`Command::WslStartup`). `false` on cancel.
async fn offer_and_wait(sh: &Arc<Shared>, tab: TabId, generation: u64, cancel: &CancellationToken) -> bool {
    let (tx, rx) = oneshot::channel();
    {
        let mut tabs = sh.tabs.lock();
        let Some(t) = tabs.get_mut(&tab).filter(|t| t.generation == generation) else { return false };
        t.wsl_run = Some(tx);
    }
    sh.emit(Event::WslStartupPending { tab }).await;
    tokio::select! {
        () = cancel.cancelled() => false,
        answer = rx => answer.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_prompt_ends_after_ansi() {
        let mut w = PromptWatch::default();
        w.feed(b"Welcome\r\nalice@box:~$ ");
        assert!(w.at_prompt());

        let mut w = PromptWatch::default();
        w.feed(b"\x1b[32malice@box\x1b[0m:~# ");
        assert!(w.at_prompt(), "colored root prompt");

        let mut w = PromptWatch::default();
        w.feed("user ❯ ".as_bytes());
        assert!(w.at_prompt(), "fish-style prompt");
    }

    #[test]
    fn not_a_prompt_mid_output() {
        let mut w = PromptWatch::default();
        w.feed(b"Loading modules...\n");
        assert!(!w.at_prompt());
        w.feed(b"Continue? [y/N] ");
        assert!(!w.at_prompt(), "a question is not a prompt end");
    }

    #[test]
    fn tail_is_bounded() {
        let mut w = PromptWatch::default();
        for _ in 0..100 {
            w.feed(&[b'x'; 1000]);
        }
        assert!(w.tail.len() <= TAIL);
    }
}
