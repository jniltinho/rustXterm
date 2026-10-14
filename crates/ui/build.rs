//! Embed every icon listed in `assets/icons/app/catalog.json` at every size present
//! (16/32/48/64/96 px PNGs) so the binary stays self-contained: writes `$OUT_DIR/icons.rs` with
//! one `include_bytes!` per file.

use std::fmt::Write as _;
use std::path::Path;

/// Bitmap sizes looked up per icon, ascending.
const SIZES: [u32; 5] = [16, 32, 48, 64, 96];

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let dir = Path::new(&manifest).join("../../assets/icons/app");
    let catalog = dir.join("catalog.json");
    println!("cargo:rerun-if-changed={}", catalog.display());
    for size in SIZES {
        println!("cargo:rerun-if-changed={}", dir.join(size.to_string()).display());
    }
    let text = std::fs::read_to_string(&catalog).unwrap_or_default();
    let mut out = String::from("&[\n");
    // The catalog is a flat array of objects; read each object's "name" and "category" strings.
    for obj in text.split('}') {
        let (Some(name), Some(category)) = (field(obj, "name"), field(obj, "category")) else { continue };
        let files: Vec<String> = SIZES
            .iter()
            .filter_map(|size| {
                let path = dir.join(size.to_string()).join(format!("{name}.png"));
                path.is_file().then(|| format!("({size}, include_bytes!({:?}) as &[u8])", path.display().to_string()))
            })
            .collect();
        if files.is_empty() {
            println!("cargo:warning=icon {name} has no PNG; skipped");
            continue;
        }
        let _ = writeln!(out, "    ({name:?}, {category:?}, &[{}]),", files.join(", "));
    }
    out.push(']');
    let target = Path::new(&std::env::var("OUT_DIR").unwrap_or_default()).join("icons.rs");
    if let Err(e) = std::fs::write(&target, out) {
        panic!("cannot write {}: {e}", target.display());
    }
}

/// Value of `"key": "value"` inside one JSON object (icon names never contain quotes).
fn field<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let rest = &obj[obj.find(&format!("\"{key}\""))? + key.len() + 2..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(&rest[..rest.find('"')?])
}
