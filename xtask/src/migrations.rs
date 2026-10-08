//! `cargo xtask migrations`: lists the compatibility code for older files and
//! fails once any of it is due for removal.
//!
//! Compatibility code is marked with `migration: remove in v` followed by a
//! version, in a comment next to it. The command reports every marker under
//! `crates/` and fails when the workspace version has reached one, or when a
//! file in the core `migrations` folder has no marker.

use crate::process::{display_relative, workspace_root};
use crate::{Result, XtaskError};
use std::fs;
use std::path::{Path, PathBuf};

const MARKER: &str = "migration: remove in v";
const SCANNED_ROOT: &str = "crates";
const MIGRATIONS_FOLDER: &str = "crates/calibraw-core/src/migrations";

type Version = [u32; 3];

#[derive(Debug, PartialEq, Eq)]
struct Marker {
    line: usize,
    remove_in: Version,
}

pub(crate) fn run() -> Result<()> {
    let root = workspace_root();
    let current = parse_version(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| XtaskError::new("the workspace version is not MAJOR.MINOR.PATCH"))?;

    let mut files = Vec::new();
    collect_sources(&root.join(SCANNED_ROOT), &mut files)?;
    files.sort();

    let mut problems = Vec::new();
    let mut count = 0;
    for file in &files {
        let relative = display_relative(&root, file);
        let markers = match markers(&fs::read_to_string(file)?) {
            Ok(markers) => markers,
            Err(line) => {
                problems.push(format!(
                    "{relative}:{line}: write the version as `{MARKER}MAJOR.MINOR.PATCH`"
                ));
                continue;
            }
        };
        let in_migrations = file.starts_with(root.join(MIGRATIONS_FOLDER));
        if in_migrations && markers.is_empty() && file.file_name() != Some("mod.rs".as_ref()) {
            problems.push(format!(
                "{relative}: a migration needs a `{MARKER}…` marker"
            ));
        }
        for marker in markers {
            count += 1;
            let [major, minor, patch] = marker.remove_in;
            let due = marker.remove_in <= current;
            println!(
                "{}  {relative}:{}  remove in v{major}.{minor}.{patch}",
                if due { "due  " } else { "keep " },
                marker.line
            );
            if due {
                problems.push(format!(
                    "{relative}:{}: due for removal since v{major}.{minor}.{patch}; delete it with its tests",
                    marker.line
                ));
            }
        }
    }

    let [major, minor, patch] = current;
    if problems.is_empty() {
        println!("ok    migrations: {count} markers, none due at v{major}.{minor}.{patch}");
        Ok(())
    } else {
        for problem in &problems {
            eprintln!("{problem}");
        }
        Err(XtaskError::new(format!(
            "{} migration problem(s) at v{major}.{minor}.{patch}",
            problems.len()
        )))
    }
}

fn collect_sources(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name() != Some("target".as_ref()) {
                collect_sources(&path, files)?;
            }
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("rs" | "wgsl")
        ) {
            files.push(path);
        }
    }
    Ok(())
}

/// The markers in `source`. A marker is the prefix followed by a digit; any
/// other text after the prefix is prose about markers. Errs with the line of
/// a marker whose version cannot be read.
fn markers(source: &str) -> std::result::Result<Vec<Marker>, usize> {
    let mut found = Vec::new();
    for (index, text) in source.lines().enumerate() {
        let mut rest = text;
        while let Some(start) = rest.find(MARKER) {
            rest = &rest[start + MARKER.len()..];
            if !rest.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let end = rest
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(rest.len());
            let version = rest[..end].trim_end_matches('.');
            let remove_in = parse_version(version).ok_or(index + 1)?;
            found.push(Marker {
                line: index + 1,
                remove_in,
            });
        }
    }
    Ok(found)
}

fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.split('.').map(|part| part.parse::<u32>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_read_with_their_lines_and_prose_is_ignored() {
        let source = "\
//! Each one is marked `migration: remove in vX.Y.Z`.
// migration: remove in v2.0.0. Old masks.
fn current() {}
/// migration: remove in v1.4.0, with the step above.
";
        assert_eq!(
            markers(source),
            Ok(vec![
                Marker {
                    line: 2,
                    remove_in: [2, 0, 0]
                },
                Marker {
                    line: 4,
                    remove_in: [1, 4, 0]
                },
            ])
        );
    }

    #[test]
    fn incomplete_versions_are_reported() {
        assert_eq!(markers("// migration: remove in v2.0 soon"), Err(1));
        assert_eq!(markers("\n// migration: remove in v2"), Err(2));
    }

    #[test]
    fn versions_compare_by_component() {
        assert!(parse_version("1.10.0") > parse_version("1.9.3"));
        assert_eq!(parse_version("1.2.0"), Some([1, 2, 0]));
        assert_eq!(parse_version("1.2.0.1"), None);
        assert_eq!(parse_version("1.x.0"), None);
    }
}
