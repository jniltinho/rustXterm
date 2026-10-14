//! Windows only (add-windows-port design W5): GUI subsystem for `rustxterm.exe`, and icon,
//! version info and manifest resources for both executables, compiled with mingw `windres`
//! (no extra crate). Other targets: nothing to do.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows/rustxterm.manifest");
    println!("cargo:rerun-if-changed=../../assets/icons/rustxterm-256.png");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = resources() {
        println!("cargo:warning=Windows resources not embedded: {e}");
    }
    println!("cargo:rustc-link-arg-bin=rustxterm=-Wl,--subsystem,windows");
    // Main-thread stack as on Linux (8 MiB); the Windows default of 1 MiB overflows in the
    // database/key-derivation path and deep UI layouts.
    println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
}

/// ICO with one 256x256 PNG entry (supported since Windows Vista).
fn ico(png: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0, 1, 0];
    // width/height 0 = 256, no palette, 1 plane, 32 bpp, size, offset 22.
    out.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0]);
    out.extend_from_slice(&(png.len() as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(png);
    out
}

fn resources() -> Result<(), String> {
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let png = std::fs::read(dir.join("../../assets/icons/rustxterm-256.png")).map_err(|e| e.to_string())?;
    std::fs::write(out.join("rustxterm.ico"), ico(&png)).map_err(|e| e.to_string())?;
    let version = std::env::var("CARGO_PKG_VERSION").map_err(|e| e.to_string())?;
    let nums: Vec<&str> = version.split(['.', '-']).take(3).collect();
    let quad = format!("{},0", nums.join(","));
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    let rc = format!(
        r#"1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "rustXterm"
      VALUE "FileDescription", "rustXterm terminal and remote session manager"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "rustxterm"
      VALUE "LegalCopyright", "MIT License"
      VALUE "ProductName", "rustXterm"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = slash(&out.join("rustxterm.ico")),
        manifest = slash(&dir.join("windows/rustxterm.manifest")),
    );
    let rc_path = out.join("rustxterm.rc");
    std::fs::write(&rc_path, rc).map_err(|e| e.to_string())?;
    let res = out.join("rustxterm.res.o");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    let status = Command::new(&windres)
        .arg(&rc_path)
        .args(["-O", "coff", "-o"])
        .arg(&res)
        .status()
        .map_err(|e| format!("{windres}: {e}"))?;
    if !status.success() {
        return Err(format!("{windres} failed: {status}"));
    }
    println!("cargo:rustc-link-arg-bins={}", res.display());
    Ok(())
}
