//! `cargo xtask cfg-ratchet`: keeps `target_os` conditions from spreading.
//!
//! Every `cfg(...)`, `cfg!(...)` and `cfg_attr(...)` whose predicate mentions
//! `target_os` counts once, in code only (not comments or strings), test code
//! included. Counts per file are compared with `xtask/cfg-baseline.txt`: a
//! file fails when its count rises, or when a file without a baseline entry
//! gains its first one. Files under `ALLOWED_DIRECTORIES` are not counted,
//! because platform code belongs there.
//!
//! A drop passes and prints the lowered entries; `--update` writes them to
//! the baseline and refuses while any count rose, so raising an entry is
//! always a reviewed edit of the baseline file.

use crate::process::{display_relative, source_files, workspace_root};
use crate::rust_source::{identifier_end, is_word_at, RustSource};
use crate::{Result, XtaskError};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const BASELINE: &str = "xtask/cfg-baseline.txt";
/// Workspace directories whose Rust files are counted.
const SCANNED_ROOTS: &[&str] = &["crates", "xtask"];
/// Directories that own platform differences; `target_os` is free there.
/// Filled as the platform modules of docs/GPU_REWORK.md (P1, P3, P7) appear.
const ALLOWED_DIRECTORIES: &[&str] = &[
    "crates/calibraw-ai/src/placement",
    "crates/calibraw-ai/src/runtime",
    "crates/calibraw-gpu/src/memory",
    "crates/calibraw-ui/src/platform",
];

/// `target_os` conditions per workspace-relative file path.
type Counts = BTreeMap<String, usize>;

#[derive(Debug, Default, PartialEq, Eq)]
struct Report {
    /// `(file, baseline, current)` for each file above its baseline.
    rose: Vec<(String, usize, usize)>,
    /// `(file, current)` for each file without a baseline entry.
    new: Vec<(String, usize)>,
    /// `(file, baseline, current)` for each file below its baseline,
    /// including files that were removed or moved to an allowed directory.
    fell: Vec<(String, usize, usize)>,
}

impl Report {
    fn failed(&self) -> bool {
        !self.rose.is_empty() || !self.new.is_empty()
    }
}

pub(crate) fn run(args: Vec<OsString>) -> Result<()> {
    let update = match args.as_slice() {
        [] => false,
        [flag] if flag == "--update" => true,
        _ => {
            return Err(XtaskError::usage(
                "usage: cargo xtask cfg-ratchet [--update]",
            ))
        }
    };
    let root = workspace_root();
    let baseline_path = root.join(BASELINE);
    let baseline = parse_baseline(&fs::read_to_string(&baseline_path)?)
        .map_err(|error| XtaskError::new(format!("{BASELINE}: {}", error.message)))?;
    let current = scan(&root)?;
    let report = compare(&current, &baseline);

    for (file, before, now) in &report.rose {
        println!("FAIL  {file}: {now} target_os conditions, baseline {before}");
    }
    for (file, now) in &report.new {
        println!("FAIL  {file}: {now} target_os conditions in a file without any in the baseline");
    }
    for (file, before, now) in &report.fell {
        println!("lower {file}: {before} -> {now}");
    }
    if report.failed() {
        return Err(XtaskError::new(format!(
            "target_os conditions spread to {} files; decide platform differences in one \
             place instead, or raise {BASELINE} in a reviewed change",
            report.rose.len() + report.new.len()
        )));
    }

    let total = current.values().sum::<usize>();
    if report.fell.is_empty() {
        println!(
            "ok    {total} target_os conditions in {} files; none rose",
            current.len()
        );
    } else if update {
        fs::write(&baseline_path, render_baseline(&current))?;
        println!("ok    {BASELINE} lowered to {total} conditions");
    } else {
        println!(
            "ok    {total} target_os conditions; run `cargo xtask cfg-ratchet --update` \
             to lower {BASELINE}"
        );
    }
    Ok(())
}

/// Counts of every scanned file with at least one condition, outside the
/// allowed directories.
fn scan(root: &Path) -> Result<Counts> {
    let mut counts = Counts::new();
    for scanned in SCANNED_ROOTS {
        for file in source_files(&root.join(scanned), &["rs"])? {
            let path = display_relative(root, &file);
            if is_under(&path, ALLOWED_DIRECTORIES) {
                continue;
            }
            let count = target_os_conditions(&RustSource::new(fs::read_to_string(&file)?));
            if count > 0 {
                counts.insert(path, count);
            }
        }
    }
    Ok(counts)
}

fn is_under(path: &str, directories: &[&str]) -> bool {
    directories.iter().any(|directory| {
        path.strip_prefix(directory)
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Number of `cfg`, `cfg!` and `cfg_attr` conditions that mention
/// `target_os`. A condition nested in a counted one is not counted again.
fn target_os_conditions(source: &RustSource) -> usize {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let mut count = 0;
    let mut counted_until = 0;
    for (start, _) in structural.match_indices("cfg") {
        if start < counted_until {
            continue;
        }
        let name_end = identifier_end(bytes, start);
        let name = &structural[start..name_end];
        if !matches!(name, "cfg" | "cfg_attr") || !is_word_at(bytes, start, name.len()) {
            continue;
        }
        let rest = structural[name_end..].trim_start();
        let rest = rest.strip_prefix('!').map_or(rest, str::trim_start);
        if !rest.starts_with('(') {
            continue;
        }
        let open = structural.len() - rest.len();
        let Some(close) = source.matching_close(open) else {
            continue;
        };
        let mentions_target_os = structural[open..close]
            .match_indices("target_os")
            .any(|(offset, word)| is_word_at(bytes, open + offset, word.len()));
        if mentions_target_os {
            count += 1;
            counted_until = close;
        }
    }
    count
}

fn compare(current: &Counts, baseline: &Counts) -> Report {
    let mut report = Report::default();
    for (file, &now) in current {
        match baseline.get(file) {
            None => report.new.push((file.clone(), now)),
            Some(&before) if now > before => report.rose.push((file.clone(), before, now)),
            Some(&before) if now < before => report.fell.push((file.clone(), before, now)),
            Some(_) => {}
        }
    }
    for (file, &before) in baseline {
        if !current.contains_key(file) {
            report.fell.push((file.clone(), before, 0));
        }
    }
    report.fell.sort();
    report
}

/// Reads `<count> <path>` lines; `#` starts a comment line.
fn parse_baseline(text: &str) -> Result<Counts> {
    let mut counts = Counts::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid = || XtaskError::new(format!("line {}: expected `<count> <path>`", index + 1));
        let (count, path) = line.split_once(char::is_whitespace).ok_or_else(invalid)?;
        let count = count.parse::<usize>().map_err(|_| invalid())?;
        let path = path.trim();
        if count == 0 || counts.insert(path.to_owned(), count).is_some() {
            return Err(XtaskError::new(format!(
                "line {}: {path} has a zero count or appears twice",
                index + 1
            )));
        }
    }
    Ok(counts)
}

fn render_baseline(counts: &Counts) -> String {
    let mut text = String::from(
        "# `target_os` conditions per file, checked by `cargo xtask cfg-ratchet`.\n\
         # Counts may only fall: `cargo xtask cfg-ratchet --update` lowers them.\n\
         # Raising one, or adding a file, is a reviewed change to this file.\n",
    );
    for (path, count) in counts {
        // Writing to a String cannot fail.
        let _ = writeln!(text, "{count:>3} {path}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(entries: &[(&str, usize)]) -> Counts {
        entries
            .iter()
            .map(|(path, count)| ((*path).to_owned(), *count))
            .collect()
    }

    #[test]
    fn counts_each_condition_once_in_code_only() {
        let source = RustSource::new(
            "#[cfg(target_os = \"android\")]\n\
             fn a() {}\n\
             #[cfg(any(target_os = \"linux\",\n    target_os = \"windows\"))]\n\
             fn b() {}\n\
             #[cfg_attr(not(target_os = \"android\"), allow(dead_code))]\n\
             fn c() { if cfg!(target_os = \"macos\") {} }\n\
             #[cfg(test)]\n\
             mod tests {}\n\
             // #[cfg(target_os = \"android\")]\n\
             const TEXT: &str = \"cfg(target_os)\";\n\
             #[cfg(my_target_os)]\n\
             fn d() {}\n\
             fn cfg_target_os() {}\n",
        );
        assert_eq!(target_os_conditions(&source), 4);
    }

    #[test]
    fn nested_conditions_count_once() {
        let source = RustSource::new(
            "#[cfg_attr(target_os = \"android\", cfg(not(target_os = \"linux\")))]\nfn a() {}\n",
        );
        assert_eq!(target_os_conditions(&source), 1);
    }

    #[test]
    fn rises_and_new_files_fail_while_drops_pass() {
        let baseline = counts(&[("a.rs", 3), ("b.rs", 2), ("gone.rs", 1), ("same.rs", 4)]);
        let current = counts(&[("a.rs", 4), ("b.rs", 1), ("new.rs", 1), ("same.rs", 4)]);
        let report = compare(&current, &baseline);
        assert_eq!(
            report,
            Report {
                rose: vec![("a.rs".to_owned(), 3, 4)],
                new: vec![("new.rs".to_owned(), 1)],
                fell: vec![("b.rs".to_owned(), 2, 1), ("gone.rs".to_owned(), 1, 0)],
            }
        );
        assert!(report.failed());
        assert!(!compare(&counts(&[("a.rs", 2)]), &baseline).failed());
    }

    #[test]
    fn baseline_round_trips_and_rejects_malformed_lines() {
        let entries = counts(&[("crates/a/src/lib.rs", 12), ("xtask/src/main.rs", 1)]);
        assert_eq!(parse_baseline(&render_baseline(&entries)).unwrap(), entries);
        for invalid in ["x a.rs", "3", "0 a.rs", "1 a.rs\n2 a.rs"] {
            assert!(parse_baseline(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn allowed_directories_match_whole_path_components() {
        let allowed = ["crates/x/src/platform"];
        assert!(is_under("crates/x/src/platform/mod.rs", &allowed));
        assert!(is_under("crates/x/src/platform/a/b.rs", &allowed));
        assert!(!is_under("crates/x/src/platforms.rs", &allowed));
        assert!(!is_under("crates/x/src/platform.rs", &allowed));
    }
}
