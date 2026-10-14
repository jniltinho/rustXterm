//! `cargo xtask scenarios` (task 10.5): every `#### Scenario:` of the change's specs must have exactly one
//! row in the scenario map of `docs/gates/release.md`, and every cited `path.rs::test` must exist.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, ensure};

const SPECS: &str = "openspec/specs";
const MAP: &str = "docs/gates/release.md";

pub fn run() -> anyhow::Result<()> {
    let mut scenarios = BTreeSet::new();
    for entry in std::fs::read_dir(SPECS).with_context(|| format!("listing {SPECS}"))? {
        let dir = entry?.path();
        let spec = dir.join("spec.md");
        if !spec.exists() {
            continue;
        }
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = std::fs::read_to_string(&spec).with_context(|| format!("reading {}", spec.display()))?;
        scenarios.extend(spec_scenarios(&text).into_iter().map(|s| (name.clone(), s)));
    }
    let map = std::fs::read_to_string(MAP).with_context(|| format!("reading {MAP}"))?;
    let problems = check(&scenarios, &map_rows(&map), |file, test| {
        std::fs::read_to_string(Path::new(file)).is_ok_and(|src| src.contains(&format!("fn {test}(")))
    })?;
    for p in &problems {
        eprintln!("scenarios: {p}");
    }
    ensure!(problems.is_empty(), "{} problem(s) in the scenario map of {MAP}", problems.len());
    println!("scenarios: {} scenarios mapped in {MAP}", scenarios.len());
    Ok(())
}

fn spec_scenarios(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.strip_prefix("#### Scenario: ")).map(|s| s.trim().to_string()).collect()
}

/// `(spec, scenario, evidence)` from four-column table rows; header and separator rows are skipped.
fn map_rows(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.trim().strip_prefix('|')?.strip_suffix('|')?.split('|').map(str::trim).collect();
            match cells.as_slice() {
                ["Spec", ..] | ["---", ..] => None,
                [spec, scenario, evidence, _status] => {
                    Some((spec.to_string(), scenario.to_string(), evidence.to_string()))
                }
                _ => None,
            }
        })
        .collect()
}

fn check(
    scenarios: &BTreeSet<(String, String)>,
    rows: &[(String, String, String)],
    test_exists: impl Fn(&str, &str) -> bool,
) -> anyhow::Result<Vec<String>> {
    let reference = regex::Regex::new(r"`([\w/.-]+\.rs)::(\w+)`")?;
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for (spec, scenario, evidence) in rows {
        let key = (spec.clone(), scenario.clone());
        if !scenarios.contains(&key) {
            problems.push(format!("row for unknown scenario {spec} / {scenario}"));
        }
        if !seen.insert(key) {
            problems.push(format!("duplicate row for {spec} / {scenario}"));
        }
        for c in reference.captures_iter(evidence) {
            if !test_exists(&c[1], &c[2]) {
                problems.push(format!("{spec} / {scenario}: {}::{} not found", &c[1], &c[2]));
            }
        }
    }
    problems.extend(scenarios.difference(&seen).map(|(s, n)| format!("unmapped scenario {s} / {n}")));
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmapped_unknown_and_stale_references_are_reported() {
        let scenarios: BTreeSet<_> =
            spec_scenarios("#### Scenario: A\n- x\n#### Scenario: B\n").into_iter().map(|s| ("s".into(), s)).collect();
        let rows = map_rows(
            "| Spec | Scenario | Evidence | Status |\n|---|---|---|---|\n\
             | s | A | `a.rs::ok`, `a.rs::gone` | auto |\n| s | C | none | manual |\n",
        );
        let problems = check(&scenarios, &rows, |_, t| t == "ok").unwrap();
        assert_eq!(
            problems,
            ["s / A: a.rs::gone not found", "row for unknown scenario s / C", "unmapped scenario s / B"]
        );
    }
}
