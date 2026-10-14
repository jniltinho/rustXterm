//! Follow the terminal folder of an SSH tab without shell cooperation (the UI still takes OSC 7
//! from the terminal stream first).
//!
//! Heuristic (Linux servers): once the shell channel is up, one exec on the same connection runs
//! [`PS_COMMAND`]. The exec's own process (`$$`) and the tab's login shell are both children of
//! the server's per-connection session process (OpenSSH and Dropbear fork one child per
//! channel), so the login shell is the process with the same parent that owns a terminal (`tty`
//! not `?`); the youngest wins when several match. Child shells (`sh`, `sudo -s`, nested ssh)
//! have the login shell as parent and are never followed. Not found yet: retried for about 5 s.
//! Output without a usable `ps -eo` table (BusyBox, macOS, BSD): the fallback is off for the
//! connection.
//!
//! Each Enter sent to the tab then schedules one check [`DEBOUNCE`] later: SFTP realpath of
//! `/proc/<pid>/cwd` on the tab's SFTP channel, reported as `Event::CwdChanged` only when it
//! changed. A failed realpath (no `/proc`, shell gone) re-detects the shell once, then stops.
// ponytail: one check per Enter; `sleep 4; cd /var` is missed. Add a check when
// the output goes quiet if that matters.

use std::sync::Arc;
use std::time::Duration;

use rxt_core::{Event, TabId};
use tokio::sync::mpsc;

use crate::Shared;
use crate::transport::Link;

/// One-shot command listing the processes; its first line is the exec's own PID.
pub(crate) const PS_COMMAND: &str = "exec sh -c 'echo $$; ps -eo tty,pid,ppid,etime,args'";
/// Delay between an Enter and the folder check (lets `cd` finish; coalesces pastes).
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(500);
const DETECT_TRIES: u32 = 5;
const DETECT_RETRY: Duration = Duration::from_secs(1);
const EXEC_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of looking for the login shell in [`PS_COMMAND`] output.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Shell {
    Pid(u32),
    /// The table is usable but the shell is not listed yet.
    NotYet,
    /// No usable `ps -eo` table.
    Unsupported,
}

/// Find the login shell of the connection in [`PS_COMMAND`] output (see the module docs).
pub(crate) fn find_shell(output: &str) -> Shell {
    let mut lines = output.lines();
    let Some(me) = lines.next().and_then(|l| l.trim().parse::<u32>().ok()) else { return Shell::Unsupported };
    // (tty, pid, ppid, age in seconds); the header and unparsable lines are skipped.
    let rows: Vec<(&str, u32, u32, u64)> = lines
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let tty = f.next()?;
            let pid = f.next()?.parse().ok()?;
            let ppid = f.next()?.parse().ok()?;
            Some((tty, pid, ppid, elapsed_secs(f.next()?)?))
        })
        .collect();
    let Some(parent) = rows.iter().find(|r| r.1 == me).map(|r| r.2) else { return Shell::Unsupported };
    rows.iter()
        .filter(|r| r.2 == parent && r.1 != me && !matches!(r.0, "?" | "-"))
        .min_by_key(|r| r.3)
        .map_or(Shell::NotYet, |r| Shell::Pid(r.1))
}

/// `ps` elapsed time `[[dd-]hh:]mm:ss` in seconds.
fn elapsed_secs(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, s),
    };
    let mut secs = 0u64;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

/// Login shell PID of `link`, retrying while it is not listed yet; `None` disables following.
async fn detect(link: &dyn Link) -> Option<u32> {
    for _ in 0..DETECT_TRIES {
        let out = match tokio::time::timeout(EXEC_TIMEOUT, link.exec(PS_COMMAND)).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                tracing::debug!("follow: exec failed: {e}");
                return None;
            }
            Err(_) => return None,
        };
        match find_shell(&out) {
            Shell::Pid(pid) => return Some(pid),
            Shell::NotYet => tokio::time::sleep(DETECT_RETRY).await,
            Shell::Unsupported => return None,
        }
    }
    None
}

/// Follow task of one SSH run. `kicks` carries Enter presses (`false`) and "follow turned on"
/// (`true`: report the folder even when unchanged). Returns when following is impossible.
pub(crate) async fn run(sh: Arc<Shared>, tab: TabId, link: Arc<dyn Link>, mut kicks: mpsc::Receiver<bool>) {
    let mut pid = None;
    let mut last: Option<String> = None;
    let mut redetected = false;
    let mut force = false;
    loop {
        if pid.is_none() {
            // A tab with follow off never runs the probe.
            if !sh.follows(tab) {
                match kicks.recv().await {
                    Some(f) => force |= f,
                    None => return,
                }
                continue;
            }
            let Some(found) = detect(link.as_ref()).await else { return };
            pid = Some(found);
        } else {
            match kicks.recv().await {
                Some(f) => force |= f,
                None => return,
            }
            tokio::time::sleep(DEBOUNCE).await;
            while let Ok(f) = kicks.try_recv() {
                force |= f;
            }
            if !sh.follows(tab) {
                continue;
            }
        }
        let Some(p) = pid else { continue };
        let Ok(fs) = crate::files::file_ops(&sh, tab).await else { return };
        match fs.real_path(&format!("/proc/{p}/cwd")).await {
            Ok(cwd) => {
                // The first check after connecting only records the start folder.
                let report = force || last.as_ref().is_some_and(|l| *l != cwd);
                force = false;
                last = Some(cwd.clone());
                if report {
                    sh.emit(Event::CwdChanged { tab, path: cwd }).await;
                }
            }
            Err(e) if !redetected => {
                tracing::debug!("follow: cannot resolve /proc/{p}/cwd: {e}; detecting the shell again");
                redetected = true;
                pid = None;
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
TT           PID    PPID     ELAPSED COMMAND
?              1       0  1-02:03:04 /usr/sbin/sshd -D
?            100       1       05:00 sshd: alice [priv]
?            101     100       04:59 sshd: alice@pts/0,pts/1
pts/0        102     101       04:58 -bash
pts/0        150     102       01:00 sh
pts/1        160     101       00:10 -zsh
?            200     101       00:00 ps -eo tty,pid,ppid,etime,args
";

    #[test]
    fn picks_youngest_login_shell_beside_the_exec() {
        assert_eq!(find_shell(&format!("200\n{TABLE}")), Shell::Pid(160));
        // Only one PTY channel: its shell, never the child shell `sh` (ppid 102).
        let one = TABLE.lines().filter(|l| !l.contains("-zsh")).collect::<Vec<_>>().join("\n");
        assert_eq!(find_shell(&format!("200\n{one}")), Shell::Pid(102));
    }

    #[test]
    fn shell_not_started_yet_or_unsupported_ps() {
        let none = TABLE.lines().filter(|l| !l.starts_with("pts/")).collect::<Vec<_>>().join("\n");
        assert_eq!(find_shell(&format!("200\n{none}")), Shell::NotYet);
        // BusyBox `ps` rejects `-eo`: only the PID line reaches stdout.
        assert_eq!(find_shell("200\n"), Shell::Unsupported);
        assert_eq!(find_shell("sh: ps: not found\n"), Shell::Unsupported);
        // Our own process missing from the table.
        assert_eq!(find_shell(&format!("999\n{TABLE}")), Shell::Unsupported);
    }

    #[test]
    fn elapsed_formats() {
        assert_eq!(elapsed_secs("00:07"), Some(7));
        assert_eq!(elapsed_secs("01:02:03"), Some(3723));
        assert_eq!(elapsed_secs("2-00:00:01"), Some(172_801));
        assert_eq!(elapsed_secs("x"), None);
    }
}
