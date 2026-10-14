//! `cargo xtask msi-normalize <file.msi> <package-code-uuid> <source-date-epoch>`
//! (add-windows-port design W9): wixl writes a random PackageCode and the current time into the
//! summary information; this sets the PackageCode (UUIDv5 of the version, from package.sh), the
//! creation and last-saved times (SOURCE_DATE_EPOCH) and a fixed creating application, and clears
//! the last-printed time, so two builds of one version are byte-identical.

use std::time::{Duration, SystemTime};

use anyhow::{Context, bail};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let [path, code, epoch] = args else {
        bail!("usage: cargo xtask msi-normalize <file.msi> <package-code-uuid> <source-date-epoch>")
    };
    let code: uuid::Uuid = code.parse().context("package code")?;
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(epoch.parse().context("source date epoch")?);
    let mut package = msi::open_rw(path).with_context(|| format!("opening {path}"))?;
    let info = package.summary_info_mut();
    info.set_uuid(code);
    info.set_creation_time(time);
    info.set_last_save_time(time);
    info.clear_last_printed();
    info.set_creating_application("rustXterm packaging/windows (wixl)");
    package.flush().with_context(|| format!("writing {path}"))?;
    println!("msi-normalize: {path} PackageCode {{{}}}", code.hyphenated().to_string().to_uppercase());
    Ok(())
}
