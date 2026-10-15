//! Local shell on Linux (local-shell spec) through `portable-pty`: the user's login shell in its
//! own session (`setsid` + controlling terminal). Closing sends SIGHUP to every process group of
//! that session, then SIGKILL after [`KILL_GRACE`]; processes that left the session are not
//! reached (documented limit).

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use rxt_core::LaunchInfo;
use rxt_platform::{Error, LocalPty, LocalShell, PtyControl, PtyReader, PtyWriter, Result};

/// Time between SIGHUP and SIGKILL on close.
const KILL_GRACE: Duration = Duration::from_secs(1);
/// Variables the AppImage runtime adds; the shell must not inherit them.
const APPIMAGE_VARS: [&str; 4] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// Linux local shells.
#[derive(Default)]
pub struct LinuxPty;

impl LocalPty for LinuxPty {
    fn prepare(&self) -> Result<LaunchInfo> {
        Ok(launch_info(&child_env(std::env::vars_os())))
    }

    fn spawn(&self, info: &LaunchInfo, cols: u16, rows: u16, _cursor: (u16, u16)) -> Result<LocalShell> {
        let mut builder = builder(&child_env(std::env::vars_os()));
        builder.env("SHELL", &info.shell);
        builder.cwd(&info.cwd);
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| Error::Io(format!("cannot create a pseudo-terminal: {e}")))?;
        let child =
            pair.slave.spawn_command(builder).map_err(|e| Error::Io(format!("cannot start {}: {e}", info.shell)))?;
        // The shell holds the slave now; keeping ours would hide its end of output.
        drop(pair.slave);
        let pid = child
            .process_id()
            .and_then(|p| i32::try_from(p).ok())
            .ok_or_else(|| Error::Io("shell has no pid".into()))?;
        let reader = pair.master.try_clone_reader().map_err(|e| Error::Io(e.to_string()))?;
        let writer = pair.master.take_writer().map_err(|e| Error::Io(e.to_string()))?;
        let control = Arc::new(Control { pid, child: Mutex::new(child), master: Mutex::new(Some(pair.master)) });
        Ok(LocalShell { reader: Box::new(Reader(reader)), writer: Box::new(Writer(writer)), control })
    }
}

/// Shell (`$SHELL` if executable, else the passwd shell, else `/bin/sh`), folder (`$HOME`,
/// else `/`), user and host for `env`.
fn launch_info(env: &[(OsString, OsString)]) -> LaunchInfo {
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_string_lossy().into_owned());
    let cwd = get("HOME").filter(|h| Path::new(h).is_dir()).unwrap_or_else(|| "/".into());
    let user = get("USER").or_else(|| get("LOGNAME")).unwrap_or_else(|| "user".into());
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_owned());
    LaunchInfo {
        shell: builder(env).get_shell(),
        args: Vec::new(),
        cwd,
        user,
        host: host.ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "localhost".into()),
    }
}

fn builder(env: &[(OsString, OsString)]) -> CommandBuilder {
    let mut b = CommandBuilder::new_default_prog();
    b.env_clear();
    for (k, v) in env {
        b.env(k, v);
    }
    b
}

/// The shell's environment: ours without the AppImage runtime's variables and path entries,
/// plus `TERM` and `COLORTERM`. Values are never logged.
pub fn child_env(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vars.collect();
    let appdir = env
        .iter()
        .any(|(k, _)| k == "APPIMAGE")
        .then(|| env.iter().find(|(k, _)| k == "APPDIR").map(|(_, v)| v.to_string_lossy().into_owned()))
        .flatten()
        .filter(|d| !d.is_empty());
    if let Some(appdir) = appdir {
        env.retain(|(k, _)| !APPIMAGE_VARS.iter().any(|v| k == OsStr::new(v)));
        env.retain_mut(|(_, v)| {
            let Some(s) = v.to_str().filter(|s| s.contains(appdir.as_str())) else { return true };
            let kept = s.split(':').filter(|p| !p.starts_with(appdir.as_str())).collect::<Vec<_>>().join(":");
            let keep = !kept.is_empty();
            *v = kept.into();
            keep
        });
    }
    env.retain(|(k, _)| k != "TERM" && k != "COLORTERM");
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push(("COLORTERM".into(), "truecolor".into()));
    env
}

struct Reader(Box<dyn Read + Send>);

impl PtyReader for Reader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.0.read(buf) {
            Ok(n) => Ok(n),
            // EIO: every process closed the terminal (end of output).
            Err(e) if e.raw_os_error() == Some(nix::libc::EIO) => Ok(0),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(self.read(buf)?),
            Err(e) => Err(Error::Io(e.to_string())),
        }
    }
}

struct Writer(Box<dyn Write + Send>);

impl PtyWriter for Writer {
    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.0.write_all(bytes).and_then(|()| self.0.flush()).map_err(|e| Error::Io(e.to_string()))
    }
}

struct Control {
    pid: i32,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl PtyControl for Control {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = guard(&self.master);
        let Some(m) = master.as_ref() else { return Ok(()) };
        m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| Error::Io(e.to_string()))
    }

    fn close(&self) {
        let Some(master) = guard(&self.master).take() else { return };
        let mut groups = session_groups(self.pid);
        signal(&groups, Signal::SIGHUP);
        let pid = self.pid;
        // The child stays reaped by `wait_exit`/the reaper below; nothing here blocks the caller.
        let child = std::mem::replace(&mut *guard(&self.child), Box::new(Reaped));
        let _ = std::thread::Builder::new().name("rxt-pty-close".into()).spawn(move || {
            let mut child = child;
            let deadline = Instant::now() + KILL_GRACE;
            while Instant::now() < deadline && !session_groups(pid).is_empty() {
                std::thread::sleep(Duration::from_millis(50));
            }
            groups.extend(session_groups(pid));
            groups.dedup();
            signal(&groups, Signal::SIGKILL);
            let _ = child.wait();
            drop(master);
        });
    }

    fn wait_exit(&self, timeout: Duration) -> Option<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = guard(&self.child).try_wait() {
                return Some(status.exit_code());
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.close();
    }
}

/// Stand-in for the child after `close` handed it to the reaper thread.
#[derive(Debug)]
struct Reaped;

impl portable_pty::ChildKiller for Reaped {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(Reaped)
    }
}

impl Child for Reaped {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(Some(portable_pty::ExitStatus::with_exit_code(129)))
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        Ok(portable_pty::ExitStatus::with_exit_code(129))
    }
    fn process_id(&self) -> Option<u32> {
        None
    }
}

fn signal(groups: &[i32], sig: Signal) {
    for g in groups {
        let _ = killpg(Pid::from_raw(*g), sig);
    }
}

/// Process groups of the session led by `sid` (the shell), from `/proc`.
fn session_groups(sid: i32) -> Vec<i32> {
    let mut groups = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return groups };
    for entry in dir.flatten() {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        if let Some((pgrp, session)) = parse_stat(&stat)
            && session == sid
            && !groups.contains(&pgrp)
        {
            groups.push(pgrp);
        }
    }
    groups.sort_unstable();
    groups
}

/// (process group, session) from a `/proc/<pid>/stat` line; the command may contain spaces and
/// parentheses, so fields are counted after the last `)`.
fn parse_stat(stat: &str) -> Option<(i32, i32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut f = rest.split_whitespace();
    let (_state, _ppid) = (f.next()?, f.next()?);
    Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        list.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    fn get<'a>(env: &'a [(OsString, OsString)], k: &str) -> Option<&'a str> {
        env.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.to_str())
    }

    #[test]
    fn appimage_variables_and_paths_are_removed() {
        let env = child_env(
            vars(&[
                ("APPIMAGE", "/opt/apps/rxt.AppImage"),
                ("APPDIR", "/tmp/.mount_rxt"),
                ("ARGV0", "rxt"),
                ("OWD", "/srv/a"),
                ("PATH", "/tmp/.mount_rxt/usr/bin:/usr/bin"),
                ("LD_LIBRARY_PATH", "/tmp/.mount_rxt/usr/lib"),
                ("HOME", "/srv/a"),
                ("TERM", "dumb"),
            ])
            .into_iter(),
        );
        for k in ["APPIMAGE", "APPDIR", "ARGV0", "OWD", "LD_LIBRARY_PATH"] {
            assert_eq!(get(&env, k), None, "{k}");
        }
        assert_eq!(get(&env, "PATH"), Some("/usr/bin"));
        assert_eq!(get(&env, "HOME"), Some("/srv/a"));
        assert_eq!((get(&env, "TERM"), get(&env, "COLORTERM")), (Some("xterm-256color"), Some("truecolor")));
    }

    #[test]
    fn without_appimage_the_environment_is_kept() {
        let env = child_env(vars(&[("APPDIR", "/opt/x"), ("PATH", "/opt/x/bin:/usr/bin")]).into_iter());
        assert_eq!((get(&env, "APPDIR"), get(&env, "PATH")), (Some("/opt/x"), Some("/opt/x/bin:/usr/bin")));
    }

    #[test]
    fn invalid_shell_and_home_fall_back() {
        let env = vars(&[("SHELL", "/nonexistent/shell"), ("HOME", "/nonexistent/home"), ("USER", "alice")]);
        let info = launch_info(&env);
        assert_ne!(info.shell, "/nonexistent/shell");
        assert!(Path::new(&info.shell).is_file(), "{info:?}");
        assert_eq!((info.cwd.as_str(), info.user.as_str()), ("/", "alice"));
        let ok = launch_info(&vars(&[("SHELL", "/bin/sh"), ("HOME", "/tmp")]));
        assert_eq!((ok.shell.as_str(), ok.cwd.as_str()), ("/bin/sh", "/tmp"));
    }

    #[test]
    fn stat_fields_after_the_command() {
        assert_eq!(parse_stat("42 (a (b) c) S 1 40 41 0"), Some((40, 41)));
        assert_eq!(parse_stat("garbage"), None);
    }
}
