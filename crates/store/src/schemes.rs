//! User terminal color schemes: `schemes/*.json` next to `config.toml`, loaded once at start.

use std::path::Path;

use rxt_core::TerminalScheme;

/// Largest scheme file read; bigger files are skipped.
pub const MAX_FILE: u64 = 64 * 1024;

/// Schemes in `dir` (`*.json`, file-name order). Files that are too big, invalid, or whose name is
/// already in `taken` or used by an earlier file are skipped with a warning. A missing folder is empty.
pub fn load(dir: &Path, taken: &[&str]) -> Vec<TerminalScheme> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .collect();
    paths.sort();
    let mut out: Vec<TerminalScheme> = Vec::new();
    for path in paths {
        let scheme = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_FILE => Err(format!("larger than {MAX_FILE} bytes")),
            Ok(_) => std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str::<TerminalScheme>(&text).map_err(|e| e.to_string())),
            Err(e) => Err(e.to_string()),
        };
        match scheme {
            Ok(s) if taken.contains(&s.name.as_str()) || out.iter().any(|o| o.name == s.name) => {
                tracing::warn!("skipping color scheme {}: name {:?} is already used", path.display(), s.name);
            }
            Ok(s) => out.push(s),
            Err(e) => tracing::warn!("skipping color scheme {}: {e}", path.display()),
        }
    }
    out
}
