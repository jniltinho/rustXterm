//! Local shell on Windows (local-shell spec, design decision 4): ConPTY plus a kill-on-close job
//! object the shell is put in at creation (`PROC_THREAD_ATTRIBUTE_JOB_LIST`), so nothing it starts
//! outlives the tab or the app. The pseudo-console functions are looked up at run time: Windows
//! before 10 1809 has none and must still start the app (`+` then opens New session).
//!
//! This file holds the crate's other `unsafe` besides [`crate::sys`] and `control.rs`; one
//! operation per block, each with a `// SAFETY:` comment.

use std::ffi::c_void;
use std::io;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use rxt_core::LaunchInfo;
use rxt_platform::{Error, LocalPty, LocalShell, PtyControl, PtyReader, PtyWriter, Result};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, S_OK, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{COORD, HPCON, PSEUDOCONSOLE_INHERIT_CURSOR};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentThreadId, GetExitCodeProcess, InitializeProcThreadAttributeList, OpenThread,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, THREAD_TERMINATE, UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::{DsrFilter, command_line, env_block, local_shell, sys::wide};

/// Output chunks buffered between the pipe reader and [`PtyReader::read`].
const CHUNKS: usize = 4;
/// Exit code given to processes ended by closing the tab.
const CLOSED: u32 = 129;

type CreateFn = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> i32;
type ResizeFn = unsafe extern "system" fn(HPCON, COORD) -> i32;
type CloseFn = unsafe extern "system" fn(HPCON);

#[derive(Clone, Copy)]
struct Api {
    create: CreateFn,
    resize: ResizeFn,
    close: CloseFn,
}

/// The pseudo-console functions of kernel32, `None` on Windows without ConPTY.
fn api() -> Option<Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    *API.get_or_init(|| {
        let kernel32 = wide("kernel32.dll");
        // SAFETY: NUL-terminated name; kernel32 is always loaded, the handle needs no release.
        let module = unsafe { GetModuleHandleW(kernel32.as_ptr()) };
        if module.is_null() {
            return None;
        }
        let find = |name: &[u8]| {
            // SAFETY: `module` is kernel32; `name` is a NUL-terminated ASCII export name.
            unsafe { GetProcAddress(module, name.as_ptr()) }
        };
        let (create, resize, close) =
            (find(b"CreatePseudoConsole\0")?, find(b"ResizePseudoConsole\0")?, find(b"ClosePseudoConsole\0")?);
        Some(Api {
            // SAFETY: documented signature of CreatePseudoConsole (consoleapi.h).
            create: unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateFn>(create) },
            // SAFETY: documented signature of ResizePseudoConsole (consoleapi.h).
            resize: unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, ResizeFn>(resize) },
            // SAFETY: documented signature of ClosePseudoConsole (consoleapi.h).
            close: unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, CloseFn>(close) },
        })
    })
}

/// Owned kernel handle, closed once.
struct Owned(HANDLE);

// SAFETY: kernel handles may be used and closed from any thread.
unsafe impl Send for Owned {}
// SAFETY: the calls made through `&Owned` (Read/WriteFile, waits) are thread-safe.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: `self.0` is a live handle owned by this wrapper.
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn io(context: &str, e: io::Error) -> Error {
    Error::Io(format!("{context}: {e}"))
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD { X: i16::try_from(cols).unwrap_or(i16::MAX), Y: i16::try_from(rows).unwrap_or(i16::MAX) }
}

fn pipe() -> io::Result<(Owned, Owned)> {
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: valid out pointers; default (non-inheritable) security attributes.
    check(unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) })?;
    Ok((Owned(read), Owned(write)))
}

/// Kill-on-close job; its handle is not inheritable (null security attributes).
fn job() -> io::Result<Owned> {
    // SAFETY: no security attributes, unnamed job.
    let job = Owned(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) });
    if job.0.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let size = u32::try_from(std::mem::size_of_val(&limits)).unwrap_or(u32::MAX);
    // SAFETY: `limits` is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION of `size` bytes.
    check(unsafe {
        SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size)
    })?;
    Ok(job)
}

/// `PROC_THREAD_ATTRIBUTE_LIST` with the pseudo-console and the job, kept alive with the values
/// it points to until `CreateProcessW` returned.
struct Attributes {
    buf: Vec<u64>,
    _job: Box<HANDLE>,
}

impl Attributes {
    fn new(hpc: HPCON, job: HANDLE) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: size query with a null list; only `size` is written (fails with
        // ERROR_INSUFFICIENT_BUFFER by design).
        let _ = unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut size) };
        let mut buf = vec![0u64; size.div_ceil(8)];
        let list = buf.as_mut_ptr().cast::<c_void>();
        // SAFETY: `buf` holds `size` writable, 8-byte aligned bytes.
        check(unsafe { InitializeProcThreadAttributeList(list, 2, 0, &mut size) })?;
        let job = Box::new(job);
        let attrs = Self { buf, _job: job };
        let list = attrs.list();
        // SAFETY: `list` was initialized above; the pseudo-console attribute takes the HPCON value
        // itself (not a pointer to it), as documented.
        check(unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const c_void,
                std::mem::size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null(),
            )
        })?;
        // SAFETY: `list` is initialized; `_job` points to one HANDLE that lives as long as `attrs`.
        check(unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                (&raw const *attrs._job).cast(),
                std::mem::size_of::<HANDLE>(),
                ptr::null_mut(),
                ptr::null(),
            )
        })?;
        Ok(attrs)
    }

    fn list(&self) -> *mut c_void {
        self.buf.as_ptr().cast_mut().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the list was initialized in `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.list()) };
    }
}

/// Windows local shells through ConPTY.
pub struct ConPty(Api);

impl ConPty {
    /// `None` on Windows without a pseudo-console (before 10 1809).
    pub fn detect() -> Option<Self> {
        api().map(Self)
    }
}

fn var(k: &str) -> Option<String> {
    std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty())
}

impl LocalPty for ConPty {
    fn prepare(&self) -> Result<LaunchInfo> {
        let root = var("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let path = std::env::var_os("PATH");
        let comspec = std::env::var_os("ComSpec");
        let (shell, args) = local_shell(path.as_deref(), Path::new(&root), comspec.as_deref(), Path::is_file)
            .ok_or_else(|| Error::NotFound("no shell (pwsh.exe, powershell.exe or %ComSpec%)".into()))?;
        let drive = var("SystemDrive").map_or_else(|| r"C:\".to_owned(), |d| format!(r"{d}\"));
        let cwd = var("USERPROFILE").filter(|h| Path::new(h).is_dir()).unwrap_or(drive);
        Ok(LaunchInfo {
            shell: shell.display().to_string(),
            args,
            cwd,
            user: var("USERNAME").unwrap_or_else(|| "user".into()),
            host: var("COMPUTERNAME").unwrap_or_else(|| "localhost".into()),
            ..LaunchInfo::default()
        })
    }

    fn spawn(&self, info: &LaunchInfo, cols: u16, rows: u16, cursor: (u16, u16)) -> Result<LocalShell> {
        let api = self.0;
        let (in_read, in_write) = pipe().map_err(|e| io("input pipe", e))?;
        let (out_read, out_write) = pipe().map_err(|e| io("output pipe", e))?;
        let mut hpc: HPCON = 0;
        // SAFETY: `api.create` is CreatePseudoConsole; the pipe ends are live; `hpc` is an out
        // pointer.
        let hr =
            unsafe { (api.create)(coord(cols, rows), in_read.0, out_write.0, PSEUDOCONSOLE_INHERIT_CURSOR, &mut hpc) };
        if hr != S_OK {
            return Err(Error::Io(format!("cannot create a pseudo-console (HRESULT {hr:#x})")));
        }
        let console = Console { api, hpc: Mutex::new(Some(hpc)) };
        // The pseudo-console holds its own copies of its pipe ends.
        drop((in_read, out_write));
        let job = job().map_err(|e| io("job object", e))?;
        let attrs = Attributes::new(hpc, job.0).map_err(|e| io("process attributes", e))?;

        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).unwrap_or(u32::MAX);
        // No inherited standard handles: the shell's console is the pseudo-console.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        si.lpAttributeList = attrs.list();
        let mut cmdline = wide(command_line(&info.shell, &info.args));
        let cwd = wide(&info.cwd);
        let env = env_block(
            std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())),
        );
        let mut pi = PROCESS_INFORMATION::default();
        // SAFETY: every pointer is valid for the call: the mutable command line, the environment
        // block (UTF-16, double-NUL terminated), the folder, the extended startup info with its
        // live attribute list, and `pi`. Handles are not inherited; the job list puts the shell
        // in `job` before it runs, and a failed assignment fails the call (fail closed).
        check(unsafe {
            CreateProcessW(
                ptr::null(),
                cmdline.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env.as_ptr().cast(),
                cwd.as_ptr(),
                (&raw const si.StartupInfo),
                &mut pi,
            )
        })
        .map_err(|e| io(&format!("cannot start {}", info.shell), e))?;
        drop(attrs);
        let (process, _thread) = (Owned(pi.hProcess), Owned(pi.hThread));

        let input = Arc::new(Input { pipe: in_write, busy: Mutex::new(None) });
        let reply = format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1).into_bytes();
        let shared = Arc::new(Shared {
            console,
            job,
            process,
            input: input.clone(),
            closed: AtomicBool::new(false),
            dsr: Mutex::new(DsrFilter::default()),
            reply,
        });
        let (tx, rx) = sync_channel(CHUNKS);
        let pump = shared.clone();
        std::thread::Builder::new()
            .name("rxt-conpty-read".into())
            .spawn(move || pump.read_loop(&out_read, &tx))
            .map_err(|e| io("output reader", e))?;
        Ok(LocalShell {
            reader: Box::new(Reader { rx, chunk: Vec::new(), pos: 0 }),
            writer: Box::new(Writer(input)),
            control: Arc::new(Control(shared)),
        })
    }
}

/// The pseudo-console handle, closed once by teardown.
struct Console {
    api: Api,
    hpc: Mutex<Option<HPCON>>,
}

/// Input pipe; `busy` holds the thread blocked in `WriteFile`, so teardown can cancel it.
struct Input {
    pipe: Owned,
    busy: Mutex<Option<Owned>>,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Input {
    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        // SAFETY: no preconditions.
        let id = unsafe { GetCurrentThreadId() };
        // SAFETY: opens this thread with the access CancelSynchronousIo needs.
        let me = Owned(unsafe { OpenThread(THREAD_TERMINATE, 0, id) });
        *guard(&self.busy) = (!me.0.is_null()).then_some(me);
        let result = self.write_raw(bytes);
        *guard(&self.busy) = None;
        result
    }

    /// Write without registering for cancellation (the adapter's own short replies).
    fn write_raw(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
            let mut done = 0u32;
            // SAFETY: `bytes` is readable for `len` bytes; synchronous write on a live pipe.
            check(unsafe { WriteFile(self.pipe.0, bytes.as_ptr(), len, &mut done, ptr::null_mut()) })?;
            bytes = &bytes[done as usize..];
        }
        Ok(())
    }

    /// End a write blocked on a shell that does not read.
    fn cancel(&self) {
        if let Some(t) = guard(&self.busy).as_ref() {
            // SAFETY: `t` is a live handle with THREAD_TERMINATE to the writing thread.
            unsafe { CancelSynchronousIo(t.0) };
        }
    }
}

struct Shared {
    console: Console,
    job: Owned,
    process: Owned,
    input: Arc<Input>,
    closed: AtomicBool,
    dsr: Mutex<DsrFilter>,
    /// Answer to ConPTY's start-up cursor request: the cursor after the banner.
    reply: Vec<u8>,
}

impl Shared {
    /// Drain the output pipe until the pseudo-console closes, answering its cursor request.
    /// Keeps reading after the tab dropped its reader (ConPTY must be drained to close).
    fn read_loop(&self, pipe: &Owned, tx: &SyncSender<Vec<u8>>) {
        let mut buf = vec![0u8; 8192];
        let mut sink = false;
        loop {
            let mut n = 0u32;
            let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            // SAFETY: `buf` is writable for `len` bytes; synchronous read on a live pipe.
            if unsafe { ReadFile(pipe.0, buf.as_mut_ptr(), len, &mut n, ptr::null_mut()) } == 0 || n == 0 {
                return;
            }
            let (out, found) = guard(&self.dsr).feed(&buf[..n as usize]);
            if found {
                let _ = self.input.write_raw(&self.reply);
            }
            if sink || out.is_empty() {
                continue;
            }
            // Blocks while the tab is not reading (backpressure), unless closing.
            let mut chunk = out;
            loop {
                match tx.try_send(chunk) {
                    Ok(()) => break,
                    Err(TrySendError::Disconnected(_)) => {
                        sink = true;
                        break;
                    }
                    Err(TrySendError::Full(c)) if self.closed.load(Ordering::Acquire) => {
                        drop(c);
                        sink = true;
                        break;
                    }
                    Err(TrySendError::Full(c)) => {
                        chunk = c;
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        }
    }

    /// Teardown (design decision 4): answer a pending cursor request, cancel a blocked write,
    /// end the job (the whole tree), then close the pseudo-console while `read_loop` drains it.
    fn teardown(&self) {
        if !guard(&self.dsr).answered() {
            let _ = self.input.write_raw(&self.reply);
        }
        for _ in 0..20 {
            self.input.cancel();
            if guard(&self.input.busy).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: `job` is the live kill-on-close job of this shell.
        unsafe { TerminateJobObject(self.job.0, CLOSED) };
        if let Some(hpc) = guard(&self.console.hpc).take() {
            // SAFETY: `hpc` is the live pseudo-console, closed exactly once here.
            unsafe { (self.console.api.close)(hpc) };
        }
    }
}

struct Reader {
    rx: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl PtyReader for Reader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pos >= self.chunk.len() {
            match self.rx.recv() {
                Ok(c) => (self.chunk, self.pos) = (c, 0),
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.pos);
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct Writer(Arc<Input>);

impl PtyWriter for Writer {
    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.0.write_all(bytes).map_err(|e| io("shell input", e))
    }
}

struct Control(Arc<Shared>);

impl PtyControl for Control {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let hpc = *guard(&self.0.console.hpc);
        let Some(hpc) = hpc else { return Ok(()) };
        // SAFETY: `hpc` is the live pseudo-console (teardown takes it under the same lock order).
        let hr = unsafe { (self.0.console.api.resize)(hpc, coord(cols, rows)) };
        if hr == S_OK { Ok(()) } else { Err(Error::Io(format!("resize failed (HRESULT {hr:#x})"))) }
    }

    fn close(&self) {
        if self.0.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let shared = self.0.clone();
        // Returns at once; the teardown thread owns the remaining handles.
        let spawned = std::thread::Builder::new().name("rxt-conpty-close".into()).spawn(move || shared.teardown());
        if spawned.is_err() {
            self.0.teardown();
        }
    }

    fn wait_exit(&self, timeout: Duration) -> Option<u32> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: `process` is the live shell process handle.
        if unsafe { WaitForSingleObject(self.0.process.0, ms) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: live process handle; `code` is a valid out pointer.
        let ok = unsafe { GetExitCodeProcess(self.0.process.0, &mut code) };
        Some(if ok == 0 { CLOSED } else { code })
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    //! Native tests (CI `windows-test` job): real shells through ConPTY.
    use super::*;
    use std::time::Instant;

    fn cmd() -> LaunchInfo {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        LaunchInfo { shell: comspec, args: vec!["/Q".into()], ..ConPty::detect().unwrap().prepare().unwrap() }
    }

    fn read_until(r: &mut Box<dyn PtyReader>, needle: &str) -> String {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut reader = std::mem::replace(r, Box::new(Reader { rx: sync_channel(1).1, chunk: Vec::new(), pos: 0 }));
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut out = String::new();
        while !out.contains(needle) {
            let left =
                deadline.checked_duration_since(Instant::now()).unwrap_or_else(|| panic!("no {needle:?} in {out:?}"));
            if let Ok(b) = rx.recv_timeout(left) {
                out.push_str(&String::from_utf8_lossy(&b));
            }
        }
        out
    }

    fn running(image: &str) -> bool {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {image}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_ascii_lowercase().contains(&image.to_ascii_lowercase())
    }

    #[test]
    fn prepare_finds_a_shell_in_the_profile() {
        let info = ConPty::detect().expect("ConPTY on the CI runner").prepare().unwrap();
        assert!(Path::new(&info.shell).is_file(), "{info:?}");
        assert!(Path::new(&info.cwd).is_dir(), "{info:?}");
    }

    #[test]
    fn echo_term_unicode_and_exit_code() {
        let pty = ConPty::detect().unwrap();
        let mut sh = pty.spawn(&cmd(), 80, 24, (2, 0)).unwrap();
        sh.writer.write_all("echo T=%TERM% ção\r\n".as_bytes()).unwrap();
        let out = read_until(&mut sh.reader, "T=xterm-256color");
        assert!(!out.contains("\x1b[6n"), "cursor request answered by the adapter");
        sh.control.resize(100, 30).unwrap();
        sh.writer.write_all(b"exit 3\r\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let code = loop {
            if let Some(c) = sh.control.wait_exit(Duration::from_millis(200)) {
                break c;
            }
            assert!(Instant::now() < deadline);
        };
        assert_eq!(code, 3);
    }

    #[test]
    fn close_ends_the_process_tree() {
        let pty = ConPty::detect().unwrap();
        let mut sh = pty.spawn(&cmd(), 80, 24, (0, 0)).unwrap();
        sh.writer.write_all(b"start /b ping -n 1000 127.0.0.1 >NUL\r\necho STARTED\r\n").unwrap();
        read_until(&mut sh.reader, "STARTED");
        std::thread::sleep(Duration::from_secs(1));
        assert!(running("PING.EXE"));
        let t = Instant::now();
        sh.control.close();
        assert!(t.elapsed() < Duration::from_millis(500));
        let deadline = Instant::now() + Duration::from_secs(5);
        while running("PING.EXE") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(!running("PING.EXE"), "the job ended the child");
    }

    #[test]
    fn close_right_after_spawn_and_with_a_blocked_write() {
        let pty = ConPty::detect().unwrap();
        let sh = pty.spawn(&cmd(), 80, 24, (0, 0)).unwrap();
        sh.control.close();
        let LocalShell { mut writer, control, .. } = pty.spawn(&cmd(), 80, 24, (0, 0)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let chunk = vec![b'x'; 1 << 20];
            let mut r = Ok(());
            for _ in 0..64 {
                r = writer.write_all(&chunk);
                if r.is_err() {
                    break;
                }
            }
            let _ = tx.send(r.is_err());
        });
        std::thread::sleep(Duration::from_secs(1));
        control.close();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "writer thread still blocked");
    }
}
