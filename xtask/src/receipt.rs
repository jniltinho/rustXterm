//! `cargo xtask windows-receipt --check <dist-dir> [--receipt docs/gates/windows.md]`
//! (add-windows-port design W10): binds the manual Windows E2E receipt to a release before the
//! Windows artifacts are signed and published.
//!
//! The receipt's release section has `- Source commit: `<sha>``, one `- SHA-256 `<file>`: `<hex>``
//! line per unsigned artifact (zip, MSI and both executables) and a table row `| Wnn | … | PASS |`
//! per mandatory check. The check passes only when every mandatory row is PASS, the source commit
//! is HEAD or an ancestor whose diff to HEAD touches only the receipt, and every digest equals the
//! rebuilt unsigned artifact (reproducible builds make the rebuild identical to what was tested).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail, ensure};
use sha2::{Digest, Sha256};

const RECEIPT: &str = "docs/gates/windows.md";
/// Mandatory receipt rows (tasks.md 3.2).
const ROWS: std::ops::RangeInclusive<u32> = 1..=41;

#[derive(Debug, Default, PartialEq)]
struct Receipt {
    commit: Option<String>,
    /// File name → lowercase hex SHA-256.
    digests: BTreeMap<String, String>,
    /// Row id (`W01`) → result cell.
    rows: BTreeMap<String, String>,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let (dist, receipt) = match args {
        [flag, dist] if flag == "--check" => (dist, RECEIPT.to_string()),
        [flag, dist, r, path] if flag == "--check" && r == "--receipt" => (dist, path.clone()),
        _ => bail!("usage: cargo xtask windows-receipt --check <dist-dir> [--receipt {RECEIPT}]"),
    };
    let text = std::fs::read_to_string(&receipt).with_context(|| format!("reading {receipt}"))?;
    let parsed = parse(&text);
    check_rows(&parsed)?;
    let commit = parsed.commit.as_deref().context("receipt has no `- Source commit:` line")?;
    check_commit(commit, &receipt)?;
    check_digests(&parsed, &artifact_digests(Path::new(dist))?)?;
    println!("windows-receipt: ok (commit {commit}, {} digests, {} rows PASS)", parsed.digests.len(), ROWS.count());
    Ok(())
}

fn parse(md: &str) -> Receipt {
    let tick = |s: &str| s.trim().trim_matches('`').to_string();
    let mut r = Receipt::default();
    for line in md.lines().map(str::trim) {
        if let Some(c) = line.strip_prefix("- Source commit:") {
            r.commit = Some(tick(c));
        } else if let Some((name, hex)) = line.strip_prefix("- SHA-256 ").and_then(|l| l.split_once(':')) {
            r.digests.insert(tick(name), tick(hex).to_lowercase());
        } else if let Some(cells) = line.strip_prefix('|').and_then(|l| l.strip_suffix('|')) {
            let cells: Vec<&str> = cells.split('|').map(str::trim).collect();
            if let (Some(id), Some(result)) = (cells.first(), cells.last())
                && id.len() == 3
                && id.starts_with('W')
                && id[1..].bytes().all(|b| b.is_ascii_digit())
            {
                r.rows.insert((*id).to_string(), (*result).to_string());
            }
        }
    }
    r
}

fn check_rows(r: &Receipt) -> anyhow::Result<()> {
    let open: Vec<String> =
        ROWS.map(|n| format!("W{n:02}")).filter(|id| r.rows.get(id).map(String::as_str) != Some("PASS")).collect();
    ensure!(open.is_empty(), "receipt rows missing or not PASS: {}", open.join(" "));
    Ok(())
}

/// The receipt commit is HEAD, or an ancestor whose changes up to HEAD touch only the receipt.
fn check_commit(commit: &str, receipt: &str) -> anyhow::Result<()> {
    let ancestor = Command::new("git").args(["merge-base", "--is-ancestor", commit, "HEAD"]).status()?;
    ensure!(ancestor.success(), "receipt commit {commit} is not an ancestor of HEAD");
    let out = Command::new("git").args(["diff", "--name-only", commit, "HEAD"]).output()?;
    ensure!(out.status.success(), "git diff failed: {}", String::from_utf8_lossy(&out.stderr));
    stale(&String::from_utf8_lossy(&out.stdout), receipt)
}

fn stale(changed: &str, receipt: &str) -> anyhow::Result<()> {
    let other: Vec<&str> = changed.lines().filter(|f| !f.is_empty() && *f != receipt).collect();
    ensure!(other.is_empty(), "receipt is stale: files changed since its commit: {other:?}");
    Ok(())
}

/// SHA-256 of the unsigned zip and MSI in `dist` and of both executables inside the zip.
fn artifact_digests(dist: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dist).with_context(|| format!("listing {}", dist.display()))? {
        let p = e?.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.ends_with(".msi") || name.ends_with("-windows-x86_64.zip") {
            out.insert(name.clone(), hex(&std::fs::read(&p)?));
        }
        if name.ends_with("-windows-x86_64.zip") {
            let tmp = std::env::temp_dir().join(format!("rxt-receipt-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            let ok = Command::new("python3").args(["-m", "zipfile", "-e"]).arg(&p).arg(&tmp).status()?;
            ensure!(ok.success(), "extracting {name} failed");
            for exe in ["rustxterm.exe", "rustxterm-cli.exe"] {
                let top = name.trim_end_matches(".zip");
                out.insert(exe.to_string(), hex(&std::fs::read(tmp.join(top).join(exe))?));
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
    ensure!(out.len() == 4, "expected the Windows zip and MSI in {}, found {:?}", dist.display(), out.keys());
    Ok(out)
}

fn check_digests(r: &Receipt, built: &BTreeMap<String, String>) -> anyhow::Result<()> {
    for (name, digest) in built {
        match r.digests.get(name) {
            Some(d) if d == digest => {}
            Some(d) => bail!("{name}: receipt SHA-256 {d} differs from the rebuilt {digest}"),
            None => bail!("receipt has no SHA-256 for {name}"),
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(rows: impl Fn(u32) -> Option<&'static str>) -> String {
        let mut md = String::from(
            "# Windows gate receipt\n\n## Release receipt\n\n- Source commit: `abc123`\n\
             - SHA-256 `rustxterm-0.3.0-windows-x86_64.zip`: `AA11`\n\
             - SHA-256 `rustxterm-0.3.0-x86_64.msi`: `bb22`\n\
             - SHA-256 `rustxterm.exe`: `cc33`\n- SHA-256 `rustxterm-cli.exe`: `dd44`\n\n\
             | Row | Check | Result |\n|---|---|---|\n",
        );
        for n in ROWS {
            if let Some(result) = rows(n) {
                md.push_str(&format!("| W{n:02} | check {n} | {result} |\n"));
            }
        }
        md
    }

    fn built() -> BTreeMap<String, String> {
        [
            ("rustxterm-0.3.0-windows-x86_64.zip", "aa11"),
            ("rustxterm-0.3.0-x86_64.msi", "bb22"),
            ("rustxterm.exe", "cc33"),
            ("rustxterm-cli.exe", "dd44"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    #[test]
    fn matching_pass_receipt_passes() {
        let r = parse(&receipt(|_| Some("PASS")));
        assert_eq!(r.commit.as_deref(), Some("abc123"));
        assert_eq!(r.rows.len(), 41);
        check_rows(&r).unwrap();
        check_digests(&r, &built()).unwrap();
        stale("docs/gates/windows.md\n", RECEIPT).unwrap();
        stale("", RECEIPT).unwrap();
    }

    #[test]
    fn fail_row_is_refused() {
        let r = parse(&receipt(|n| Some(if n == 12 { "FAIL" } else { "PASS" })));
        assert!(check_rows(&r).unwrap_err().to_string().contains("W12"));
    }

    #[test]
    fn missing_row_is_refused_even_with_an_overall_pass() {
        let md = receipt(|n| (n != 40).then_some("PASS")) + "\nOverall: PASS\n";
        let err = check_rows(&parse(&md)).unwrap_err().to_string();
        assert!(err.contains("W40") && !err.contains("W41"), "{err}");
    }

    #[test]
    fn stale_commit_is_refused() {
        let err = stale("docs/gates/windows.md\ncrates/ssh/src/lib.rs\n", RECEIPT).unwrap_err();
        assert!(err.to_string().contains("crates/ssh/src/lib.rs"));
    }

    #[test]
    fn mismatched_or_missing_digest_is_refused() {
        let r = parse(&receipt(|_| Some("PASS")));
        let mut other = built();
        other.insert("rustxterm.exe".into(), "ffff".into());
        assert!(check_digests(&r, &other).unwrap_err().to_string().contains("rustxterm.exe"));
        let mut extra = built();
        extra.insert("rustxterm-0.3.1-x86_64.msi".into(), "bb22".into());
        assert!(check_digests(&r, &extra).unwrap_err().to_string().contains("no SHA-256"));
    }

    #[test]
    fn smoke_receipt_is_not_a_release_receipt() {
        let smoke = "| Check | How | Result |\n|---|---|---|\n| PE headers | objdump | PASS |\n";
        assert!(check_rows(&parse(smoke)).is_err());
    }
}
