//! `cargo xtask ui-lint`: finds UI code that bypasses an established shared
//! control or re-exports shared primitives.
//!
//! Only production modules of the UI package are scanned; comments, string
//! contents and test-only items are ignored. Each finding is identified by its
//! rule, the function containing it and its normalized source line, never by
//! file or line number: moving code keeps its approval, while replacing one
//! violation with another needs a new one.
//!
//! Approved findings live in `xtask/ui-lint-baseline.json`, each with a
//! reason. An approval that no longer matches anything fails the check too, so
//! the baseline shrinks as areas migrate. `--suggest` prints entries for the
//! current unapproved findings; their reasons must be written before they pass.

use crate::loc::{module_categories, test_item_ranges, Category};
use crate::process::{display_relative, workspace_root};
use crate::rust_source::{is_word_at, RustSource};
use crate::{Result, XtaskError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::ops::Range;
use std::path::Path;

const PACKAGE_ROOTS: &[&str] = &[
    "crates/calibraw-ui/src/lib.rs",
    "crates/calibraw-ui/src/main.rs",
];
const BASELINE: &str = "xtask/ui-lint-baseline.json";
const UNWRITTEN_REASON: &str = "TODO: explain why this cannot use the shared control";

struct Rule {
    id: &'static str,
    /// Code (not comments or strings) that triggers the rule, matched at
    /// identifier boundaries.
    patterns: &'static [&'static str],
    advice: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        id: "number-field",
        patterns: &["DragValue"],
        advice: "use moduwu_design::NumberField",
    },
    Rule {
        id: "slider",
        patterns: &["egui::Slider"],
        advice: "use moduwu_design::Slider, or AdjustmentSlider for parameter specifications",
    },
    Rule {
        id: "combo-box",
        patterns: &["ComboBox"],
        advice: "use moduwu_design::combo_box, form_combo or responsive_combo_box",
    },
    Rule {
        id: "text-field",
        patterns: &["TextEdit::singleline"],
        advice: "use moduwu_design::singleline_text_edit or dialog_text_field",
    },
    Rule {
        id: "dialog-window",
        patterns: &["Window::new"],
        advice: "use moduwu_design::dialog_window",
    },
    Rule {
        id: "moduwu-reexport",
        patterns: &[
            "pub use moduwu_design",
            "pub(crate) use moduwu_design",
            "pub(super) use moduwu_design",
        ],
        advice: "import shared primitives from moduwu_design directly",
    },
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Key {
    rule: String,
    /// The innermost enclosing function, or `<module>`.
    function: String,
    /// The finding's source line with whitespace collapsed.
    code: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Approval {
    #[serde(flatten)]
    key: Key,
    reason: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Baseline {
    approved: Vec<Approval>,
}

#[derive(Debug)]
struct Finding {
    key: Key,
    file: String,
    line: usize,
}

#[derive(Debug, Default)]
struct Report {
    unapproved: Vec<Finding>,
    stale: Vec<Key>,
    unexplained: Vec<Key>,
    approved: usize,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<()> {
    let suggest = match args.as_slice() {
        [] => false,
        [flag] if flag == "--suggest" => true,
        _ => return Err(XtaskError::usage("usage: cargo xtask ui-lint [--suggest]")),
    };
    let root = workspace_root();
    let baseline_path = root.join(BASELINE);
    let baseline: Baseline = serde_json::from_str(&fs::read_to_string(&baseline_path)?)
        .map_err(|error| XtaskError::new(format!("{BASELINE}: {error}")))?;
    let findings = scan(&root)?;
    let report = check(findings, &baseline);

    for finding in &report.unapproved {
        let advice = RULES
            .iter()
            .find(|rule| rule.id == finding.key.rule)
            .map_or("", |rule| rule.advice);
        println!(
            "FAIL  {}:{} [{}] {}\n      {advice}",
            finding.file, finding.line, finding.key.rule, finding.key.code
        );
    }
    for key in &report.stale {
        println!(
            "STALE [{}] in {}: `{}` no longer occurs; remove it from {BASELINE}",
            key.rule, key.function, key.code
        );
    }
    for key in &report.unexplained {
        println!(
            "FAIL  [{}] in {}: `{}` is approved without a reason",
            key.rule, key.function, key.code
        );
    }
    if suggest && !report.unapproved.is_empty() {
        let suggestions = Baseline {
            approved: report
                .unapproved
                .iter()
                .map(|finding| Approval {
                    key: finding.key.clone(),
                    reason: UNWRITTEN_REASON.to_owned(),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&suggestions)?);
    }

    let failures = report.unapproved.len() + report.stale.len() + report.unexplained.len();
    if failures == 0 {
        println!(
            "ok    ui-lint: no unapproved findings ({} approved in {BASELINE})",
            report.approved
        );
        Ok(())
    } else {
        Err(XtaskError::new(format!(
            "ui-lint found {failures} problem(s)"
        )))
    }
}

fn scan(root: &Path) -> Result<Vec<Finding>> {
    let targets = PACKAGE_ROOTS
        .iter()
        .map(|path| (Category::Production, root.join(path)))
        .collect::<Vec<_>>();
    let mut findings = Vec::new();
    for (file, category) in module_categories(&targets) {
        if category != Category::Production {
            continue;
        }
        let source = RustSource::new(fs::read_to_string(&file)?);
        let relative = display_relative(root, &file);
        findings.extend(scan_source(&source).into_iter().map(|(line, key)| Finding {
            key,
            file: relative.clone(),
            line,
        }));
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok(findings)
}

/// `(1-based line, key)` of every finding outside test-only items.
fn scan_source(source: &RustSource) -> Vec<(usize, Key)> {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let tests = test_item_ranges(source);
    let functions = function_bodies(source);
    let mut findings = BTreeMap::new();
    for rule in RULES {
        for pattern in rule.patterns {
            for (offset, _) in structural.match_indices(pattern) {
                if !is_word_at(bytes, offset, pattern.len())
                    || tests.iter().any(|range| range.contains(&offset))
                {
                    continue;
                }
                let line = source.line_of(offset);
                let function = functions
                    .iter()
                    .filter(|(_, body)| body.contains(&offset))
                    .max_by_key(|(_, body)| body.start)
                    .map_or("<module>", |(name, _)| name.as_str());
                let code = source.text()[source.line_range(line)]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                // One finding per rule and line, however often it matches.
                findings.entry((line, rule.id)).or_insert(Key {
                    rule: rule.id.to_owned(),
                    function: function.to_owned(),
                    code,
                });
            }
        }
    }
    findings
        .into_iter()
        .map(|((line, _), key)| (line + 1, key))
        .collect()
}

/// Every `fn` with a body: its name and the byte range of the body.
fn function_bodies(source: &RustSource) -> Vec<(String, Range<usize>)> {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let mut functions = Vec::new();
    for (offset, _) in structural.match_indices("fn") {
        if !is_word_at(bytes, offset, 2) {
            continue;
        }
        let name_start = offset
            + 2
            + bytes[offset + 2..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
        let name_end = name_start
            + bytes[name_start..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
                .count();
        if name_end == name_start {
            continue; // `fn(..)` pointer types and `Fn` bounds.
        }
        // The body opens at the first `{` outside parentheses, angle brackets
        // and brackets; a `;` there first means a declaration without a body.
        let mut depth = 0isize;
        let mut open = None;
        for (index, byte) in bytes.iter().enumerate().skip(name_end) {
            match byte {
                b'(' | b'[' | b'<' => depth += 1,
                b')' | b']' => depth -= 1,
                b'>' if index > 0 && bytes[index - 1] != b'-' => depth -= 1,
                b'{' if depth == 0 => {
                    open = Some(index);
                    break;
                }
                b';' if depth == 0 => break,
                _ => {}
            }
        }
        if let Some(open) = open {
            if let Some(close) = source.matching_close(open) {
                functions.push((structural[name_start..name_end].to_owned(), open..close));
            }
        }
    }
    functions
}

fn check(findings: Vec<Finding>, baseline: &Baseline) -> Report {
    let mut remaining: BTreeMap<&Key, usize> = BTreeMap::new();
    let mut report = Report::default();
    for approval in &baseline.approved {
        *remaining.entry(&approval.key).or_default() += 1;
        let reason = approval.reason.trim();
        if reason.is_empty() || reason == UNWRITTEN_REASON {
            report.unexplained.push(approval.key.clone());
        }
    }
    for finding in findings {
        match remaining.get_mut(&finding.key) {
            Some(count) if *count > 0 => {
                *count -= 1;
                report.approved += 1;
            }
            _ => report.unapproved.push(finding),
        }
    }
    report.stale = remaining
        .into_iter()
        .flat_map(|(key, count)| std::iter::repeat_n(key.clone(), count))
        .collect();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<(usize, Key)> {
        scan_source(&RustSource::new(text))
    }

    fn finding(rule: &str, function: &str, code: &str, file: &str) -> Finding {
        Finding {
            key: Key {
                rule: rule.to_owned(),
                function: function.to_owned(),
                code: code.to_owned(),
            },
            file: file.to_owned(),
            line: 1,
        }
    }

    fn approval(rule: &str, function: &str, code: &str, reason: &str) -> Approval {
        Approval {
            key: Key {
                rule: rule.to_owned(),
                function: function.to_owned(),
                code: code.to_owned(),
            },
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn findings_name_their_rule_and_innermost_function() {
        let text = "\
pub(crate) use moduwu_design::Slider;
fn outer(ui: &mut Ui) {
    let inner = |ui: &mut Ui| ui.add(egui::DragValue::new(&mut x));
    fn nested<T: Fn() -> u8>(ui: &mut Ui) {
        egui::ComboBox::from_id_salt(\"a\").show_ui(ui, |_| {});
    }
}
";
        let found = keys(text);
        let summary = found
            .iter()
            .map(|(line, key)| (*line, key.rule.as_str(), key.function.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            [
                (1, "moduwu-reexport", "<module>"),
                (3, "number-field", "outer"),
                (5, "combo-box", "nested"),
            ]
        );
        assert_eq!(
            found[1].1.code,
            "let inner = |ui: &mut Ui| ui.add(egui::DragValue::new(&mut x));"
        );
    }

    #[test]
    fn comments_strings_tests_and_longer_identifiers_are_ignored() {
        let text = "\
// egui::DragValue in a comment
fn label() -> &'static str { \"ComboBox\" }
fn custom() { MyComboBoxLike::new(); }
#[cfg(test)]
mod tests {
    fn helper() { egui::DragValue::new(&mut 1); }
}
";
        assert!(keys(text).is_empty());
    }

    #[test]
    fn approvals_survive_moves_but_not_replacement() {
        let baseline = Baseline {
            approved: vec![approval(
                "combo-box",
                "f",
                "egui::ComboBox::new()",
                "Domain.",
            )],
        };
        let moved = check(
            vec![finding(
                "combo-box",
                "f",
                "egui::ComboBox::new()",
                "new/path.rs",
            )],
            &baseline,
        );
        assert!(moved.unapproved.is_empty() && moved.stale.is_empty());

        let replaced = check(
            vec![finding("combo-box", "f", "egui::ComboBox::other()", "a.rs")],
            &baseline,
        );
        assert_eq!(replaced.unapproved.len(), 1);
        assert_eq!(replaced.stale.len(), 1);
    }

    #[test]
    fn each_approval_covers_one_occurrence_and_needs_a_reason() {
        let baseline = Baseline {
            approved: vec![approval("slider", "f", "egui::Slider::new()", " ")],
        };
        let report = check(
            vec![
                finding("slider", "f", "egui::Slider::new()", "a.rs"),
                finding("slider", "f", "egui::Slider::new()", "a.rs"),
            ],
            &baseline,
        );
        assert_eq!(report.approved, 1);
        assert_eq!(report.unapproved.len(), 1);
        assert_eq!(report.unexplained.len(), 1);
    }

    #[test]
    fn the_repository_baseline_parses() {
        let text = fs::read_to_string(workspace_root().join(BASELINE)).unwrap();
        let baseline: Baseline = serde_json::from_str(&text).unwrap();
        assert!(baseline
            .approved
            .iter()
            .all(|approval| RULES.iter().any(|rule| rule.id == approval.key.rule)));
    }
}
