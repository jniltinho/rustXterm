//! Safe wrappers over the Win32 calls the adapter needs. All `unsafe` of the crate is here
//! (plus the named-pipe creation in `control.rs` and the pseudo-console in `conpty.rs`); one
//! operation per block.

use std::ffi::{OsStr, c_void};
use std::io;
use std::num::NonZeroIsize;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::RawHandle;
use std::path::Path;
use std::ptr;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NOT_FOUND, ERROR_SUCCESS, HANDLE, LocalFree, WIN32_ERROR};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation,
    IsTokenRestricted, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    RevertToSelf, SID_AND_ATTRIBUTES, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TOKEN_USER, TokenGroups, TokenIntegrityLevel, TokenIsAppContainer, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, GetDriveTypeW, GetLongPathNameW, GetVolumeInformationW,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeServerProcessId, ImpersonateNamedPipeClient};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows_sys::core::{BOOL, PWSTR};

/// NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn check(ok: BOOL) -> io::Result<()> {
    if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn win32(code: WIN32_ERROR) -> io::Result<()> {
    if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) }
}

/// Copy of a NUL-terminated UTF-16 string returned by an API.
fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    loop {
        // SAFETY: `p` is a NUL-terminated string from the API and `len` never passes the NUL.
        let c = unsafe { *p.wrapping_add(len) };
        if c == 0 {
            break;
        }
        len += 1;
    }
    // SAFETY: the `len` units before the NUL were just read, so they are initialized.
    let units = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf16_lossy(units)
}

/// Owned kernel handle.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a valid handle owned by this wrapper and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Memory an API allocated with `LocalAlloc`.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was returned by an API documented to allocate with LocalAlloc.
            unsafe { LocalFree(self.0) };
        }
    }
}

fn open_process_token(process: HANDLE) -> io::Result<Handle> {
    open_process_token_with(process, TOKEN_QUERY)
}

fn open_process_token_with(process: HANDLE, access: u32) -> io::Result<Handle> {
    let mut token = ptr::null_mut();
    // SAFETY: `process` is a live process handle; `token` is a valid out pointer.
    check(unsafe { OpenProcessToken(process, access, &mut token) })?;
    Ok(Handle(token))
}

/// Token information in an 8-byte aligned buffer (the structures hold pointers).
fn token_info(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    let mut len = 0u32;
    // SAFETY: size query: a null buffer of length 0, only `len` is written.
    let _ = unsafe { GetTokenInformation(token.0, class, ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` writable bytes and is 8-byte aligned.
    check(unsafe { GetTokenInformation(token.0, class, buf.as_mut_ptr().cast(), len, &mut len) })?;
    Ok(buf)
}

/// String form of a SID that the caller keeps alive for the call.
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text: PWSTR = ptr::null_mut();
    // SAFETY: `sid` is valid for the call (caller contract); `text` receives a LocalAlloc'd string.
    check(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let text = Local(text.cast());
    Ok(from_wide(text.0.cast()))
}

fn token_sid(token: &Handle) -> io::Result<String> {
    let buf = token_info(token, TokenUser)?;
    let user = buf.as_ptr().cast::<TOKEN_USER>();
    // SAFETY: GetTokenInformation(TokenUser) filled `buf` with a TOKEN_USER; `buf` is alive.
    let sid = unsafe { (*user).User.Sid };
    sid_string(sid)
}

/// Mandatory integrity level RID (e.g. 0x2000 medium) from the token's label SID `S-1-16-<rid>`.
fn token_integrity(token: &Handle) -> io::Result<u32> {
    let buf = token_info(token, TokenIntegrityLevel)?;
    let label = buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
    // SAFETY: GetTokenInformation(TokenIntegrityLevel) filled `buf` with a TOKEN_MANDATORY_LABEL.
    let sid = unsafe { (*label).Label.Sid };
    let text = sid_string(sid)?;
    crate::integrity_rid(&text).ok_or_else(|| io::Error::other(format!("unexpected integrity label {text}")))
}

fn token_restricted(token: &Handle) -> io::Result<bool> {
    // SAFETY: `token` is a live token handle opened with TOKEN_QUERY.
    if unsafe { IsTokenRestricted(token.0) } != 0 {
        return Ok(true);
    }
    // TokenIsAppContainer is a DWORD: the low half of the first (little-endian) u64.
    Ok(token_info(token, TokenIsAppContainer)?.first().is_some_and(|v| (*v as u32) != 0))
}

/// `SE_GROUP_ENABLED` and `SE_GROUP_USE_FOR_DENY_ONLY` group attributes.
const SE_GROUP_ENABLED: u32 = 0x4;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x10;

/// (enabled, deny-only) group SIDs of the token.
fn token_groups(token: &Handle) -> io::Result<(Vec<String>, Vec<String>)> {
    let buf = token_info(token, TokenGroups)?;
    let groups = buf.as_ptr().cast::<TOKEN_GROUPS>();
    // SAFETY: GetTokenInformation(TokenGroups) filled `buf` with a TOKEN_GROUPS.
    let count = unsafe { (*groups).GroupCount } as usize;
    // SAFETY: as above; only the address of the trailing array is taken.
    let first = unsafe { &raw const (*groups).Groups };
    // SAFETY: the API wrote `count` SID_AND_ATTRIBUTES entries starting there, inside `buf`.
    let entries = unsafe { std::slice::from_raw_parts(first.cast::<SID_AND_ATTRIBUTES>(), count) };
    let (mut enabled, mut deny_only) = (Vec::new(), Vec::new());
    for g in entries {
        if g.Attributes & SE_GROUP_USE_FOR_DENY_ONLY != 0 {
            deny_only.push(sid_string(g.Sid)?);
        } else if g.Attributes & SE_GROUP_ENABLED != 0 {
            enabled.push(sid_string(g.Sid)?);
        }
    }
    Ok((enabled, deny_only))
}

fn identity(token: &Handle) -> io::Result<crate::Identity> {
    let (enabled, deny_only) = token_groups(token)?;
    Ok(crate::Identity {
        sid: token_sid(token)?,
        restricted: token_restricted(token)?,
        integrity: token_integrity(token)?,
        enabled,
        deny_only,
    })
}

/// Identity of this process.
pub fn current_identity() -> io::Result<crate::Identity> {
    // SAFETY: returns a pseudo handle; no preconditions and nothing to close.
    let me = unsafe { GetCurrentProcess() };
    identity(&open_process_token(me)?)
}

/// Identity of process `pid` (fails when we may not query it, e.g. another user's process).
pub fn process_identity(pid: u32) -> io::Result<crate::Identity> {
    // SAFETY: plain call; a non-null result is owned by `Handle` below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(h);
    identity(&open_process_token(process.0)?)
}

/// Identity of the security context the client connected with (design W4): impersonate the
/// client on this thread, open the thread token, revert — synchronously, nothing in between.
/// Windows allows this only after data was read from the pipe.
pub fn pipe_client_identity(pipe: RawHandle) -> io::Result<crate::Identity> {
    // SAFETY: returns a pseudo handle for the current thread; no preconditions.
    let thread = unsafe { GetCurrentThread() };
    // SAFETY: `pipe` is a live, connected server instance borrowed from its owner.
    check(unsafe { ImpersonateNamedPipeClient(pipe) })?;
    let mut token = ptr::null_mut();
    // SAFETY: `thread` is the current thread; `token` is a valid out pointer. OpenAsSelf = TRUE
    // checks access against the process token, not the impersonated one.
    let opened = check(unsafe { OpenThreadToken(thread, TOKEN_QUERY, 1, &mut token) });
    // SAFETY: ends the impersonation started above on this same thread.
    if unsafe { RevertToSelf() } == 0 {
        // Never keep running with a client's identity.
        std::process::abort();
    }
    opened?;
    identity(&Handle(token))
}

/// Process id of the server of client pipe `pipe`.
pub fn pipe_server_pid(pipe: RawHandle) -> io::Result<u32> {
    let mut pid = 0;
    // SAFETY: `pipe` is a live client pipe handle borrowed from its owner.
    check(unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) })?;
    Ok(pid)
}

/// Self-relative security descriptor built from SDDL.
pub struct SecurityDescriptor(Local);

impl SecurityDescriptor {
    pub fn from_sddl(sddl: &str) -> io::Result<Self> {
        let text = wide(sddl);
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `text` is NUL-terminated; `sd` receives a LocalAlloc'd descriptor.
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                ptr::null_mut(),
            )
        })?;
        Ok(Self(Local(sd)))
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.0.0
    }

    fn dacl(&self) -> io::Result<*mut ACL> {
        let (mut present, mut defaulted, mut acl) = (0, 0, ptr::null_mut());
        // SAFETY: `self` holds a valid descriptor; the out pointers are valid locals.
        check(unsafe { GetSecurityDescriptorDacl(self.as_ptr(), &mut present, &mut acl, &mut defaulted) })?;
        if present == 0 || acl.is_null() {
            return Err(io::Error::other("security descriptor has no DACL"));
        }
        Ok(acl)
    }
    fn owner(&self) -> io::Result<PSID> {
        let (mut owner, mut defaulted) = (ptr::null_mut(), 0);
        // SAFETY: `self` holds a valid descriptor; the out pointers are valid locals.
        check(unsafe { GetSecurityDescriptorOwner(self.as_ptr(), &mut owner, &mut defaulted) })?;
        if owner.is_null() {
            return Err(io::Error::other("security descriptor has no owner"));
        }
        Ok(owner)
    }
}

/// Apply owner and protected DACL from `sddl` (`O:<sid>D:P(...)`) to `path`. The DACL goes
/// first so the owner change is allowed by it; the owner is only written when it differs.
pub fn set_owner_and_protected_dacl(path: &Path, sddl: &str, owner_is_set: bool) -> io::Result<()> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let dacl = sd.dacl()?;
    let name = wide(path);
    let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
    // SAFETY: `name` is NUL-terminated; `dacl` points into `sd`, alive for the call.
    win32(unsafe {
        SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, info, ptr::null_mut(), ptr::null_mut(), dacl, ptr::null())
    })?;
    if owner_is_set {
        return Ok(());
    }
    let owner = sd.owner()?;
    // SAFETY: `name` is NUL-terminated; `owner` points into `sd`, alive for the call.
    win32(unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            owner,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
        )
    })
}

/// Create directory `path` with the security descriptor `sddl` from the start (no window with
/// an inherited ACL). Fails if it already exists.
pub fn create_dir_with(path: &Path, sddl: &str) -> io::Result<()> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let attrs = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        nLength: size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.as_ptr(),
        bInheritHandle: 0,
    };
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated; `attrs` and its descriptor `sd` are alive for the call.
    check(unsafe { CreateDirectoryW(name.as_ptr(), &attrs) })
}

/// Owner and DACL of `path` as SDDL (for the read-back check).
pub fn owner_dacl_sddl(path: &Path) -> io::Result<String> {
    let name = wide(path);
    let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let (no_sid, no_acl) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: `name` is NUL-terminated; only `sd` is requested and receives a LocalAlloc'd copy.
    win32(unsafe {
        GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, info, no_sid, no_sid, no_acl, no_acl, &mut sd)
    })?;
    let sd = Local(sd);
    let mut text: PWSTR = ptr::null_mut();
    // SAFETY: `sd` is the descriptor just returned; `text` receives a LocalAlloc'd string.
    check(unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(sd.0, SDDL_REVISION_1, info, &mut text, ptr::null_mut())
    })?;
    let text = Local(text.cast());
    Ok(from_wide(text.0.cast()))
}

/// `GetDriveTypeW` result for a remote (network) drive.
const DRIVE_REMOTE: u32 = 4;
/// `GetVolumeInformationW` flag: the file system preserves and enforces ACLs.
const FILE_PERSISTENT_ACLS: u32 = 8;

/// Whether the volume with root `root` (e.g. `C:\`) is local and keeps ACLs.
pub fn volume_is_local_with_acls(root: &Path) -> io::Result<bool> {
    let name = wide(root);
    // SAFETY: `name` is a NUL-terminated root path.
    if unsafe { GetDriveTypeW(name.as_ptr()) } == DRIVE_REMOTE {
        return Ok(false);
    }
    let mut flags = 0u32;
    let null = ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; only `flags` is requested, other buffers are null/0.
    check(unsafe { GetVolumeInformationW(name.as_ptr(), null, 0, null.cast(), null.cast(), &mut flags, null, 0) })?;
    Ok(flags & FILE_PERSISTENT_ACLS != 0)
}

/// `path` (which must exist) with 8.3 short names such as `RUNNER~1` expanded. Unlike
/// canonicalization this does not resolve links or junctions.
pub fn long_path(path: &Path) -> io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let name = wide(path);
    let mut buf = vec![0u16; 260];
    loop {
        let cap = u32::try_from(buf.len()).map_err(|_| io::Error::other("path too long"))?;
        // SAFETY: `name` is NUL-terminated; `buf` is writable for `cap` UTF-16 units.
        let n = unsafe { GetLongPathNameW(name.as_ptr(), buf.as_mut_ptr(), cap) } as usize;
        match n {
            0 => return Err(io::Error::last_os_error()),
            n if n < buf.len() => return Ok(std::ffi::OsString::from_wide(&buf[..n]).into()),
            n => buf.resize(n, 0),
        }
    }
}

/// Credential returned by `CredReadW`.
struct Credential(*mut CREDENTIALW);

impl Drop for Credential {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful CredReadW and is freed once.
        unsafe { CredFree(self.0.cast()) };
    }
}

/// Generic credential blob for `target`; `None` when absent.
pub fn cred_read(target: &str) -> io::Result<Option<Vec<u8>>> {
    let name = wide(target);
    let mut cred: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; `cred` receives a CredFree-owned buffer.
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) } == 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(ERROR_NOT_FOUND as i32) { Ok(None) } else { Err(e) };
    }
    let cred = Credential(cred);
    // SAFETY: CredReadW succeeded, so `cred.0` points to a valid CREDENTIALW until freed.
    let c = unsafe { &*cred.0 };
    let (blob, size) = (c.CredentialBlob, c.CredentialBlobSize);
    if blob.is_null() || size == 0 {
        return Ok(Some(Vec::new()));
    }
    // SAFETY: the API guarantees `size` readable bytes at `blob` while `cred` is alive.
    let bytes = unsafe { std::slice::from_raw_parts(blob, size as usize) };
    Ok(Some(bytes.to_vec()))
}

/// Create or replace generic credential `target` (local-machine persistence: does not roam).
pub fn cred_write(target: &str, user: &str, blob: &[u8]) -> io::Result<()> {
    let mut name = wide(target);
    let mut user = wide(user);
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        UserName: user.as_mut_ptr(),
        CredentialBlobSize: u32::try_from(blob.len()).map_err(io::Error::other)?,
        CredentialBlob: blob.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..CREDENTIALW::default()
    };
    // SAFETY: every pointer in `cred` refers to a buffer alive for the call; CredWriteW only reads.
    check(unsafe { CredWriteW(&cred, 0) })
}

/// Delete generic credential `target`; absent is not an error.
pub fn cred_delete(target: &str) -> io::Result<()> {
    let name = wide(target);
    // SAFETY: `name` is NUL-terminated.
    match check(unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) }) {
        Err(e) if e.raw_os_error() == Some(ERROR_NOT_FOUND as i32) => Ok(()),
        other => other,
    }
}

/// `HKCU\<subkey>\<value>` as a DWORD.
pub fn reg_dword(subkey: &str, value: &str) -> io::Result<u32> {
    let (key, name) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    let out: *mut u32 = &mut data;
    // SAFETY: `key`/`name` are NUL-terminated; `out`/`size` describe a writable 4-byte buffer.
    win32(unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            out.cast(),
            &mut size,
        )
    })?;
    Ok(data)
}

/// Registry root hives for the generic helpers below.
pub const HKCU: HKEY = HKEY_CURRENT_USER;
pub const HKLM: HKEY = HKEY_LOCAL_MACHINE;

/// An open registry key, closed on drop.
struct RegKey(HKEY);

impl RegKey {
    fn open(root: HKEY, subkey: &str) -> io::Result<Self> {
        let key = wide(subkey);
        let mut h: HKEY = ptr::null_mut();
        // SAFETY: `key` is NUL-terminated; `h` receives a key handle owned by `RegKey`.
        win32(unsafe { RegOpenKeyExW(root, key.as_ptr(), 0, KEY_READ, &mut h) })?;
        Ok(Self(h))
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a key opened by `RegKey::open`, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// `<root>\<subkey>\<value>` as a string (REG_SZ), empty entries included.
pub fn reg_sz(root: HKEY, subkey: &str, value: &str) -> io::Result<String> {
    let (key, name) = (wide(subkey), wide(value));
    let mut size = 0u32;
    // SAFETY: size query with a null buffer; only `size` is written.
    win32(unsafe {
        RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), ptr::null_mut(), &mut size)
    })?;
    let mut buf = vec![0u16; (size as usize).div_ceil(2).max(1)];
    // SAFETY: `buf` holds at least `size` writable bytes; `key`/`name` are NUL-terminated.
    win32(unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    })?;
    Ok(from_wide(buf.as_ptr()))
}

/// `<root>\<subkey>\<value>` as a DWORD.
pub fn reg_dword_at(root: HKEY, subkey: &str, value: &str) -> io::Result<u32> {
    let (key, name) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    let out: *mut u32 = &mut data;
    // SAFETY: `key`/`name` are NUL-terminated; `out`/`size` describe a writable 4-byte buffer.
    win32(unsafe {
        RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, ptr::null_mut(), out.cast(), &mut size)
    })?;
    Ok(data)
}

/// Names of the immediate subkeys of `<root>\<subkey>`.
pub fn reg_subkeys(root: HKEY, subkey: &str) -> io::Result<Vec<String>> {
    let key = RegKey::open(root, subkey)?;
    let mut names = Vec::new();
    for i in 0u32.. {
        // WSL distribution subkeys are GUIDs; 256 units is ample for any subkey name.
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: `key.0` is an open key; `buf`/`len` describe a writable buffer; other out
        // parameters are null (not requested).
        let rc = unsafe {
            RegEnumKeyExW(
                key.0,
                i,
                buf.as_mut_ptr(),
                &mut len,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if rc == ERROR_NO_MORE_ITEMS {
            break;
        }
        win32(rc)?;
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    Ok(names)
}

/// Whether `<root>\<subkey>` exists and opens for reading.
pub fn reg_key_exists(root: HKEY, subkey: &str) -> bool {
    RegKey::open(root, subkey).is_ok()
}

/// An open `HKCU` key watched for value changes.
pub struct RegWatch(HKEY);

impl RegWatch {
    pub fn open(subkey: &str) -> io::Result<Self> {
        let key = wide(subkey);
        let mut h: HKEY = ptr::null_mut();
        // SAFETY: `key` is NUL-terminated; `h` receives a key handle owned by `RegWatch`.
        win32(unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_NOTIFY, &mut h) })?;
        Ok(Self(h))
    }

    /// Block until a value of the key changes.
    pub fn wait(&self) -> io::Result<()> {
        // SAFETY: `self.0` is an open key with KEY_NOTIFY; synchronous wait, no event handle.
        win32(unsafe { RegNotifyChangeKeyValue(self.0, 0, REG_NOTIFY_CHANGE_LAST_SET, ptr::null_mut(), 0) })
    }
}

impl Drop for RegWatch {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a key opened by `RegWatch::open`, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// `SE_ERR_NOASSOC`: no application is associated with the file type.
const SE_ERR_NOASSOC: isize = 31;

/// Open `target` with its default handler (`ShellExecuteW`, verb `open`).
pub fn shell_open(target: &OsStr) -> io::Result<()> {
    let (verb, file) = (wide("open"), wide(target));
    // SAFETY: both strings are NUL-terminated; no owner window, parameters or directory.
    let r = unsafe {
        ShellExecuteW(ptr::null_mut(), verb.as_ptr(), file.as_ptr(), ptr::null(), ptr::null(), SW_SHOWNORMAL)
    };
    match r as isize {
        code if code > 32 => Ok(()),
        SE_ERR_NOASSOC => Err(io::Error::other("no application is associated with this file type")),
        0 => Err(io::Error::other("out of memory or resources")),
        code => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

/// Borrowed main-window HWND used as the owner of file dialogs.
pub struct ParentWindow(pub NonZeroIsize);

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // SAFETY: the HWND comes from the live main window (eframe reports it every frame) and
        // the dialog that borrows it is modal to that window, so it outlives the borrow.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for ParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

/// Tokens for the pipe refusal tests: a restricted token and a low-integrity token of the
/// current user, impersonated on the calling thread.
#[cfg(test)]
pub mod test_tokens {
    use super::*;
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, DuplicateTokenEx, ImpersonateLoggedOnUser, SecurityImpersonation,
        SetTokenInformation, TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TokenImpersonation,
    };

    pub struct Token(Handle);
    // SAFETY: a token handle may be used from any thread.
    unsafe impl Send for Token {}

    fn own_token() -> io::Result<Handle> {
        // SAFETY: pseudo handle of this process; no preconditions.
        let me = unsafe { GetCurrentProcess() };
        open_process_token_with(me, TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY)
    }

    /// Restricted token whose only restricting SID is the user's own SID (`IsTokenRestricted`).
    pub fn restricted() -> io::Result<Token> {
        let own = own_token()?;
        let buf = token_info(&own, TokenUser)?;
        let user = buf.as_ptr().cast::<TOKEN_USER>();
        // SAFETY: `buf` holds the TOKEN_USER just returned and stays alive below.
        let restrict = SID_AND_ATTRIBUTES { Sid: unsafe { (*user).User.Sid }, Attributes: 0 };
        let mut out = ptr::null_mut();
        // SAFETY: `own` is a live token with TOKEN_DUPLICATE; `restrict` points into `buf`.
        check(unsafe {
            CreateRestrictedToken(own.0, DISABLE_MAX_PRIVILEGE, 0, ptr::null(), 0, ptr::null(), 1, &restrict, &mut out)
        })?;
        Ok(Token(Handle(out)))
    }

    /// Restricted token with `Users` (S-1-5-32-545) as deny-only, or (with `None`) only privileges
    /// removed.
    pub fn filtered(deny_users: bool) -> io::Result<Token> {
        let own = own_token()?;
        let text = wide("S-1-5-32-545");
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: `text` is NUL-terminated; `sid` receives a LocalAlloc'd SID.
        check(unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) })?;
        let sid = Local(sid);
        let disable = SID_AND_ATTRIBUTES { Sid: sid.0, Attributes: 0 };
        let n = u32::from(deny_users);
        let mut out = ptr::null_mut();
        // SAFETY: `own` is a live token with TOKEN_DUPLICATE; `disable` and its SID are alive.
        check(unsafe {
            CreateRestrictedToken(own.0, DISABLE_MAX_PRIVILEGE, n, &disable, 0, ptr::null(), 0, ptr::null(), &mut out)
        })?;
        Ok(Token(Handle(out)))
    }

    /// Impersonation token of the user at low integrity (`S-1-16-4096`).
    pub fn low_integrity() -> io::Result<Token> {
        let own = own_token()?;
        let mut dup = ptr::null_mut();
        // SAFETY: `own` is a live token with TOKEN_DUPLICATE; `dup` is a valid out pointer.
        check(unsafe {
            DuplicateTokenEx(own.0, TOKEN_ALL_ACCESS, ptr::null(), SecurityImpersonation, TokenImpersonation, &mut dup)
        })?;
        let dup = Handle(dup);
        let text = wide("S-1-16-4096");
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: `text` is NUL-terminated; `sid` receives a LocalAlloc'd SID.
        check(unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) })?;
        let sid = Local(sid);
        let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: sid.0, Attributes: 0x20 } };
        let size = (size_of::<TOKEN_MANDATORY_LABEL>() + 12) as u32;
        let info: *const TOKEN_MANDATORY_LABEL = &label;
        // SAFETY: `dup` is our token with TOKEN_ADJUST_DEFAULT; `label` and the SID are alive.
        check(unsafe { SetTokenInformation(dup.0, TokenIntegrityLevel, info.cast(), size) })?;
        Ok(Token(dup))
    }

    /// Impersonates `token` on this thread until dropped.
    pub struct Impersonation;

    pub fn impersonate(token: &Token) -> io::Result<Impersonation> {
        // SAFETY: `token` is a live token handle with TOKEN_QUERY | TOKEN_DUPLICATE.
        check(unsafe { ImpersonateLoggedOnUser(token.0.0) })?;
        Ok(Impersonation)
    }

    impl Drop for Impersonation {
        fn drop(&mut self) {
            // SAFETY: ends the impersonation started by `impersonate` on this thread.
            unsafe { RevertToSelf() };
        }
    }
}
