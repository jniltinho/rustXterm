//! `cargo xtask guard` (design D12): local-only paths, forbidden digests, absolute
//! home paths and asset provenance across history, the work tree and release artifacts.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use regex::bytes::Regex;
use sha2::{Digest, Sha256};

const LOCAL_ONLY: &[&str] = &["base/", "docs/prints/"];
/// Placeholder homes plus the fictional lab accounts (lab/README.md); real user homes still fail.
const HOME_ALLOWLIST: &[&str] =
    &["/home/user/", "/home/builder/", "/Users/user/", "/home/alice/", "/home/bob/", "/home/carol/", "/home/tester/"];
const ASSET_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "ttf", "otf", "woff", "woff2"];

#[derive(Default)]
pub struct Options {
    /// CI mode: no access to `base/`, so the digest check is skipped.
    pub ci: bool,
    pub artifacts: Vec<PathBuf>,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut opts = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ci" => opts.ci = true,
            "--artifact" => opts.artifacts.push(it.next().context("--artifact needs a path")?.into()),
            other => bail!("guard: unknown argument {other}"),
        }
    }
    let top = git(Path::new("."), &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(String::from_utf8_lossy(&top).trim());
    let report = check(&root, &opts)?;
    for note in &report.notes {
        println!("guard: {note}");
    }
    if report.problems.is_empty() {
        println!("guard: ok");
        return Ok(());
    }
    report.problems.iter().for_each(|p| eprintln!("guard: {p}"));
    bail!("guard failed with {} problem(s)", report.problems.len())
}

#[derive(Default)]
pub struct Report {
    pub problems: BTreeSet<String>,
    pub notes: Vec<String>,
}

struct Scanner {
    /// sha256 → path under `base/`.
    forbidden: HashMap<[u8; 32], String>,
    home: Regex,
}

impl Scanner {
    fn scan(&self, label: &str, bytes: &[u8], report: &mut Report) {
        if let Some(src) = self.forbidden.get(&<[u8; 32]>::from(Sha256::digest(bytes))) {
            report.problems.insert(format!("{label}: identical to forbidden file {src}"));
        }
        let home = self
            .home
            .find_iter(bytes)
            .map(|m| m.as_bytes())
            .find(|m| !HOME_ALLOWLIST.iter().any(|a| a.as_bytes() == *m));
        if let Some(m) = home {
            report.problems.insert(format!("{label}: absolute home path {}", String::from_utf8_lossy(m)));
        }
    }
}

/// Run every check for the repository at `root`.
pub fn check(root: &Path, opts: &Options) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let forbidden = if opts.ci { HashMap::new() } else { forbidden_digests(&root.join("base"))? };
    report.notes.push(match (opts.ci, forbidden.len()) {
        (true, _) => "CI mode: digest check skipped".into(),
        (false, 0) => "base/ absent or empty: digest check skipped".into(),
        (false, n) => format!("digest check against {n} files under base/"),
    });
    let scanner = Scanner { forbidden, home: Regex::new(r"(?-u)/(?:home/[a-z_][a-z0-9_-]*|Users/[^/\x00-\x20]+)/")? };

    // (a) local-only paths in any commit.
    let log = git(root, &["log", "--all", "-m", "--no-renames", "--name-only", "--format="])?;
    for path in String::from_utf8_lossy(&log).lines().filter(|p| is_local_only(p)) {
        report.problems.insert(format!("history: a commit touches local-only path {path}"));
    }

    // Tracked plus untracked-but-not-ignored files (useful before the first commit).
    let listed = git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
    let files: BTreeSet<String> =
        listed.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    for f in &files {
        if is_local_only(f) {
            report.problems.insert(format!("file {f}: under a local-only directory"));
        }
        let path = root.join(f);
        if path.symlink_metadata().is_ok_and(|m| m.is_file()) {
            let bytes = std::fs::read(&path).with_context(|| format!("reading {f}"))?;
            scanner.scan(&format!("file {f}"), &bytes, &mut report);
        }
    }

    // (b) every historical blob.
    let blobs = history_blobs(root, |oid, path, bytes| {
        scanner.scan(&format!("history blob {oid} ({path})"), bytes, &mut report)
    })?;
    report.notes.push(format!("scanned {} files and {blobs} historical blobs", files.len()));

    for artifact in &opts.artifacts {
        let dir = extract(artifact)?;
        let mut members = Vec::new();
        walk(&dir, &mut members)?;
        for m in &members {
            let rel = m.strip_prefix(&dir).unwrap_or(m).display();
            let bytes = std::fs::read(m)?;
            scanner.scan(&format!("artifact {} member {rel}", artifact.display()), &bytes, &mut report);
        }
        let _ = std::fs::remove_dir_all(&dir);
        report.notes.push(format!("scanned {} members of {}", members.len(), artifact.display()));
    }

    // (c) provenance of icons, images, fonts and fixtures.
    let attribution = std::fs::read_to_string(root.join("ATTRIBUTION.md")).unwrap_or_default();
    let rows = attribution_paths(&attribution);
    for f in files.iter().filter(|f| needs_attribution(f)) {
        if !rows.iter().any(|r| glob_match(r, f)) {
            report.problems.insert(format!("file {f}: no ATTRIBUTION.md row"));
        }
    }
    Ok(report)
}

fn is_local_only(path: &str) -> bool {
    LOCAL_ONLY.iter().any(|p| path.starts_with(p))
}

fn git(root: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().context("running git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(out.stdout)
}

/// sha256 of every non-empty file under `base/` except `base/icons/`.
fn forbidden_digests(base: &Path) -> anyhow::Result<HashMap<[u8; 32], String>> {
    let mut files = Vec::new();
    if base.is_dir() {
        walk(base, &mut files)?;
    }
    let mut out = HashMap::new();
    for f in files.iter().filter(|f| !f.starts_with(base.join("icons"))) {
        let bytes = std::fs::read(f).with_context(|| format!("reading {}", f.display()))?;
        if !bytes.is_empty() {
            let rel = f.strip_prefix(base).unwrap_or(f).display();
            out.insert(Sha256::digest(&bytes).into(), format!("base/{rel}"));
        }
    }
    Ok(out)
}

/// Regular files below `dir` (symlinks are skipped).
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let path = entry?.path();
        let meta = path.symlink_metadata()?;
        if meta.is_dir() {
            walk(&path, out)?;
        } else if meta.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Feed every blob reachable from any ref to `f`; returns the blob count.
fn history_blobs(root: &Path, mut f: impl FnMut(&str, &str, &[u8])) -> anyhow::Result<usize> {
    let list = git(root, &["rev-list", "--objects", "--all"])?;
    let list = String::from_utf8_lossy(&list).into_owned();
    let names: HashMap<&str, &str> = list.lines().filter_map(|l| l.split_once(' ')).collect();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("running git cat-file")?;
    let mut stdin = child.stdin.take().context("cat-file stdin")?;
    let ids: String = list.lines().filter_map(|l| l.split(' ').next()).map(|id| format!("{id}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(ids.as_bytes()));
    let mut reader = BufReader::new(child.stdout.take().context("cat-file stdout")?);
    let (mut header, mut count) = (String::new(), 0);
    while {
        header.clear();
        reader.read_line(&mut header)? > 0
    } {
        let mut parts = header.split_whitespace();
        let (Some(oid), Some(kind), Some(size)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let mut body = vec![0; size.parse::<usize>()? + 1];
        reader.read_exact(&mut body)?;
        body.pop();
        if kind == "blob" {
            count += 1;
            f(oid, names.get(oid).copied().unwrap_or("?"), &body);
        }
    }
    writer.join().map_err(|_| anyhow::anyhow!("cat-file writer panicked"))??;
    if !child.wait()?.success() {
        bail!("git cat-file failed");
    }
    Ok(count)
}

/// Extract a tarball (`tar -xf`), AppImage (`--appimage-extract`), zip (`python3 -m zipfile`) or
/// MSI (`msiextract`, windows-toolchain image) into a fresh temp dir.
fn extract(artifact: &Path) -> anyhow::Result<PathBuf> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let abs = artifact.canonicalize().with_context(|| format!("artifact {}", artifact.display()))?;
    let dir =
        std::env::temp_dir().join(format!("rxt-guard-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir)?;
    let name = abs.to_string_lossy().to_ascii_lowercase();
    let mut cmd = if name.ends_with(".appimage") {
        let mut c = Command::new(&abs);
        c.arg("--appimage-extract").current_dir(&dir);
        c
    } else if name.ends_with(".zip") {
        let mut c = Command::new("python3");
        c.args(["-m", "zipfile", "-e"]).arg(&abs).arg(&dir);
        c
    } else if name.ends_with(".msi") {
        let mut c = Command::new("msiextract");
        c.arg("-C").arg(&dir).arg(&abs);
        c
    } else {
        let mut c = Command::new("tar");
        c.arg("-xf").arg(&abs).arg("-C").arg(&dir);
        c
    };
    let status = cmd.stdout(Stdio::null()).status().with_context(|| format!("extracting {}", abs.display()))?;
    if !status.success() {
        bail!("extracting {} failed", abs.display());
    }
    Ok(dir)
}

/// Icons, images, fonts, fixtures and everything under `assets/fonts/` (fonts plus license texts).
fn needs_attribution(path: &str) -> bool {
    if path.starts_with("assets/fonts/") {
        return true;
    }
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    ASSET_EXTS.contains(&ext.as_str()) || path.split('/').rev().skip(1).any(|s| s == "fixtures")
}

/// First column of every table row in ATTRIBUTION.md.
fn attribution_paths(md: &str) -> Vec<String> {
    md.lines()
        .filter_map(|l| l.trim().strip_prefix('|')?.split('|').next())
        .map(|c| c.trim().trim_matches('`').to_string())
        .filter(|c| !c.is_empty() && c != "Path" && !c.starts_with("--"))
        .collect()
}

/// Path glob: `*` matches within one segment, `**` matches any number of segments.
fn glob_match(pattern: &str, path: &str) -> bool {
    fn segs(p: &[&str], s: &[&str]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((&"**", rest)) => (0..=s.len()).any(|i| segs(rest, &s[i..])),
            Some((first, rest)) => {
                s.split_first().is_some_and(|(h, t)| wild(first.as_bytes(), h.as_bytes()) && segs(rest, t))
            }
        }
    }
    fn wild(p: &[u8], s: &[u8]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((b'*', rest)) => (0..=s.len()).any(|i| wild(rest, &s[i..])),
            Some((c, rest)) => s.split_first().is_some_and(|(h, t)| h == c && wild(rest, t)),
        }
    }
    segs(&pattern.split('/').collect::<Vec<_>>(), &path.split('/').collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Disposable git repo with `base/` ignored, like the real one.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rxt-guard-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let r = Repo(dir);
            r.git(&["init", "-q", "-b", "main"]);
            r.write(".gitignore", b"base/\ndocs/prints/\n");
            r.write(
                "ATTRIBUTION.md",
                b"| Path | Origin | License |\n|---|---|---|\n| `assets/ok/*.png` | ours | MIT |\n",
            );
            r.commit("init");
            r
        }
        fn write(&self, path: &str, bytes: &[u8]) {
            let p = self.0.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
        fn git(&self, args: &[&str]) {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        fn commit(&self, msg: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        }
        fn check(&self, ci: bool, artifacts: Vec<PathBuf>) -> Vec<String> {
            check(&self.0, &Options { ci, artifacts }).unwrap().problems.into_iter().collect()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn home_path() -> String {
        format!("/{}/mallory/.ssh/id_ed25519", "home")
    }

    fn has(problems: &[String], needle: &str) -> bool {
        problems.iter().any(|p| p.contains(needle))
    }

    #[test]
    fn clean_repo_passes() {
        let r = Repo::new("clean");
        r.write("assets/ok/a.png", b"png");
        r.write("README", format!("see /{}/user/ and /work/x", "home").as_bytes());
        r.commit("ok");
        assert_eq!(r.check(false, vec![]), Vec::<String>::new());
    }

    #[test]
    fn committed_base_path_fails_even_after_removal() {
        let r = Repo::new("basepath");
        r.write("base/notes/x.txt", b"note");
        r.git(&["add", "-f", "base/notes/x.txt"]);
        r.commit("oops");
        r.git(&["rm", "-q", "--cached", "base/notes/x.txt"]);
        r.commit("remove");
        let p = r.check(true, vec![]);
        assert!(has(&p, "history: a commit touches local-only path base/notes/x.txt"), "{p:#?}");
        assert!(!has(&p, "under a local-only directory"));
    }

    #[test]
    fn home_path_only_in_history_fails() {
        let r = Repo::new("home");
        r.write("doc.txt", home_path().as_bytes());
        r.commit("leak");
        r.write("doc.txt", b"clean");
        r.commit("fix");
        let p = r.check(true, vec![]);
        assert!(has(&p, "(doc.txt): absolute home path"), "{p:#?}");
        assert!(!has(&p, "file doc.txt"));
    }

    #[test]
    fn renamed_forbidden_copy_fails_locally_not_in_ci() {
        let r = Repo::new("digest");
        r.write("base/bin/secret.dll", b"synthetic forbidden bytes");
        r.write("base/icons/i.png", b"icon bytes");
        r.write("assets/ok/renamed.png", b"synthetic forbidden bytes");
        r.write("assets/ok/icon.png", b"icon bytes");
        r.commit("copy");
        let p = r.check(false, vec![]);
        assert!(has(&p, "file assets/ok/renamed.png: identical to forbidden file base/bin/secret.dll"), "{p:#?}");
        assert!(has(&p, "(assets/ok/renamed.png): identical"));
        assert!(!has(&p, "icon.png"));
        assert!(r.check(true, vec![]).is_empty());
    }

    #[test]
    fn artifact_members_are_scanned() {
        let r = Repo::new("artifact");
        r.write("base/x.bin", b"forbidden member");
        r.write("stage/usr/share/x.dat", b"forbidden member");
        r.write("stage/usr/share/y.txt", home_path().as_bytes());
        let tar = r.0.join("dist.tar.gz");
        let ok = Command::new("tar").arg("-czf").arg(&tar).arg("-C").arg(r.0.join("stage")).arg(".").status().unwrap();
        assert!(ok.success());
        // Fake AppImage: extracts the same tree when asked.
        let app = r.0.join("rustxterm.AppImage");
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = --appimage-extract ] && mkdir squashfs-root && tar -xzf '{}' -C squashfs-root\n",
            tar.display()
        );
        fs::write(&app, script).unwrap();
        Command::new("chmod").arg("+x").arg(&app).status().unwrap();
        r.write(".gitignore", b"base/\nstage/\n*.tar.gz\n*.AppImage\n");
        r.commit("ignore");
        let p = r.check(false, vec![tar.clone(), app.clone()]);
        assert!(has(&p, "dist.tar.gz member usr/share/x.dat: identical to forbidden file base/x.bin"), "{p:#?}");
        assert!(has(&p, "dist.tar.gz member usr/share/y.txt: absolute home path"));
        assert!(has(&p, "rustxterm.AppImage member squashfs-root/usr/share/x.dat: identical"));
        assert_eq!(p.len(), 4, "{p:#?}");
    }

    /// Windows artifacts: zip members always; MSI contents where msitools exist (windows-toolchain
    /// image; the Linux image has no `wixl`, so that half is skipped there with a note).
    #[test]
    fn windows_zip_and_msi_members_are_scanned() {
        let r = Repo::new("winartifact");
        r.write("base/x.bin", b"forbidden member");
        r.write("stage/rustxterm/renamed.exe", b"forbidden member");
        r.write("stage/rustxterm/README.txt", home_path().as_bytes());
        r.write(".gitignore", b"base/\nstage/\n*.zip\n*.msi\n*.wxs\n");
        r.commit("ignore");
        let zip = r.0.join("rustxterm.zip");
        let ok = Command::new("python3")
            .args(["-m", "zipfile", "-c"])
            .arg(&zip)
            .arg(r.0.join("stage/rustxterm"))
            .status()
            .unwrap();
        assert!(ok.success());
        let mut artifacts = vec![zip];
        let wxs = r#"<?xml version="1.0"?><Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
            <Product Id="11111111-1111-1111-1111-111111111111" Name="t" Language="1033" Version="1.0.0"
              Manufacturer="t" UpgradeCode="22222222-2222-2222-2222-222222222222">
            <Package InstallerVersion="500" Compressed="yes" /><Media Id="1" Cabinet="t.cab" EmbedCab="yes" />
            <Directory Id="TARGETDIR" Name="SourceDir"><Directory Id="D" Name="t">
            <Component Id="C" Guid="33333333-3333-3333-3333-333333333333">
              <File Id="A" Name="renamed.exe" Source="stage/rustxterm/renamed.exe" />
              <File Id="B" Name="README.txt" Source="stage/rustxterm/README.txt" />
            </Component></Directory></Directory><Feature Id="F" Level="1"><ComponentRef Id="C" /></Feature>
            </Product></Wix>"#;
        r.write("t.wxs", wxs.as_bytes());
        let msi = r.0.join("rustxterm.msi");
        match Command::new("wixl").current_dir(&r.0).arg("-o").arg(&msi).arg("t.wxs").status() {
            Ok(s) => {
                assert!(s.success(), "wixl failed");
                artifacts.push(msi);
            }
            Err(_) => eprintln!("wixl not installed: MSI half skipped (run in the windows-toolchain image)"),
        }
        let n = artifacts.len();
        let p = r.check(false, artifacts);
        assert!(
            has(&p, "rustxterm.zip member rustxterm/renamed.exe: identical to forbidden file base/x.bin"),
            "{p:#?}"
        );
        assert!(has(&p, "rustxterm.zip member rustxterm/README.txt: absolute home path"), "{p:#?}");
        if n == 2 {
            assert!(has(&p, "rustxterm.msi member") && has(&p, "renamed.exe: identical to forbidden"), "{p:#?}");
            assert!(p.iter().any(|x| x.contains("rustxterm.msi member") && x.contains("absolute home path")), "{p:#?}");
        }
        assert_eq!(p.len(), 2 * n, "{p:#?}");
    }

    #[test]
    fn missing_attribution_row_fails() {
        let r = Repo::new("attr");
        r.write("assets/new/logo.svg", b"<svg/>");
        r.write("crates/x/tests/fixtures/sample.txt", b"data");
        let p = r.check(true, vec![]);
        assert!(has(&p, "file assets/new/logo.svg: no ATTRIBUTION.md row"), "{p:#?}");
        assert!(has(&p, "file crates/x/tests/fixtures/sample.txt: no ATTRIBUTION.md row"));
        r.write("ATTRIBUTION.md", b"| Path | Origin | License |\n|---|---|---|\n| `assets/**/*.svg` | ours | MIT |\n| `crates/*/tests/fixtures/**` | ours | MIT |\n");
        assert!(r.check(true, vec![]).is_empty());
    }

    #[test]
    fn unlisted_font_file_fails() {
        let r = Repo::new("fonts");
        r.write("assets/fonts/demo/Demo-Regular.ttf", b"font");
        r.write("assets/fonts/demo/OFL.txt", b"license");
        let p = r.check(true, vec![]);
        assert!(has(&p, "file assets/fonts/demo/Demo-Regular.ttf: no ATTRIBUTION.md row"), "{p:#?}");
        assert!(has(&p, "file assets/fonts/demo/OFL.txt: no ATTRIBUTION.md row"));
        r.write(
            "ATTRIBUTION.md",
            b"| Path | Origin | License |\n|---|---|---|\n| `assets/fonts/demo/Demo-Regular.ttf` | demo | OFL-1.1 |\n",
        );
        let p = r.check(true, vec![]);
        assert_eq!(p, vec!["file assets/fonts/demo/OFL.txt: no ATTRIBUTION.md row".to_string()]);
    }

    #[test]
    fn globs() {
        assert!(glob_match("crates/*/tests/snapshots/**", "crates/ui/tests/snapshots/a/b.png"));
        assert!(glob_match("a/**/c.png", "a/c.png"));
        assert!(glob_match("a/*.png", "a/x.png"));
        assert!(!glob_match("a/*.png", "a/b/x.png"));
        assert!(!glob_match("a/x.png", "a/x.pngx"));
    }
}
