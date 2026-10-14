//! `cargo xtask dist-check <dir> [--reproduce <other-dir>]` (design D10, add-windows-port W9):
//! checks the artifacts of `docker build --target dist`. Linux: glibc floor, NEEDED allowlist,
//! stripped, `--self-test`. Windows (when the zip/MSI are present): both executables in zip and MSI,
//! PE subsystems, zero PE timestamps, DLL import allowlist. Then SHA256SUMS and the guard.
//! `--reproduce` compares the Windows artifacts byte for byte with a second clean build.
//! Needs `objdump`/`readelf` (binutils) and, for Windows artifacts, `python3` and `msiextract`
//! (windows-toolchain image).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail, ensure};
use sha2::{Digest, Sha256};

/// Newest glibc symbol version the portable binary may reference.
const GLIBC_MAX: (u32, u32) = (2, 31);
/// Shared libraries the binary may link directly; graphics and windowing libraries are dlopen'ed.
const NEEDED_ALLOW: &[&str] =
    &["libc.so.", "libm.so.", "libdl.so.", "libpthread.so.", "librt.so.", "libgcc_s.so.", "ld-linux-x86-64.so."];

/// Windows system DLLs the executables may import (lowercase; `api-ms-win-core-*` API sets too).
const DLL_ALLOW: &[&str] = &[
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    "cfgmgr32.dll",
    "combase.dll",
    "crypt32.dll",
    "dnsapi.dll",
    "dwmapi.dll",
    "dxgi.dll",
    "gdi32.dll",
    "imm32.dll",
    "kernel32.dll",
    "msvcrt.dll",
    "ncrypt.dll",
    "ntdll.dll",
    "ole32.dll",
    "oleaut32.dll",
    "opengl32.dll",
    "secur32.dll",
    "setupapi.dll",
    "shell32.dll",
    "uiautomationcore.dll",
    "user32.dll",
    "userenv.dll",
    "uxtheme.dll",
    "ws2_32.dll",
];

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let (dir, other) = match args {
        [dir] => (dir, None),
        [dir, flag, other] if flag == "--reproduce" => (dir, Some(Path::new(other))),
        _ => bail!("usage: cargo xtask dist-check <dir> [--reproduce <other-dir>]"),
    };
    let dir = Path::new(dir);
    let names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    let pick = |suffix: &str| -> anyhow::Result<PathBuf> {
        let found: Vec<&String> = names.iter().filter(|n| n.starts_with("rustxterm-") && n.ends_with(suffix)).collect();
        match found.as_slice() {
            [one] => Ok(dir.join(one)),
            _ => bail!("expected exactly one rustxterm-*{suffix} in {}, found {found:?}", dir.display()),
        }
    };
    let mut artifacts = linux(&pick)?;
    if names.iter().any(|n| n.ends_with(".msi") || n.ends_with("-windows-x86_64.zip")) {
        artifacts.extend(windows(&pick("-windows-x86_64.zip")?, &pick("-x86_64.msi")?)?);
    }

    let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).context("reading SHA256SUMS")?;
    for artifact in &artifacts {
        let name = artifact.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let digest = hex(&Sha256::digest(std::fs::read(artifact)?));
        ensure!(sums.lines().any(|l| l == format!("{digest}  {name}")), "SHA256SUMS lacks a matching line for {name}");
    }
    println!("dist-check: SHA256SUMS ok");

    let guard_args: Vec<String> = std::iter::once("--ci".to_string())
        .chain(artifacts.iter().skip(1).flat_map(|p| ["--artifact".to_string(), p.display().to_string()]))
        .collect();
    crate::guard::run(&guard_args)?;
    if let Some(other) = other {
        reproduce(dir, other, &names)?;
    }
    println!("dist-check: ok");
    Ok(())
}

/// Linux checks; returns `[binary, tarball, AppImage]`.
fn linux(pick: &dyn Fn(&str) -> anyhow::Result<PathBuf>) -> anyhow::Result<Vec<PathBuf>> {
    let bin = pick("-x86_64")?;
    let appimage = pick("-x86_64.AppImage")?;
    let tarball = pick("-x86_64.tar.gz")?;

    let glibc = glibc_max(&tool("objdump", &["-T"], &bin)?)?;
    ensure!(glibc <= GLIBC_MAX, "binary needs GLIBC_{}.{} (max {}.{})", glibc.0, glibc.1, GLIBC_MAX.0, GLIBC_MAX.1);
    println!("dist-check: newest glibc symbol GLIBC_{}.{}", glibc.0, glibc.1);

    let needed = needed_libs(&tool("readelf", &["-d"], &bin)?);
    let bad: Vec<&String> = needed.iter().filter(|l| !NEEDED_ALLOW.iter().any(|a| l.starts_with(a))).collect();
    ensure!(bad.is_empty(), "binary links unexpected shared libraries: {bad:?}");
    println!("dist-check: NEEDED {}", needed.join(" "));

    ensure!(!tool("readelf", &["-S", "-W"], &bin)?.contains(".symtab"), "binary is not stripped");

    run_ok(Command::new(abs(&bin)?).arg("--self-test"), "binary --self-test")?;
    run_ok(Command::new(abs(&appimage)?).args(["--appimage-extract-and-run", "--self-test"]), "AppImage --self-test")?;
    Ok(vec![bin, tarball, appimage])
}

/// Windows checks on the executables inside the zip and the MSI; returns `[zip, msi]`.
fn windows(zip: &Path, msi: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let tmp = std::env::temp_dir().join(format!("rxt-dist-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (from_zip, from_msi) = (tmp.join("zip"), tmp.join("msi"));
    std::fs::create_dir_all(&from_msi)?;
    run_ok(Command::new("python3").args(["-m", "zipfile", "-e"]).arg(zip).arg(&from_zip), "unzip")?;
    run_ok(
        Command::new("msiextract").arg("-C").arg(&from_msi).arg(msi).stdout(std::process::Stdio::null()),
        "msiextract",
    )?;
    for (exe, subsystem) in [("rustxterm.exe", 2), ("rustxterm-cli.exe", 3)] {
        let in_zip = find(&from_zip, exe)?;
        let bytes = std::fs::read(&in_zip)?;
        ensure!(std::fs::read(find(&from_msi, exe)?)? == bytes, "{exe} differs between the zip and the MSI");
        let pe = parse_pe(&tool("objdump", &["-p"], &in_zip)?)?;
        ensure!(pe.subsystem == subsystem, "{exe}: PE subsystem {} (expected {subsystem})", pe.subsystem);
        ensure!(pe.timestamp_zero, "{exe}: PE timestamp is not zero (link with --no-insert-timestamp)");
        let bad: Vec<&String> = pe.dlls.iter().filter(|d| !dll_allowed(d)).collect();
        ensure!(bad.is_empty(), "{exe} imports DLLs outside the allowlist: {bad:?}");
        println!("dist-check: {exe} subsystem {subsystem}, timestamp 0, imports {}", pe.dlls.join(" "));
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(vec![zip.to_path_buf(), msi.to_path_buf()])
}

/// The single file named `name` below `dir`.
fn find(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut found = Vec::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == name) {
                found.push(p);
            }
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => bail!("expected exactly one {name} in {}, found {found:?}", dir.display()),
    }
}

/// Byte-for-byte comparison of the Windows artifacts (zip, MSI) with the same names in `other`, a
/// second clean build (design W9). The Linux ELF is not covered: the zigbuild link is not
/// reproducible yet (two clean builds differ), which is tracked outside add-windows-port.
fn reproduce(dir: &Path, other: &Path, names: &[String]) -> anyhow::Result<()> {
    let names: Vec<&String> =
        names.iter().filter(|n| n.ends_with(".msi") || n.ends_with("-windows-x86_64.zip")).collect();
    ensure!(!names.is_empty(), "--reproduce: no Windows artifacts in {}", dir.display());
    let mut differ = Vec::new();
    for n in &names {
        let theirs = std::fs::read(other.join(n)).with_context(|| format!("{n} missing in {}", other.display()))?;
        if std::fs::read(dir.join(n))? != theirs {
            differ.push(n.as_str());
        }
    }
    ensure!(differ.is_empty(), "not reproducible: {differ:?} differ from {}", other.display());
    println!("dist-check: {} artifacts identical to {}", names.len(), other.display());
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Pe {
    subsystem: u32,
    timestamp_zero: bool,
    /// Imported DLLs, lowercase, sorted, unique.
    dlls: Vec<String>,
}

/// Subsystem, header timestamp and imported DLLs from `objdump -p` of a PE file.
fn parse_pe(objdump: &str) -> anyhow::Result<Pe> {
    let field = |key: &str| objdump.lines().find_map(|l| l.strip_prefix(key)).map(str::trim);
    let subsystem = field("Subsystem").context("no Subsystem line")?;
    let subsystem = u32::from_str_radix(subsystem.split_whitespace().next().unwrap_or_default(), 16)?;
    let stamp = field("Time/Date").context("no Time/Date line")?;
    let mut dlls: Vec<String> =
        objdump.lines().filter_map(|l| l.trim().strip_prefix("DLL Name:")).map(|d| d.trim().to_lowercase()).collect();
    dlls.sort();
    dlls.dedup();
    Ok(Pe { subsystem, timestamp_zero: stamp == "Thu Jan  1 00:00:00 1970", dlls })
}

fn dll_allowed(dll: &str) -> bool {
    DLL_ALLOW.contains(&dll) || (dll.starts_with("api-ms-win-core-") && dll.ends_with(".dll"))
}

fn abs(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize().with_context(|| format!("resolving {}", path.display()))
}

fn tool(program: &str, args: &[&str], file: &Path) -> anyhow::Result<String> {
    let out = Command::new(program).args(args).arg(file).output().with_context(|| format!("running {program}"))?;
    ensure!(out.status.success(), "{program} failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_ok(cmd: &mut Command, what: &str) -> anyhow::Result<()> {
    let status = cmd.status().with_context(|| format!("starting {what}"))?;
    ensure!(status.success(), "{what} failed ({status})");
    Ok(())
}

/// Highest `GLIBC_x.y[.z]` version referenced in `objdump -T` output (patch level ignored).
fn glibc_max(objdump: &str) -> anyhow::Result<(u32, u32)> {
    let re = regex::Regex::new(r"GLIBC_(\d+)\.(\d+)")?;
    re.captures_iter(objdump)
        .map(|c| Ok((c[1].parse()?, c[2].parse()?)))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .max()
        .context("no GLIBC symbol versions found")
}

/// Library names from `readelf -d` `(NEEDED)` entries.
fn needed_libs(readelf: &str) -> Vec<String> {
    readelf
        .lines()
        .filter(|l| l.contains("(NEEDED)"))
        .filter_map(|l| Some(l.split_once('[')?.1.split_once(']')?.0.to_string()))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glibc_versions_compare_numerically() {
        let dump = "0 DF *UND* 0 GLIBC_2.2.5 memcpy\n0 DF *UND* 0 GLIBC_2.17 clock\n0 DF *UND* 0 (GLIBC_2.9) pipe2\n";
        assert_eq!(glibc_max(dump).unwrap(), (2, 17));
        assert!(glibc_max("0 DF *UND* 0 GLIBC_2.34 x\n").unwrap() > GLIBC_MAX);
        assert!(glibc_max("nothing").is_err());
    }

    #[test]
    fn pe_headers_and_imports_are_parsed() {
        let dump = "x.exe:     file format pei-x86-64\nTime/Date\t\tThu Jan  1 00:00:00 1970\n\
                    MajorSubsystemVersion\t5\nSubsystem\t\t00000002\t(Windows GUI)\n\
                    \tDLL Name: KERNEL32.dll\n\tDLL Name: kernel32.dll\n\tDLL Name: api-ms-win-core-synch-l1-2-0.dll\n";
        let pe = parse_pe(dump).unwrap();
        assert_eq!(
            pe,
            Pe {
                subsystem: 2,
                timestamp_zero: true,
                dlls: vec!["api-ms-win-core-synch-l1-2-0.dll".into(), "kernel32.dll".into()]
            }
        );
        assert!(pe.dlls.iter().all(|d| dll_allowed(d)));
        let cui = parse_pe("Time/Date\t\tWed Oct 14 01:02:03 2026\nSubsystem\t\t00000003\t(Windows CUI)\n").unwrap();
        assert_eq!((cui.subsystem, cui.timestamp_zero), (3, false));
        assert!(
            !dll_allowed("libssl-3-x64.dll") && !dll_allowed("libwinpthread-1.dll") && !dll_allowed("vcruntime140.dll")
        );
        assert!(parse_pe("no headers").is_err());
    }

    #[test]
    fn needed_entries_are_parsed() {
        let dump = " 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]\n \
                    0x0000000000000001 (NEEDED)             Shared library: [libssl.so.3]\n \
                    0x000000000000000e (SONAME)             Library soname: [x.so]\n";
        assert_eq!(needed_libs(dump), ["libc.so.6", "libssl.so.3"]);
    }
}
