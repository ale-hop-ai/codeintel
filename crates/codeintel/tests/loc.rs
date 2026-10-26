//! Non-test LOC per crate, pinned: `specs/00-overview.md` § Layout (#149).
//!
//! The number this project exists to keep small had nothing tracking it. This
//! is no cap — a move is fine, an unexplained one is not. Re-read the diff,
//! then `UPDATE_GOLDEN=1 cargo test -p codeintel --test loc`, and say in the
//! commit message why it moved.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const HEADER: &str = "crate\tlines\tcode\n";

#[test]
fn non_test_loc_is_what_the_golden_says() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("crates/ is readable")
        .map(|e| e.expect("a crates/ entry").path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    crates.sort();

    let mut got = String::from(HEADER);
    let (mut all_lines, mut all_code) = (0, 0);
    for src in &crates {
        let (mut lines, mut code) = (0, 0);
        for file in counted(src) {
            let text = std::fs::read_to_string(&file).expect("a source file is UTF-8");
            let (l, c) = count(&text);
            lines += l;
            code += c;
        }
        let name = src
            .parent()
            .and_then(Path::file_name)
            .expect("crates/<name>/src")
            .to_string_lossy();
        writeln!(got, "{name}\t{lines}\t{code}").expect("writes to a String");
        all_lines += lines;
        all_code += code;
    }
    writeln!(got, "total\t{all_lines}\t{all_code}").expect("writes to a String");

    let golden = root.join("tests/fixtures/loc.tsv");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &got).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert!(
        expected == got,
        "non-test LOC moved. Say why in the commit message, then \
         `UPDATE_GOLDEN=1 cargo test -p codeintel --test loc`\nexpected:\n{expected}got:\n{got}"
    );
}

/// Every `.rs` under `src`, minus the files a `#[cfg(test)] mod NAME;` names.
fn counted(src: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("a src/ directory is readable") {
            let path = entry.expect("a src/ entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    let mut excluded = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a source file is UTF-8");
        let dir = file.parent().expect("a file has a directory");
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            if line.trim() != "#[cfg(test)]" {
                continue;
            }
            let item = lines.next().unwrap_or_default().trim();
            if let Some(name) = item.strip_prefix("mod ").and_then(|m| m.strip_suffix(';')) {
                excluded.push(dir.join(format!("{name}.rs")));
                excluded.push(dir.join(name).join("mod.rs"));
            }
        }
    }
    files.retain(|f| !excluded.contains(f));
    files
}

/// `(lines, code)` for one file, with every `#[cfg(test)]` item dropped.
fn count(text: &str) -> (usize, usize) {
    let (mut lines, mut code) = (0, 0);
    let mut iter = text.lines();
    while let Some(line) = iter.next() {
        if line.trim() == "#[cfg(test)]" {
            let indent = line.len() - line.trim_start().len();
            let close = format!("{}}}", " ".repeat(indent));
            let first = iter.next().unwrap_or_default();
            if !first.trim_end().ends_with(';') {
                for rest in iter.by_ref() {
                    if rest == close {
                        break;
                    }
                }
            }
            continue;
        }
        lines += 1;
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            code += 1;
        }
    }
    (lines, code)
}
