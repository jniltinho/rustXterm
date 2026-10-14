//! `cargo xtask lab test`: runs the `#[ignore]`d lab tests and checks that the executed
//! test names equal `lab/expected-tests.txt` exactly. Runs inside the toolchain container;
//! `lab/lab.sh` starts the lab on the host and calls this with host networking.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Entry point for `cargo xtask lab <args>`.
pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("test") => test(),
        Some("up" | "down" | "status") => {
            bail!("`lab {}` needs Docker on the host: run `lab/lab.sh {}`", args[0], args[0])
        }
        _ => bail!("usage: cargo xtask lab test (host: lab/lab.sh up|down|status|test)"),
    }
}

fn test() -> Result<()> {
    let root = workspace_root();
    let generated = root.join("lab/.generated");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut child = Command::new(cargo)
        .current_dir(&root)
        .args(["test", "--workspace", "--tests", "--no-fail-fast", "--", "--ignored"])
        .env("RXT_LAB_SSH_KEY", generated.join("bob_ed25519"))
        .env("RXT_LAB_LOG_DIR", generated.join("logs"))
        .stdout(Stdio::piped())
        .spawn()
        .context("spawning cargo test")?;

    let mut outcomes = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let line = line.context("reading cargo test output")?;
            println!("{line}");
            outcomes.extend(parse_line(&line));
        }
    }
    let status = child.wait().context("waiting for cargo test")?;

    let expected_path = root.join("lab/expected-tests.txt");
    let expected =
        std::fs::read_to_string(&expected_path).with_context(|| format!("reading {}", expected_path.display()))?;
    let failed: Vec<&str> = outcomes.iter().filter(|o| !o.passed).map(|o| o.name.as_str()).collect();
    let executed: BTreeSet<&str> = outcomes.iter().map(|o| o.name.as_str()).collect();
    let diff = compare(&parse_expected(&expected), &executed);

    for name in &failed {
        eprintln!("lab: FAILED {name}");
    }
    for name in &diff.missing {
        eprintln!("lab: expected but not executed: {name}");
    }
    for name in &diff.unexpected {
        eprintln!("lab: executed but not expected: {name}");
    }
    if !status.success() || !failed.is_empty() || !diff.missing.is_empty() || !diff.unexpected.is_empty() {
        bail!(
            "lab test failed: cargo {status}, {} failed, {} missing, {} unexpected",
            failed.len(),
            diff.missing.len(),
            diff.unexpected.len()
        );
    }
    println!("lab: {} tests executed, matching lab/expected-tests.txt", executed.len());
    Ok(())
}

fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

#[derive(Debug, PartialEq)]
struct Outcome {
    name: String,
    passed: bool,
}

/// Parses a libtest line `test <name> ... ok|FAILED`; `ignored` and other lines are not executions.
fn parse_line(line: &str) -> Option<Outcome> {
    let (name, result) = line.strip_prefix("test ")?.rsplit_once(" ... ")?;
    let passed = match result.trim() {
        "ok" => true,
        "FAILED" => false,
        _ => return None,
    };
    Some(Outcome { name: name.to_string(), passed })
}

/// Expected names: one per line, blank lines and `#` comments ignored.
fn parse_expected(text: &str) -> BTreeSet<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

#[derive(Debug, Default, PartialEq)]
struct Diff<'a> {
    missing: Vec<&'a str>,
    unexpected: Vec<&'a str>,
}

fn compare<'a>(expected: &BTreeSet<&'a str>, executed: &BTreeSet<&'a str>) -> Diff<'a> {
    Diff {
        missing: expected.difference(executed).copied().collect(),
        unexpected: executed.difference(expected).copied().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_libtest_lines() {
        assert_eq!(
            parse_line("test ssh::auth::password ... ok"),
            Some(Outcome { name: "ssh::auth::password".into(), passed: true })
        );
        assert_eq!(
            parse_line("test sftp::big_file ... FAILED"),
            Some(Outcome { name: "sftp::big_file".into(), passed: false })
        );
        assert_eq!(parse_line("test a::b ... ignored"), None);
        assert_eq!(parse_line("test a::b ... ignored, needs lab"), None);
        assert_eq!(parse_line("test result: ok. 3 passed; 0 failed"), None);
        assert_eq!(parse_line("running 3 tests"), None);
    }

    #[test]
    fn expected_list_skips_comments_and_blanks() {
        let set = parse_expected("# header\n\n  a::one \n#b::two\nc::three\n");
        assert_eq!(set, BTreeSet::from(["a::one", "c::three"]));
    }

    #[test]
    fn compare_reports_missing_and_unexpected() {
        let expected = BTreeSet::from(["a", "b"]);
        let executed = BTreeSet::from(["b", "c"]);
        assert_eq!(compare(&expected, &executed), Diff { missing: vec!["a"], unexpected: vec!["c"] });
        assert_eq!(compare(&expected, &expected), Diff::default());
    }

    #[test]
    fn renamed_test_fails_comparison() {
        let expected = BTreeSet::from(["ssh::password"]);
        let executed = BTreeSet::from(["ssh::password_renamed"]);
        let diff = compare(&expected, &executed);
        assert_eq!(diff.missing, ["ssh::password"]);
        assert_eq!(diff.unexpected, ["ssh::password_renamed"]);
    }
}
