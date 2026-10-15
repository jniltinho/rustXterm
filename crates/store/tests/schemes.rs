#![allow(clippy::unwrap_used, clippy::expect_used)]

use rxt_core::TerminalScheme;
use rxt_store::schemes;

#[test]
fn load_skips_invalid_oversized_and_duplicate_names() {
    let dir = std::env::temp_dir().join(format!("rxt-schemes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let named = |name: &str| {
        let mut s = TerminalScheme::dark();
        s.name = name.into();
        serde_json::to_string(&s).unwrap()
    };
    std::fs::write(dir.join("a.json"), named("Mine")).unwrap();
    std::fs::write(dir.join("b.json"), named("Mine")).unwrap();
    std::fs::write(dir.join("c.json"), named("Dracula")).unwrap();
    std::fs::write(dir.join("d.json"), "{ not json").unwrap();
    let mut big = named("Big");
    big.push_str(&" ".repeat(schemes::MAX_FILE as usize));
    std::fs::write(dir.join("e.json"), big).unwrap();
    std::fs::write(dir.join("f.json"), named("Other")).unwrap();
    std::fs::write(dir.join("notes.txt"), named("Text")).unwrap();

    let loaded = schemes::load(&dir, &["Dracula"]);
    let names: Vec<&str> = loaded.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Mine", "Other"]);
    assert!(schemes::load(&dir.join("missing"), &[]).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}
