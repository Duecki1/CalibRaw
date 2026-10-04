//! Screenshot baselines: `cargo xtask baseline-run` and `baseline-compare`.
//!
//! Captures go to one persistent location outside every checkout,
//! `$CALIBRAW_BASELINE_DIR`, in a new folder per run named
//! `<date>-<revision>-<label>`. A run folder is never reused, so worktrees
//! cannot overwrite each other's captures and the original reference
//! survives dependency updates.

use crate::loc::git_revision;
use crate::process::{next_value, run_checked, run_command_output, workspace_root};
use crate::{Result, XtaskError};
use serde_json::json;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REVIEW_TEST: &str = "app::ui_review_tests::gpu_ui_review";

/// `cargo xtask baseline-run LABEL [--filter FILTER]`
pub(crate) fn run(args: Vec<OsString>) -> Result<()> {
    let mut label = None;
    let mut filter = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--filter" => {
                filter = Some(
                    next_value(&args, &mut index, "--filter")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            value if value.starts_with('-') => {
                return Err(XtaskError::usage(format!(
                    "unknown baseline-run option: {value}"
                )));
            }
            value if label.is_none() => label = Some(value.to_owned()),
            _ => return Err(XtaskError::usage("baseline-run accepts one LABEL")),
        }
        index += 1;
    }
    let label = label.ok_or_else(|| XtaskError::usage("baseline-run requires a LABEL"))?;
    if label.is_empty()
        || !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        return Err(XtaskError::usage(
            "LABEL may only contain ASCII letters, digits, '-', '_' and '.'",
        ));
    }

    let root = workspace_root();
    let baseline_dir = baseline_directory(&root)?;
    let revision = git_revision(&root);
    let short = revision.trim_end_matches("+dirty");
    let run_dir = baseline_dir.join(format!("{}-{short}-{label}", utc_date()));
    // `create_dir` (not `create_dir_all`) refuses to reuse an existing run.
    fs::create_dir(&run_dir).map_err(|error| {
        XtaskError::new(format!(
            "cannot create new run folder {}: {error}",
            run_dir.display()
        ))
    })?;

    let lockfile = fs::read(root.join("Cargo.lock"))?;
    let rustc = run_command_output(Command::new("rustc").arg("-V"), "rustc -V")
        .map(|version| version.trim().to_owned())
        .unwrap_or_default();
    let mut command_line = vec![
        "cargo",
        "test",
        "--locked",
        "-p",
        "calibraw-ui",
        "--lib",
        REVIEW_TEST,
        "--",
        "--ignored",
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ];
    let manifest_path = run_dir.join("run-manifest.json");
    let mut manifest = json!({
        "label": label,
        "calibraw_revision": revision,
        "moduwu_revision": moduwu_revision(&String::from_utf8_lossy(&lockfile)),
        "cargo_lock_sha256": sha256_hex(&lockfile),
        "command": command_line.join(" "),
        "filter": filter,
        "environment": {
            "os": env::consts::OS,
            "arch": env::consts::ARCH,
            "rustc": rustc,
        },
        "started_unix_seconds": unix_seconds(),
        "harness_manifest": "manifest.json (written by the review harness: adapter, screenshots)",
    });
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    let program = command_line.remove(0);
    let mut command = Command::new(program);
    command
        .current_dir(&root)
        .args(&command_line)
        .env("CALIBRAW_UI_REVIEW_DIR", &run_dir);
    match &filter {
        Some(filter) => command.env("CALIBRAW_UI_REVIEW_FILTER", filter),
        None => command.env_remove("CALIBRAW_UI_REVIEW_FILTER"),
    };
    let result = run_checked(&mut command, "UI review capture");

    let captures = png_files(&run_dir)?;
    manifest["finished_unix_seconds"] = json!(unix_seconds());
    manifest["succeeded"] = json!(result.is_ok());
    manifest["captures"] = json!(captures
        .iter()
        .map(|path| {
            let bytes = fs::read(run_dir.join(path)).unwrap_or_default();
            json!({ "path": path, "sha256": sha256_hex(&bytes) })
        })
        .collect::<Vec<_>>());
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    result?;
    println!("{} captures in {}", captures.len(), run_dir.display());
    Ok(())
}

/// `cargo xtask baseline-compare BEFORE AFTER [--tolerance N] [--diff-dir DIR]`
pub(crate) fn compare(args: Vec<OsString>) -> Result<()> {
    let mut directories = Vec::new();
    let mut tolerance = 0u8;
    let mut diff_dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--tolerance" => {
                let value = next_value(&args, &mut index, "--tolerance")?;
                tolerance = value.to_string_lossy().parse().map_err(|_| {
                    XtaskError::usage("--tolerance expects a channel difference from 0 to 255")
                })?;
            }
            "--diff-dir" => {
                diff_dir = Some(PathBuf::from(next_value(&args, &mut index, "--diff-dir")?));
            }
            value if value.starts_with('-') => {
                return Err(XtaskError::usage(format!(
                    "unknown baseline-compare option: {value}"
                )));
            }
            _ => directories.push(PathBuf::from(&args[index])),
        }
        index += 1;
    }
    let [before, after] = directories.as_slice() else {
        return Err(XtaskError::usage(
            "baseline-compare expects BEFORE and AFTER run folders",
        ));
    };

    let before_files = png_files(before)?;
    let after_files = png_files(after)?;
    let mut differences = 0usize;
    for missing in after_files
        .iter()
        .filter(|path| !before_files.contains(path))
    {
        println!("new      {missing}");
    }
    for path in &before_files {
        if !after_files.contains(path) {
            println!("missing  {path}");
            differences += 1;
            continue;
        }
        let comparison = compare_images(&before.join(path), &after.join(path), tolerance)?;
        match comparison {
            ImageComparison::Identical => {}
            ImageComparison::SizeChanged { before, after } => {
                differences += 1;
                println!("resized  {path}: {before:?} -> {after:?}");
            }
            ImageComparison::Changed {
                pixels,
                max_difference,
                bounds,
                diff,
            } => {
                differences += 1;
                println!(
                    "changed  {path}: {pixels} pixels, max channel difference {max_difference}, bounds {bounds:?}"
                );
                if let Some(diff_dir) = &diff_dir {
                    let output = diff_dir.join(path);
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    diff.save(&output).map_err(|error| {
                        XtaskError::new(format!("cannot write {}: {error}", output.display()))
                    })?;
                }
            }
        }
    }
    println!(
        "{} of {} captures differ (tolerance {tolerance})",
        differences,
        before_files.len()
    );
    if differences == 0 {
        Ok(())
    } else {
        Err(XtaskError::silent(1))
    }
}

enum ImageComparison {
    Identical,
    SizeChanged {
        before: (u32, u32),
        after: (u32, u32),
    },
    Changed {
        pixels: usize,
        max_difference: u8,
        /// `[min_x, min_y, max_x, max_y]` of the changed pixels.
        bounds: [u32; 4],
        /// The after image dimmed, with changed pixels in red.
        diff: image::RgbaImage,
    },
}

fn compare_images(before: &Path, after: &Path, tolerance: u8) -> Result<ImageComparison> {
    let open = |path: &Path| {
        image::open(path)
            .map(|image| image.into_rgba8())
            .map_err(|error| XtaskError::new(format!("{}: {error}", path.display())))
    };
    let before = open(before)?;
    let after = open(after)?;
    if before.dimensions() != after.dimensions() {
        return Ok(ImageComparison::SizeChanged {
            before: before.dimensions(),
            after: after.dimensions(),
        });
    }
    let mut pixels = 0;
    let mut max_difference = 0u8;
    let mut bounds = [u32::MAX, u32::MAX, 0, 0];
    let mut diff = image::RgbaImage::new(after.width(), after.height());
    for ((x, y, old), new) in before.enumerate_pixels().zip(after.pixels()) {
        let difference = old
            .0
            .iter()
            .zip(new.0)
            .map(|(old, new)| old.abs_diff(new))
            .max()
            .unwrap_or(0);
        if difference > tolerance {
            pixels += 1;
            max_difference = max_difference.max(difference);
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
            diff.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
        } else {
            let dim = |channel: u8| channel / 3;
            diff.put_pixel(
                x,
                y,
                image::Rgba([dim(new[0]), dim(new[1]), dim(new[2]), 255]),
            );
        }
    }
    Ok(if pixels == 0 {
        ImageComparison::Identical
    } else {
        ImageComparison::Changed {
            pixels,
            max_difference,
            bounds,
            diff,
        }
    })
}

/// Validates `$CALIBRAW_BASELINE_DIR`: it must exist and must not live in the
/// checkout or the system temporary directory, which `git clean` or a reboot
/// would erase and which differ between worktrees.
fn baseline_directory(root: &Path) -> Result<PathBuf> {
    let configured = env::var_os("CALIBRAW_BASELINE_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            XtaskError::usage("set CALIBRAW_BASELINE_DIR to the fixed baseline location")
        })?;
    let directory = fs::canonicalize(&configured).map_err(|error| {
        XtaskError::new(format!(
            "CALIBRAW_BASELINE_DIR {} is not usable: {error}",
            PathBuf::from(&configured).display()
        ))
    })?;
    if !directory.is_dir() {
        return Err(XtaskError::new(format!(
            "CALIBRAW_BASELINE_DIR {} is not a directory",
            directory.display()
        )));
    }
    let root = fs::canonicalize(root)?;
    let temp = fs::canonicalize(env::temp_dir()).unwrap_or_else(|_| env::temp_dir());
    if directory.starts_with(&root) || directory.starts_with(&temp) {
        return Err(XtaskError::usage(format!(
            "CALIBRAW_BASELINE_DIR {} must be outside the checkout and the temporary directory",
            directory.display()
        )));
    }
    Ok(directory)
}

/// The pinned Moduwu commit recorded in `Cargo.lock`.
fn moduwu_revision(lockfile: &str) -> Option<String> {
    let mut in_moduwu = false;
    for line in lockfile.lines() {
        if line == "[[package]]" {
            in_moduwu = false;
        } else if line == "name = \"moduwu-design\"" {
            in_moduwu = true;
        } else if in_moduwu {
            if let Some(source) = line.strip_prefix("source = \"") {
                let source = source.trim_end_matches('"');
                return Some(
                    source
                        .rsplit_once('#')
                        .map_or(source, |(_, commit)| commit)
                        .to_owned(),
                );
            }
        }
    }
    None
}

fn png_files(directory: &Path) -> Result<Vec<String>> {
    let mut files = crate::process::source_files(directory, &["png"])?
        .into_iter()
        .map(|path| crate::process::display_relative(directory, &path))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Today's UTC date as `YYYY-MM-DD`.
fn utc_date() -> String {
    let (year, month, day) = civil_from_days((unix_seconds() / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Gregorian date for a day count since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_730), (2026, 10, 4));
    }

    #[test]
    fn moduwu_revision_is_read_from_the_lockfile_source() {
        let lockfile = "[[package]]\nname = \"egui\"\nsource = \"registry+x\"\n\n[[package]]\nname = \"moduwu-design\"\nversion = \"1.1.0\"\nsource = \"git+https://github.com/Duecki1/Moduwu?rev=64ac#64acb1e2\"\n";
        assert_eq!(moduwu_revision(lockfile).as_deref(), Some("64acb1e2"));
    }

    #[test]
    fn identical_and_changed_images_are_distinguished() {
        let directory = tempfile::tempdir().unwrap();
        let before = directory.path().join("before.png");
        let after = directory.path().join("after.png");
        let mut image = image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]));
        image.save(&before).unwrap();
        image.save(&after).unwrap();
        assert!(matches!(
            compare_images(&before, &after, 0).unwrap(),
            ImageComparison::Identical
        ));
        image.put_pixel(2, 1, image::Rgba([10, 25, 30, 255]));
        image.save(&after).unwrap();
        match compare_images(&before, &after, 0).unwrap() {
            ImageComparison::Changed {
                pixels,
                max_difference,
                bounds,
                ..
            } => {
                assert_eq!((pixels, max_difference, bounds), (1, 5, [2, 1, 2, 1]));
            }
            _ => panic!("expected a change"),
        }
        assert!(matches!(
            compare_images(&before, &after, 5).unwrap(),
            ImageComparison::Identical
        ));
    }
}
