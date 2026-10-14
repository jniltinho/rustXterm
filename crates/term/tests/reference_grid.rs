//! Replays recorded vim/htop/tmux output (80x24, see ATTRIBUTION.md) into a fixed-size
//! terminal and compares the grid with a committed reference dump.
//! Regenerate with `UPDATE_SNAPSHOTS=1`.
#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use rxt_term::{Cell, Color, Flags, NamedColor, Terminal, point_to_viewport};

/// Row, first and last column, (fg, bg, flags).
type AttrRun = Option<(usize, usize, usize, (Color, Color, Flags))>;

fn dump(term: &Terminal) -> String {
    let content = term.renderable();
    let offset = content.display_offset;
    let mut out = format!(
        "size {}x{}\ncursor {} {} {:?}\nmode {:?}\n--- text\n",
        term.cols(),
        term.rows(),
        content.cursor.point.line.0,
        content.cursor.point.column.0,
        content.cursor.shape,
        content.mode,
    );
    let cells: Vec<(usize, usize, Cell)> = content
        .display_iter
        .filter_map(|c| point_to_viewport(offset, c.point).map(|p| (p.line, p.column.0, c.cell.clone())))
        .collect();
    for row in 0..term.rows() {
        let line: String = cells
            .iter()
            .filter(|(r, _, c)| *r == row && !c.flags.contains(Flags::WIDE_CHAR_SPACER))
            .map(|(_, _, c)| c.c)
            .collect();
        writeln!(out, "{:2}|{}", row, line.trim_end()).ok();
    }
    out.push_str("--- attributes (runs differing from default)\n");
    let default = (Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::empty());
    let mut run: AttrRun = None;
    let flush = |run: &mut AttrRun, out: &mut String| {
        if let Some((row, from, to, (fg, bg, flags))) = run.take()
            && (fg, bg, flags) != default
        {
            writeln!(out, "{row:2}:{from}-{to} fg={fg:?} bg={bg:?} flags={flags:?}").ok();
        }
    };
    for (row, col, cell) in &cells {
        let attrs = (cell.fg, cell.bg, cell.flags - Flags::WRAPLINE);
        match &mut run {
            Some((r, _, to, a)) if r == row && *a == attrs => *to = *col,
            _ => {
                flush(&mut run, &mut out);
                run = Some((*row, *col, *col, attrs));
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

fn check(name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let raw = std::fs::read(dir.join(format!("{name}.raw"))).unwrap();
    let mut term = Terminal::new(80, 24, 1000);
    // Feed in odd-sized chunks to exercise split sequences.
    for chunk in raw.chunks(97) {
        term.feed(chunk);
    }
    let got = dump(&term);
    let reference = dir.join(format!("{name}.grid"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&reference, &got).unwrap();
    }
    let want = std::fs::read_to_string(&reference).unwrap();
    assert!(got == want, "grid mismatch for {name}; rerun with UPDATE_SNAPSHOTS=1 and review the diff\n{got}");
}

#[test]
fn vim() {
    check("vim");
}

#[test]
fn htop() {
    check("htop");
}

#[test]
fn tmux() {
    check("tmux");
}
