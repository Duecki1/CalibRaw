//! `cargo xtask loc`: production, test and build line counts.
//!
//! Counting method (record it with every baseline):
//!
//! * Files are those `git ls-files --cached --others --exclude-standard`
//!   reports, so ignored build output and generated trees are excluded.
//! * Rust files are classified by walking each package's module tree from the
//!   targets `cargo metadata` reports. Library and binary targets are
//!   production; `build.rs` targets are build tooling; integration tests,
//!   benches and examples have their own categories. Within production
//!   modules, every item annotated `#[test]`, `#[cfg(test)]` or
//!   `#[cfg(all(.., test, ..))]` is test code, and so is every module file it
//!   declares.
//! * WGSL and Java sources are counted per area; Java under `src/test` is test
//!   code. Gradle, CMake, scripts, packaging and CI workflows are build
//!   tooling.
//! * A line is code when it contains anything other than whitespace and
//!   comments. Comment-only and blank lines are reported separately and are
//!   not part of the code totals.

use crate::process::{display_relative, run_command_bytes, run_command_output, workspace_root};
use crate::rust_source::{is_test_only_attribute, LineKind, RustSource};
use crate::{Result, XtaskError};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn run(args: Vec<OsString>) -> Result<()> {
    let mut json = None;
    let mut repositories = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--json" => {
                json = Some(PathBuf::from(crate::process::next_value(
                    &args, &mut index, "--json",
                )?));
            }
            "--help" | "-h" => {
                println!("Usage: cargo xtask loc [--json PATH] [REPOSITORY...]");
                return Ok(());
            }
            value if value.starts_with('-') => {
                return Err(XtaskError::usage(format!("unknown loc option: {value}")));
            }
            _ => repositories.push(PathBuf::from(&args[index])),
        }
        index += 1;
    }
    if repositories.is_empty() {
        repositories.push(workspace_root());
    }

    let mut reports = Vec::new();
    for repository in repositories {
        let repository = fs::canonicalize(&repository)
            .map_err(|error| XtaskError::new(format!("{}: {error}", repository.display())))?;
        let report = count_repository(&repository)?;
        print_report(&report);
        reports.push(report);
    }
    if let Some(path) = json {
        fs::write(&path, serde_json::to_vec_pretty(&reports)?)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Production,
    Test,
    Build,
    Bench,
    Example,
    /// Rust files no target reaches; reported so they are not silently lost.
    Unreferenced,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Lines {
    code: usize,
    comment: usize,
    blank: usize,
}

impl Lines {
    fn add(&mut self, kind: LineKind) {
        match kind {
            LineKind::Code => self.code += 1,
            LineKind::Comment => self.comment += 1,
            LineKind::Blank => self.blank += 1,
        }
    }

    fn merge(&mut self, other: Lines) {
        self.code += other.code;
        self.comment += other.comment;
        self.blank += other.blank;
    }
}

#[derive(Debug, Default, Serialize)]
struct Area {
    name: String,
    language: &'static str,
    files: usize,
    categories: BTreeMap<Category, Lines>,
}

impl Area {
    fn new(name: impl Into<String>, language: &'static str) -> Self {
        Self {
            name: name.into(),
            language,
            ..Self::default()
        }
    }

    fn lines(&mut self, category: Category) -> &mut Lines {
        self.categories.entry(category).or_default()
    }

    fn code(&self, category: Category) -> usize {
        self.categories.get(&category).map_or(0, |lines| lines.code)
    }
}

#[derive(Debug, Serialize)]
struct Report {
    repository: String,
    revision: String,
    method: &'static str,
    areas: Vec<Area>,
    unreferenced_rust_files: Vec<String>,
}

const METHOD: &str = "cargo xtask loc: git-listed files; Rust module trees from cargo metadata targets; \
#[test]/#[cfg(test)] items and modules count as tests; code = lines with non-comment, non-whitespace text";

fn count_repository(repository: &Path) -> Result<Report> {
    let files = git_files(repository)?;
    let revision = git_revision(repository);
    let packages = cargo_packages(repository)?;

    let mut areas = Vec::new();
    let mut claimed = BTreeSet::new();
    let mut unreferenced = Vec::new();
    for package in &packages {
        let (area, visited, orphans) = count_rust_package(repository, package, &files)?;
        claimed.extend(visited);
        unreferenced.extend(orphans);
        areas.push(area);
    }

    for package in &packages {
        let mut wgsl = Area::new(format!("{} (WGSL)", package.name), "wgsl");
        for file in files_with_extension(&files, &package.directory, &["wgsl"]) {
            count_c_like_file(&file, Category::Production, &mut wgsl)?;
        }
        if wgsl.files > 0 {
            areas.push(wgsl);
        }
    }

    let mut java = Area::new("Android Java", "java");
    for file in files_with_extension(&files, &repository.join("android"), &["java"]) {
        let category = if display_relative(repository, &file).contains("/src/test/") {
            Category::Test
        } else {
            Category::Production
        };
        count_c_like_file(&file, category, &mut java)?;
    }
    if java.files > 0 {
        areas.push(java);
    }

    let mut tooling = Area::new("Build tooling", "mixed");
    for file in &files {
        let relative = display_relative(repository, file);
        if claimed.contains(file) {
            continue;
        }
        match build_tooling_syntax(&relative) {
            Some(Syntax::CLike) => count_c_like_file(file, Category::Build, &mut tooling)?,
            Some(Syntax::Hash) => count_hash_comment_file(file, &mut tooling)?,
            None => {}
        }
    }
    if tooling.files > 0 {
        areas.push(tooling);
    }

    Ok(Report {
        repository: repository.display().to_string(),
        revision,
        method: METHOD,
        areas,
        unreferenced_rust_files: unreferenced,
    })
}

enum Syntax {
    CLike,
    Hash,
}

fn build_tooling_syntax(relative: &str) -> Option<Syntax> {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    if name.ends_with(".gradle") || name.ends_with(".gradle.kts") {
        return Some(Syntax::CLike);
    }
    let hash_comment = name == "CMakeLists.txt"
        || name.ends_with(".cmake")
        || name.ends_with(".sh")
        || name.ends_with(".py")
        || (relative.starts_with(".github/")
            && (name.ends_with(".yml") || name.ends_with(".yaml")))
        || (relative.starts_with("packaging/")
            && (name.ends_with(".yml") || name.ends_with(".yaml") || name.ends_with(".desktop")))
        || (name == "Cargo.toml" || name == "deny.toml" || name == "about.toml");
    hash_comment.then_some(Syntax::Hash)
}

struct Package {
    name: String,
    directory: PathBuf,
    targets: Vec<(Category, PathBuf)>,
}

fn cargo_packages(repository: &Path) -> Result<Vec<Package>> {
    let output = run_command_bytes(
        Command::new("cargo")
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .arg("--manifest-path")
            .arg(repository.join("Cargo.toml")),
        "cargo metadata",
    )?;
    let metadata: Value = serde_json::from_slice(&output)?;
    let mut packages = Vec::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let name = package["name"].as_str().unwrap_or_default().to_owned();
        let manifest = PathBuf::from(package["manifest_path"].as_str().unwrap_or_default());
        let directory = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut targets = Vec::new();
        for target in package["targets"].as_array().into_iter().flatten() {
            let kinds = target["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            let category = if kinds.contains(&"custom-build") {
                Category::Build
            } else if kinds.contains(&"test") {
                Category::Test
            } else if kinds.contains(&"bench") {
                Category::Bench
            } else if kinds.contains(&"example") {
                Category::Example
            } else {
                Category::Production
            };
            if let Some(path) = target["src_path"].as_str() {
                targets.push((category, PathBuf::from(path)));
            }
        }
        packages.push(Package {
            name,
            directory,
            targets,
        });
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(packages)
}

/// Counts one package's Rust files. Returns the area, the files it reached and
/// the package's Rust files no target reaches.
fn count_rust_package(
    repository: &Path,
    package: &Package,
    files: &[PathBuf],
) -> Result<(Area, BTreeSet<PathBuf>, Vec<String>)> {
    let mut area = Area::new(package.name.clone(), "rust");
    let categories = module_categories(&package.targets);

    for (file, category) in &categories {
        let text = fs::read_to_string(file)
            .map_err(|error| XtaskError::new(format!("{}: {error}", file.display())))?;
        let source = RustSource::new(text);
        let test_lines = if *category == Category::Production {
            line_set(&source, &test_item_ranges(&source))
        } else {
            BTreeSet::new()
        };
        for line in 0..source.line_count() {
            let line_category = if test_lines.contains(&line) {
                Category::Test
            } else {
                *category
            };
            area.lines(line_category).add(source.line_kind(line));
        }
        area.files += 1;
    }

    let visited = categories.keys().cloned().collect::<BTreeSet<_>>();
    let mut orphans = Vec::new();
    for file in files_with_extension(files, &package.directory, &["rs"]) {
        let canonical = fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        if visited.contains(&canonical) || is_in_nested_package(&file, &package.directory) {
            continue;
        }
        let text = fs::read_to_string(&file)?;
        let source = RustSource::new(text);
        for line in 0..source.line_count() {
            area.lines(Category::Unreferenced)
                .add(source.line_kind(line));
        }
        orphans.push(display_relative(repository, &file));
    }
    let visited = visited
        .into_iter()
        .chain(files_with_extension(files, &package.directory, &["rs"]))
        .collect();
    Ok((area, visited, orphans))
}

/// Every module file reachable from the target roots, with its category. A
/// module declared under a test-only attribute or item is test code.
pub(crate) fn module_categories(targets: &[(Category, PathBuf)]) -> BTreeMap<PathBuf, Category> {
    let mut categories: BTreeMap<PathBuf, Category> = BTreeMap::new();
    let mut queue = Vec::new();
    for (category, path) in targets {
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        queue.push((path, *category, true));
    }

    while let Some((file, category, is_root)) = queue.pop() {
        // A file reachable as production and as a test module stays production.
        if let Some(existing) = categories.get(&file) {
            if *existing <= category {
                continue;
            }
        }
        categories.insert(file.clone(), category);
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let source = RustSource::new(text);
        let test_ranges = test_item_ranges(&source);
        for (child, child_category) in
            child_modules(&file, is_root, &source, &test_ranges, category)
        {
            queue.push((child, child_category, false));
        }
    }
    categories
}

fn is_in_nested_package(file: &Path, package_directory: &Path) -> bool {
    let mut directory = file.parent();
    while let Some(current) = directory {
        if current == package_directory {
            return false;
        }
        if current.join("Cargo.toml").is_file() {
            return true;
        }
        directory = current.parent();
    }
    false
}

/// Byte ranges of items that are compiled only for tests.
pub(crate) fn test_item_ranges(source: &RustSource) -> Vec<Range<usize>> {
    source
        .attributed_items()
        .into_iter()
        .filter(|item| {
            item.attributes
                .iter()
                .any(|attribute| is_test_only_attribute(source.attribute_text(attribute)))
        })
        .map(|item| item.start..item.item.end)
        .collect()
}

fn line_set(source: &RustSource, ranges: &[Range<usize>]) -> BTreeSet<usize> {
    let mut lines = BTreeSet::new();
    for range in ranges {
        let first = source.line_of(range.start);
        let last = source.line_of(range.end.saturating_sub(1).max(range.start));
        lines.extend(first..=last);
    }
    lines
}

/// Resolves the `mod name;` declarations of `file` to module files.
fn child_modules(
    file: &Path,
    is_root: bool,
    source: &RustSource,
    test_ranges: &[Range<usize>],
    category: Category,
) -> Vec<(PathBuf, Category)> {
    let declarations = source.module_declarations();
    let parent = file.parent().unwrap_or(Path::new("."));
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let module_directory = if is_root || stem == "mod" || stem == "lib" || stem == "main" {
        parent.to_path_buf()
    } else {
        parent.join(stem)
    };

    let mut children = Vec::new();
    for declaration in &declarations {
        if declaration.inline_body.is_some() {
            continue;
        }
        // Inline modules that enclose this declaration extend its directory.
        let mut directory = module_directory.clone();
        for enclosing in &declarations {
            if let Some(body) = &enclosing.inline_body {
                if body.start <= declaration.start && declaration.end <= body.end {
                    directory = directory.join(&enclosing.name);
                }
            }
        }
        let path_attribute = declaration
            .attributes
            .iter()
            .find_map(|attribute| source.path_attribute(attribute));
        let candidates = match path_attribute {
            Some(path) => vec![parent.join(path)],
            None => vec![
                directory.join(format!("{}.rs", declaration.name)),
                directory.join(&declaration.name).join("mod.rs"),
            ],
        };
        let Some(child) = candidates.into_iter().find(|candidate| candidate.is_file()) else {
            continue;
        };
        let test_only = declaration
            .attributes
            .iter()
            .any(|attribute| is_test_only_attribute(source.attribute_text(attribute)))
            || test_ranges
                .iter()
                .any(|range| range.start <= declaration.start && declaration.end <= range.end);
        let child_category = if category == Category::Production && test_only {
            Category::Test
        } else {
            category
        };
        children.push((fs::canonicalize(&child).unwrap_or(child), child_category));
    }
    children
}

fn count_c_like_file(file: &Path, category: Category, area: &mut Area) -> Result<()> {
    let text = fs::read_to_string(file)
        .map_err(|error| XtaskError::new(format!("{}: {error}", file.display())))?;
    let source = RustSource::new(text);
    for line in 0..source.line_count() {
        area.lines(category).add(source.line_kind(line));
    }
    area.files += 1;
    Ok(())
}

fn count_hash_comment_file(file: &Path, area: &mut Area) -> Result<()> {
    let text = fs::read_to_string(file)
        .map_err(|error| XtaskError::new(format!("{}: {error}", file.display())))?;
    let lines = area.lines(Category::Build);
    for line in text.lines() {
        let trimmed = line.trim();
        lines.add(if trimmed.is_empty() {
            LineKind::Blank
        } else if trimmed.starts_with('#') {
            LineKind::Comment
        } else {
            LineKind::Code
        });
    }
    area.files += 1;
    Ok(())
}

fn files_with_extension(files: &[PathBuf], directory: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|file| file.starts_with(directory))
        .filter(|file| {
            file.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
        })
        .cloned()
        .collect()
}

fn git_files(repository: &Path) -> Result<Vec<PathBuf>> {
    let output = run_command_output(
        Command::new("git").arg("-C").arg(repository).args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ]),
        "git ls-files",
    )?;
    let mut files = output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| repository.join(path))
        .filter(|path| path.is_file())
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

pub(crate) fn git_revision(repository: &Path) -> String {
    let revision = run_command_output(
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["rev-parse", "--short=9", "HEAD"]),
        "git rev-parse",
    )
    .map(|revision| revision.trim().to_owned())
    .unwrap_or_else(|_| "unknown".to_owned());
    let dirty = run_command_output(
        Command::new("git").arg("-C").arg(repository).args([
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ]),
        "git status",
    )
    .is_ok_and(|status| !status.trim().is_empty());
    if dirty {
        format!("{revision}+dirty")
    } else {
        revision
    }
}

fn print_report(report: &Report) {
    println!("{} @ {}", report.repository, report.revision);
    println!(
        "{:<34} {:>10} {:>10} {:>8} {:>8} {:>9} {:>6}",
        "area", "production", "test", "build", "bench/ex", "comments", "files"
    );
    let mut totals = Area::default();
    for area in &report.areas {
        let comments = area
            .categories
            .values()
            .map(|lines| lines.comment)
            .sum::<usize>();
        println!(
            "{:<34} {:>10} {:>10} {:>8} {:>8} {:>9} {:>6}",
            area.name,
            area.code(Category::Production),
            area.code(Category::Test),
            area.code(Category::Build),
            area.code(Category::Bench) + area.code(Category::Example),
            comments,
            area.files,
        );
        for (category, lines) in &area.categories {
            totals.lines(*category).merge(*lines);
        }
        totals.files += area.files;
    }
    let comments = totals
        .categories
        .values()
        .map(|lines| lines.comment)
        .sum::<usize>();
    println!(
        "{:<34} {:>10} {:>10} {:>8} {:>8} {:>9} {:>6}",
        "total",
        totals.code(Category::Production),
        totals.code(Category::Test),
        totals.code(Category::Build),
        totals.code(Category::Bench) + totals.code(Category::Example),
        comments,
        totals.files,
    );
    if !report.unreferenced_rust_files.is_empty() {
        println!("unreferenced Rust files (not reached from any target):");
        for file in &report.unreferenced_rust_files {
            println!("  {file}");
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_test_items_and_test_modules_are_classified_as_tests() {
        let directory = tempfile::tempdir().unwrap();
        let src = directory.path().join("src");
        fs::create_dir_all(src.join("lib")).unwrap();
        fs::write(
            src.join("lib.rs"),
            "mod util;\n#[cfg(test)]\nmod tests;\n\npub fn f() {}\n\n#[cfg(test)]\nfn helper() {\n    f();\n}\n",
        )
        .unwrap();
        fs::write(src.join("util.rs"), "// docs\npub fn g() {}\n").unwrap();
        fs::write(src.join("tests.rs"), "#[test]\nfn t() {}\n").unwrap();
        let root = fs::canonicalize(src.join("lib.rs")).unwrap();
        let package = Package {
            name: "fixture".to_owned(),
            directory: fs::canonicalize(directory.path()).unwrap(),
            targets: vec![(Category::Production, root)],
        };
        let files = vec![
            fs::canonicalize(src.join("lib.rs")).unwrap(),
            fs::canonicalize(src.join("util.rs")).unwrap(),
            fs::canonicalize(src.join("tests.rs")).unwrap(),
        ];
        let (area, _, orphans) = count_rust_package(directory.path(), &package, &files).unwrap();
        assert!(orphans.is_empty());
        // lib.rs: `mod util;` and `pub fn f() {}` (the `mod tests;` group and
        // the helper are tests); util.rs: one code line.
        assert_eq!(area.code(Category::Production), 3);
        // lib.rs: attribute + `mod tests;` + 4 helper lines; tests.rs: 2 lines.
        assert_eq!(area.code(Category::Test), 8);
        assert_eq!(area.categories[&Category::Production].comment, 1);
    }
}
