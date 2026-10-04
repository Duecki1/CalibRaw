//! `cargo xtask arch-check`: automated architecture boundary checks.
//!
//! Each rule inspects a package's resolved normal and build dependency graph
//! for every target platform (`cargo tree -e normal,build --target all`). The
//! check fails when `cargo tree` itself fails, so a lockfile or network error
//! is never mistaken for an absent dependency. The rules are listed in
//! `docs/ARCHITECTURE.md`; add one when a boundary is established.

use crate::process::{display_relative, run_command_output, source_files, workspace_root};
use crate::rust_source::{is_word_at, RustSource};
use crate::{Result, XtaskError};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Presentation frameworks that headless crates must not pull in.
const UI_FRAMEWORK: &[&str] = &["egui", "egui-wgpu", "eframe", "egui-winit"];

struct DependencyRule {
    package: &'static str,
    forbidden: &'static [&'static [&'static str]],
    forbidden_prefix: Option<&'static str>,
    reason: &'static str,
}

const DEPENDENCY_RULES: &[DependencyRule] = &[
    DependencyRule {
        package: "calibraw-core",
        forbidden: &[
            &["calibraw-gpu", "calibraw-ai", "calibraw-ui", "calibraw-ffi"],
            UI_FRAMEWORK,
        ],
        forbidden_prefix: None,
        reason: "core owns domain models and must not depend on processing, platform or UI crates",
    },
    DependencyRule {
        package: "calibraw-gpu",
        forbidden: &[
            &["calibraw-ai", "calibraw-ui", "calibraw-ffi"],
            UI_FRAMEWORK,
        ],
        forbidden_prefix: None,
        reason: "headless GPU processing must not depend on presentation or platform bridges",
    },
    DependencyRule {
        package: "calibraw-ai",
        forbidden: &[&["calibraw-ui", "calibraw-ffi"], UI_FRAMEWORK],
        forbidden_prefix: None,
        reason:
            "AI inference runs headless and must not depend on presentation or platform bridges",
    },
    DependencyRule {
        package: "calibraw-ffi",
        forbidden: &[
            &["calibraw-gpu", "calibraw-ai", "calibraw-ui"],
            UI_FRAMEWORK,
        ],
        forbidden_prefix: None,
        reason: "the platform bridge exposes storage and codecs, not processing or presentation",
    },
    DependencyRule {
        package: "calibraw-cli",
        forbidden: &[&["calibraw-ui", "calibraw-ffi"], UI_FRAMEWORK],
        forbidden_prefix: None,
        reason: "the standalone CLI is headless",
    },
    DependencyRule {
        package: "moduwu-design",
        forbidden: &[],
        forbidden_prefix: Some("calibraw"),
        reason: "the design library is presentation-only and must not depend on CalibRaw",
    },
];

/// Modules that must stay free of some crates even though their package
/// depends on them (a crate-level rule cannot express this).
struct SourceRule {
    directory: &'static str,
    forbidden: &'static [&'static str],
    reason: &'static str,
}

const SOURCE_RULES: &[SourceRule] = &[SourceRule {
    directory: "crates/calibraw-ui/src/services",
    forbidden: &["egui", "eframe", "egui_wgpu"],
    reason: "application services are presentation-independent",
}];

pub(crate) fn run() -> Result<()> {
    let root = workspace_root();
    let mut failures = Vec::new();
    for rule in SOURCE_RULES {
        let uses = forbidden_uses(&root, rule)?;
        if uses.is_empty() {
            println!("ok    {}: {}", rule.directory, rule.reason);
        } else {
            for (file, line, name) in &uses {
                println!("FAIL  {file}:{line} uses `{name}`: {}", rule.reason);
            }
            failures.push(rule.directory);
        }
    }
    for rule in DEPENDENCY_RULES {
        let output = run_command_output(
            Command::new("cargo")
                .current_dir(&root)
                .args(["tree", "--locked", "-p", rule.package])
                .args(["-e", "normal,build", "--target", "all"])
                .args(["--prefix", "none", "--format", "{p}"]),
            &format!("cargo tree -p {}", rule.package),
        )?;
        let packages = package_names(&output);
        if !packages.contains(rule.package) {
            return Err(XtaskError::new(format!(
                "cargo tree output for {} does not contain the package itself",
                rule.package
            )));
        }
        let mut violations = rule
            .forbidden
            .iter()
            .flat_map(|group| group.iter())
            .filter(|name| packages.contains(**name))
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        if let Some(prefix) = rule.forbidden_prefix {
            violations.extend(
                packages
                    .iter()
                    .filter(|name| name.starts_with(prefix))
                    .cloned(),
            );
        }
        if violations.is_empty() {
            println!("ok    {}: {}", rule.package, rule.reason);
        } else {
            println!(
                "FAIL  {} depends on {}: {}",
                rule.package,
                violations.join(", "),
                rule.reason
            );
            failures.push(rule.package);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::new(format!(
            "architecture boundaries violated by {}",
            failures.join(", ")
        )))
    }
}

/// `(file, line, identifier)` of every forbidden identifier in code (not
/// comments or strings) below the rule's directory.
fn forbidden_uses(root: &Path, rule: &SourceRule) -> Result<Vec<(String, usize, String)>> {
    let directory = root.join(rule.directory);
    if !directory.is_dir() {
        return Err(XtaskError::new(format!(
            "architecture rule directory {} does not exist",
            rule.directory
        )));
    }
    let mut uses = Vec::new();
    for file in source_files(&directory, &["rs"])? {
        let source = RustSource::new(fs::read_to_string(&file)?);
        uses.extend(
            identifier_uses(&source, rule.forbidden)
                .into_iter()
                .map(|(line, name)| (display_relative(root, &file), line, name)),
        );
    }
    Ok(uses)
}

fn identifier_uses(source: &RustSource, names: &[&str]) -> Vec<(usize, String)> {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let mut uses = Vec::new();
    for name in names {
        for (offset, _) in structural.match_indices(name) {
            if is_word_at(bytes, offset, name.len()) {
                uses.push((source.line_of(offset) + 1, (*name).to_owned()));
            }
        }
    }
    uses.sort();
    uses
}

/// Package names from `cargo tree --prefix none --format {p}` output.
fn package_names(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_rules_ignore_comments_strings_and_longer_words() {
        let source = RustSource::new(
            "//! Must not use egui.\nuse eframe::egui;\nlet text = \"egui\";\nlet eguix = 1;\n",
        );
        assert_eq!(
            identifier_uses(&source, &["egui", "eframe"]),
            [(2, "eframe".to_owned()), (2, "egui".to_owned())]
        );
    }

    #[test]
    fn package_names_ignore_versions_sources_and_duplicates() {
        let names = package_names(
            "calibraw-core v1.1.1 (/repo/crates/calibraw-core)\nserde v1.0.0\nserde v1.0.0 (*)\n\n",
        );
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["calibraw-core", "serde"]
        );
    }
}
