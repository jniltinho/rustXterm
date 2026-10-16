//! `cargo xtask layers`: enforce the D1 crate layering from `cargo metadata`, and the lint
//! policy of add-windows-port W2 (only the Windows adapter may allow documented `unsafe`).

use anyhow::{Context, bail};
use serde::Deserialize;

/// Workspace crates and their layer (D1). `xtask` is exempt.
const LAYERS: &[(&str, u8)] = &[
    ("rxt-core", 0),
    ("rxt-platform", 0),
    ("rxt-platform-linux", 1),
    ("rxt-platform-windows", 1),
    ("rxt-platform-macos", 1),
    ("rxt-store", 1),
    ("rxt-term", 1),
    ("rxt-ssh", 2),
    ("rxt-sftp", 2),
    ("rxt-ftp", 2),
    ("rxt-rdp", 2),
    ("rxt-vnc", 2),
    ("rxt-telnet", 2),
    ("rxt-serial", 2),
    ("rxt-engine", 3),
    ("rxt-agent", 4),
    ("rxt-ui", 4),
    ("rustxterm", 5),
];
/// Same-layer edges D1 allows.
const SAME_LAYER_OK: &[(&str, &str)] = &[("rxt-platform", "rxt-core"), ("rxt-sftp", "rxt-ssh")];
const UI_OWNERS: &[&str] = &["rxt-ui", "rustxterm"];
/// Platform adapters: only the app may depend on them (dev-dependencies aside).
const ADAPTERS: &[&str] = &["rxt-platform-linux", "rxt-platform-windows", "rxt-platform-macos"];
/// OS-specific crates and the crates allowed to depend on them (design W8).
const OWNED: &[(&[&str], &[&str], &str)] = &[
    (&["ashpd", "oo7", "zbus", "portable-pty", "nix"], &["rxt-platform-linux", "rustxterm"], "Linux-only"),
    (&["windows", "windows-sys", "raw-window-handle"], &["rxt-platform-windows"], "Windows-only"),
    (&["security-framework"], &["rxt-platform-macos"], "macOS-only"),
    (
        &["notify-rust", "rfd", "arboard", "serialport", "dirs"],
        &["rxt-platform-linux", "rxt-platform-windows", "rxt-platform-macos", "rustxterm"],
        "adapter",
    ),
];
/// The only crate with its own lint table (user decision, design W2).
const UNSAFE_CRATE: &str = "rxt-platform-windows";
const UNSAFE_DENIALS: &[&str] = &["undocumented_unsafe_blocks", "multiple_unsafe_ops_per_block"];

#[derive(Deserialize)]
pub struct Metadata {
    packages: Vec<Package>,
    #[serde(default)]
    workspace_root: String,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    dependencies: Vec<Dependency>,
    #[serde(default)]
    manifest_path: String,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    kind: Option<String>,
    path: Option<String>,
}

pub fn run() -> anyhow::Result<()> {
    let out = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let meta: Metadata = serde_json::from_slice(&out.stdout).context("parsing cargo metadata")?;
    let mut problems = check(&meta);
    let read = |path: &str| -> anyhow::Result<toml::Table> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        toml::from_str(&text).with_context(|| format!("parsing {path}"))
    };
    let root = read(&format!("{}/Cargo.toml", meta.workspace_root))?;
    let members = meta
        .packages
        .iter()
        .map(|p| Ok((p.name.clone(), read(&p.manifest_path)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    problems.extend(check_lints(&root, &members));
    if problems.is_empty() {
        println!("layers: ok ({} crates)", meta.packages.len());
        return Ok(());
    }
    problems.iter().for_each(|p| eprintln!("layer violation: {p}"));
    bail!("{} layer violation(s)", problems.len())
}

fn layer(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

fn is_ui_crate(name: &str) -> bool {
    name.starts_with("egui") || name == "eframe" || name.starts_with("wgpu")
}

/// All D1 violations in workspace metadata (dev-dependencies are ignored).
pub fn check(meta: &Metadata) -> Vec<String> {
    let mut problems = Vec::new();
    for pkg in meta.packages.iter().filter(|p| p.name != "xtask") {
        let Some(own) = layer(&pkg.name) else {
            problems.push(format!("{} is not in the xtask layer table", pkg.name));
            continue;
        };
        let p = pkg.name.as_str();
        for dep in pkg.dependencies.iter().filter(|d| d.kind.as_deref() != Some("dev")) {
            let d = dep.name.as_str();
            match layer(d) {
                Some(l) => {
                    let same_ok = l == own && SAME_LAYER_OK.contains(&(p, d));
                    if l > own || (l == own && !same_ok) {
                        problems.push(format!("{p} (L{own}) depends on {d} (L{l})"));
                    }
                    if ADAPTERS.contains(&d) && p != "rustxterm" {
                        problems.push(format!("{p} depends on {d} (only the app may)"));
                    }
                }
                None if dep.path.is_some() => problems.push(format!("{p} depends on unlayered path crate {d}")),
                None => {}
            }
            if is_ui_crate(d) && !UI_OWNERS.contains(&p) {
                problems.push(format!("{p} depends on UI crate {d} (only rxt-ui/app may)"));
            }
            for (crates, owners, kind) in OWNED {
                if crates.contains(&d) && !owners.contains(&p) {
                    problems.push(format!("{p} depends on {kind} crate {d} (only {} may)", owners.join("/")));
                }
            }
        }
    }
    problems
}

fn get<'a>(t: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Value> {
    let (last, parents) = path.split_last()?;
    let mut table = t;
    for key in parents {
        table = table.get(*key)?.as_table()?;
    }
    table.get(*last)
}

/// Lint policy (design W2): the workspace forbids `unsafe`; every crate inherits the workspace
/// lints except the Windows adapter, whose own table repeats each workspace lint at the same
/// level (`unsafe_code` aside) and denies undocumented or multi-operation `unsafe` blocks.
pub fn check_lints(root: &toml::Table, members: &[(String, toml::Table)]) -> Vec<String> {
    let mut problems = Vec::new();
    if get(root, &["workspace", "lints", "rust", "unsafe_code"]).and_then(|v| v.as_str()) != Some("forbid") {
        problems.push("workspace lints must set rust.unsafe_code = \"forbid\"".to_string());
    }
    let workspace_lints = get(root, &["workspace", "lints"]).and_then(|v| v.as_table());
    for (name, manifest) in members {
        let lints = manifest.get("lints").and_then(|v| v.as_table());
        if name != UNSAFE_CRATE {
            let inherits =
                lints.is_some_and(|l| l.len() == 1 && l.get("workspace").and_then(|v| v.as_bool()) == Some(true));
            if !inherits {
                problems.push(format!("{name} must use exactly `[lints] workspace = true`"));
            }
            continue;
        }
        let Some(lints) = lints.filter(|l| !l.contains_key("workspace")) else {
            problems.push(format!("{name} must have its own lint table (not workspace = true)"));
            continue;
        };
        for (group, table) in workspace_lints.into_iter().flatten() {
            for (lint, level) in table.as_table().into_iter().flatten() {
                if group == "rust" && lint == "unsafe_code" {
                    continue;
                }
                if get(lints, &[group, lint]) != Some(level) {
                    problems.push(format!("{name} must keep workspace lint {group}.{lint} = {level}"));
                }
            }
        }
        if get(lints, &["rust", "unsafe_code"]).and_then(|v| v.as_str()) != Some("allow") {
            problems.push(format!("{name} must set rust.unsafe_code = \"allow\" (its only exemption)"));
        }
        for lint in UNSAFE_DENIALS {
            if get(lints, &["clippy", lint]).and_then(|v| v.as_str()) != Some("deny") {
                problems.push(format!("{name} must deny clippy.{lint}"));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(edges: &[(&str, &[&str])]) -> Metadata {
        let packages = edges
            .iter()
            .map(|(name, deps)| {
                let deps: Vec<_> = deps
                    .iter()
                    .map(
                        |d| serde_json::json!({"name": d, "kind": null, "path": d.starts_with("rxt-").then_some("/p")}),
                    )
                    .collect();
                serde_json::json!({"name": name, "dependencies": deps})
            })
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({ "packages": packages })).unwrap()
    }

    #[test]
    fn clean_graph_passes() {
        let m = meta(&[
            ("rxt-core", &["serde"]),
            ("rxt-platform", &["rxt-core"]),
            ("rxt-sftp", &["rxt-core", "rxt-ssh"]),
            ("rxt-platform-linux", &["rxt-platform", "ashpd", "zbus", "arboard"]),
            ("rxt-platform-windows", &["rxt-platform", "windows-sys", "raw-window-handle", "arboard", "dirs"]),
            ("rxt-platform-macos", &["rxt-platform", "security-framework", "arboard", "dirs"]),
            ("rxt-ui", &["rxt-engine", "egui", "eframe"]),
            ("rustxterm", &["rxt-ui", "rxt-platform-linux", "rxt-platform-windows", "rxt-platform-macos", "eframe"]),
            ("xtask", &["rxt-ui", "anything"]),
        ]);
        assert_eq!(check(&m), Vec::<String>::new());
    }

    #[test]
    fn violations_are_reported() {
        let m = meta(&[
            ("rxt-ssh", &["rxt-ui", "rxt-ftp", "egui"]),
            ("rxt-telnet", &["rxt-ui"]),
            ("rxt-serial", &["rxt-ui", "rxt-platform-linux"]),
            ("rxt-ssh-extra", &[]),
            ("rxt-engine", &["rxt-platform-linux", "arboard", "portable-pty"]),
            ("rxt-ftp", &["rxt-sftp"]),
            ("rxt-term", &["wgpu"]),
            ("rxt-store", &["rxt-platform-windows", "windows-sys", "dirs"]),
            ("rxt-platform-linux", &["windows-sys"]),
        ]);
        let p = check(&m);
        let has = |s: &str| p.iter().any(|x| x.contains(s));
        assert!(has("rxt-ssh (L2) depends on rxt-ui"));
        assert!(has("rxt-telnet (L2) depends on rxt-ui"));
        assert!(has("rxt-serial (L2) depends on rxt-ui"));
        assert!(has("rxt-serial depends on rxt-platform-linux"));
        assert!(has("rxt-ssh (L2) depends on rxt-ftp"));
        assert!(has("rxt-ssh depends on UI crate egui"));
        assert!(has("rxt-ssh-extra is not in the xtask layer table"));
        assert!(has("rxt-engine depends on rxt-platform-linux"));
        assert!(has("rxt-engine depends on adapter crate arboard"));
        assert!(has("rxt-engine depends on Linux-only crate portable-pty"));
        assert!(has("rxt-ftp (L2) depends on rxt-sftp"));
        assert!(has("rxt-term depends on UI crate wgpu"));
        assert!(has("rxt-store depends on rxt-platform-windows (only the app may)"));
        assert!(has("rxt-store depends on Windows-only crate windows-sys"));
        assert!(has("rxt-store depends on adapter crate dirs"));
        assert!(has("rxt-platform-linux depends on Windows-only crate windows-sys"));
        assert_eq!(p.len(), 17, "{p:#?}");
    }

    const ROOT: &str = r#"
        [workspace.lints.rust]
        unsafe_code = "forbid"
        [workspace.lints.clippy]
        unwrap_used = "deny"
        todo = "deny"
    "#;
    const INHERIT: &str = "[lints]\nworkspace = true\n";
    const ADAPTER: &str = r#"
        [lints.rust]
        unsafe_code = "allow"
        [lints.clippy]
        unwrap_used = "deny"
        todo = "deny"
        undocumented_unsafe_blocks = "deny"
        multiple_unsafe_ops_per_block = "deny"
    "#;

    fn lints(root: &str, members: &[(&str, &str)]) -> Vec<String> {
        let members: Vec<_> = members.iter().map(|(n, t)| (n.to_string(), toml::from_str(t).unwrap())).collect();
        check_lints(&toml::from_str(root).unwrap(), &members)
    }

    #[test]
    fn lint_policy_passes() {
        assert_eq!(lints(ROOT, &[("rxt-core", INHERIT), ("rxt-platform-windows", ADAPTER)]), Vec::<String>::new());
    }

    #[test]
    fn lint_policy_violations() {
        // Root forbid removed.
        let p = lints(&ROOT.replace("forbid", "deny"), &[("rxt-platform-windows", ADAPTER)]);
        assert!(p.iter().any(|x| x.contains("unsafe_code = \"forbid\"")), "{p:?}");
        // Another crate with its own lints (relaxing unsafe), or with extra keys next to workspace.
        let p = lints(ROOT, &[("rxt-core", "[lints.rust]\nunsafe_code = \"allow\"\n")]);
        assert_eq!(p, vec!["rxt-core must use exactly `[lints] workspace = true`"]);
        let p = lints(ROOT, &[("rxt-core", "[lints]\nworkspace = true\n[lints.rust]\nunsafe_code = \"allow\"\n")]);
        assert_eq!(p.len(), 1, "{p:?}");
        // Adapter drops or weakens a workspace lint.
        let p = lints(ROOT, &[("rxt-platform-windows", &ADAPTER.replace("todo = \"deny\"", "todo = \"warn\""))]);
        assert_eq!(p, vec!["rxt-platform-windows must keep workspace lint clippy.todo = \"deny\""]);
        // Adapter drops a safety denial.
        let p =
            lints(ROOT, &[("rxt-platform-windows", &ADAPTER.replace("multiple_unsafe_ops_per_block = \"deny\"", ""))]);
        assert_eq!(p, vec!["rxt-platform-windows must deny clippy.multiple_unsafe_ops_per_block"]);
        // Adapter `unsafe_code` other than exactly "allow".
        let p = lints(
            ROOT,
            &[("rxt-platform-windows", &ADAPTER.replace("unsafe_code = \"allow\"", "unsafe_code = \"warn\""))],
        );
        assert_eq!(p, vec!["rxt-platform-windows must set rust.unsafe_code = \"allow\" (its only exemption)"]);
        // Adapter inheriting would forbid unsafe: it must keep its own table.
        let p = lints(ROOT, &[("rxt-platform-windows", INHERIT)]);
        assert_eq!(p.len(), 1, "{p:?}");
    }

    #[test]
    fn dev_dependencies_are_ignored() {
        let m: Metadata = serde_json::from_value(serde_json::json!({"packages": [
            {"name": "rxt-ssh", "dependencies": [{"name": "rxt-ui", "kind": "dev", "path": "/p"}]}
        ]}))
        .unwrap();
        assert!(check(&m).is_empty());
    }
}
