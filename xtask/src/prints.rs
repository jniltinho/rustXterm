//! `cargo xtask prints`: render every UI screen in both themes into `docs/prints/rustxterm/`.
//!
//! The kittest snapshot tests in `rxt-ui` already render each screen offscreen in Light and Dark;
//! this runs them (failing if any screen changed unexpectedly) and copies the PNGs out, then runs
//! the snapshot tests again with `RXT_PRINTS_SCALE=2` for `<screen>_<theme>@2x.png` prints (never
//! compared). `docs/prints/` is git-ignored.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const PRINTS: &str = "docs/prints/rustxterm";

pub fn run() -> Result<()> {
    cargo_test(&[], &[])?;
    let n = copy_pngs(Path::new("crates/ui/tests/snapshots"), Path::new(PRINTS))?;
    let out = std::env::current_dir()?.join(PRINTS);
    let out = out.to_str().context("non-UTF-8 prints path")?;
    // Snapshot tests only: the other tests assert 1× geometry.
    cargo_test(&["--lib", "tests::snapshot"], &[("RXT_PRINTS_SCALE", "2"), ("RXT_PRINTS_DIR", out)])?;
    println!("prints: {n} screenshots (+@2x) in {PRINTS}/");
    Ok(())
}

fn cargo_test(args: &[&str], env: &[(&str, &str)]) -> Result<()> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["test", "-p", "rxt-ui", "--locked"])
        .args(args)
        .envs(env.iter().copied())
        .status()
        .context("running rxt-ui snapshot tests")?;
    if !status.success() {
        bail!("rxt-ui snapshot tests failed; fix them (or regenerate with UPDATE_SNAPSHOTS=1) before printing");
    }
    Ok(())
}

/// Copy snapshot PNGs (not `.diff.png`/`.new.png`/`.old.png` leftovers) into `dst`.
fn copy_pngs(src: &Path, dst: &Path) -> Result<usize> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    let mut n = 0;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let stem = name.strip_suffix(".png").unwrap_or_default();
        if stem.is_empty() || stem.contains('.') {
            continue;
        }
        std::fs::copy(&path, dst.join(name)).with_context(|| format!("copying {}", path.display()))?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_only_final_snapshots() {
        let dir = std::env::temp_dir().join(format!("rxt-prints-{}", std::process::id()));
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        for f in ["main_light.png", "main_dark.png", "main_light.diff.png", "main_light.new.png", "notes.txt"] {
            std::fs::write(src.join(f), b"x").unwrap();
        }
        assert_eq!(copy_pngs(&src, &dst).unwrap(), 2);
        assert!(dst.join("main_dark.png").exists() && !dst.join("main_light.diff.png").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
