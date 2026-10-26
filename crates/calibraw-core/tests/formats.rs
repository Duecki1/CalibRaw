//! Format fixtures: every file in `data/formats/` is read the way CalibRaw
//! reads it, written again in the current layout, and compared with the
//! `<name>.expected.json` beside it.
//!
//! - `sidecar/*.json`: edit sidecars. `migrated` records whether CalibRaw
//!   saves the file again in the current layout.
//! - `preset/*.json`: native presets.
//! - `xmp/*.xmp`: Lightroom and Camera Raw presets, with the warnings the
//!   import shows.
//!
//! A file CalibRaw rejects expects `{"error": message}`. To add a case, add
//! the file and run this test with `CALIBRAW_PARITY_BLESS=1`, which writes
//! every expected file; review their diff before committing it.

use calibraw_core::presets::xmp::read_xmp_preset_file;
use calibraw_core::presets::Preset;
use calibraw_core::sidecar;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const KINDS: [(&str, &str); 3] = [("sidecar", "json"), ("preset", "json"), ("xmp", "xmp")];
const EXPECTED_SUFFIX: &str = ".expected.json";

#[test]
fn format_fixtures_read_as_expected() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/formats");
    let actual_root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("formats");
    let bless = std::env::var_os("CALIBRAW_PARITY_BLESS").is_some();
    let mut failures = Vec::new();
    for (kind, extension) in KINDS {
        for input in inputs(&root.join(kind), extension) {
            let actual = pretty(&read(kind, &input));
            let name = input.file_stem().unwrap().to_string_lossy().into_owned();
            let expected_path = input.with_file_name(format!("{name}{EXPECTED_SUFFIX}"));
            if bless {
                fs::write(&expected_path, &actual).unwrap();
                continue;
            }
            if fs::read_to_string(&expected_path).ok().as_deref() != Some(actual.as_str()) {
                let actual_path = actual_root.join(kind).join(format!("{name}.json"));
                fs::create_dir_all(actual_path.parent().unwrap()).unwrap();
                fs::write(&actual_path, &actual).unwrap();
                failures.push(format!(
                    "diff {} {}",
                    expected_path.display(),
                    actual_path.display()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) differ; bless with CALIBRAW_PARITY_BLESS=1 if the change is intended:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The fixtures of one kind, sorted so failures list in a stable order.
fn inputs(folder: &Path, extension: &str) -> Vec<PathBuf> {
    let mut inputs: Vec<_> = fs::read_dir(folder)
        .unwrap_or_else(|error| panic!("{}: {error}", folder.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            path.extension().is_some_and(|found| found == extension)
                && !name.ends_with(EXPECTED_SUFFIX)
        })
        .collect();
    inputs.sort();
    assert!(!inputs.is_empty(), "{} has no fixtures", folder.display());
    inputs
}

fn read(kind: &str, path: &Path) -> Value {
    let result = match kind {
        "sidecar" => read_sidecar(path),
        "preset" => read_preset(path),
        "xmp" => read_xmp(path),
        _ => unreachable!("KINDS lists every kind"),
    };
    result.unwrap_or_else(|error| json!({ "error": error }))
}

fn read_sidecar(path: &Path) -> Result<Value, String> {
    let loaded = sidecar::decode(&fs::read(path).unwrap()).map_err(|error| error.to_string())?;
    let saved = sidecar::encode_with_review_and_editing_time(
        loaded.edits,
        loaded.review,
        loaded.editing_time_ms,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({ "migrated": loaded.migrated, "saved": parse(&saved) }))
}

fn read_preset(path: &Path) -> Result<Value, String> {
    let preset = Preset::decode(&fs::read(path).unwrap()).map_err(|error| error.to_string())?;
    Ok(parse(&preset.encode().map_err(|error| error.to_string())?))
}

fn read_xmp(path: &Path) -> Result<Value, String> {
    let (import, _) = read_xmp_preset_file(path).map_err(|error| error.to_string())?;
    let saved = import.preset.encode().map_err(|error| error.to_string())?;
    Ok(json!({ "warnings": import.warnings, "saved": parse(&saved) }))
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("CalibRaw writes JSON")
}

/// Indented JSON that keeps arrays of numbers, such as curve points, on one
/// line, so expected files stay short enough to review.
fn pretty(value: &Value) -> String {
    fn numeric(value: &Value) -> bool {
        match value {
            Value::Number(_) => true,
            Value::Array(items) => items.iter().all(numeric),
            _ => false,
        }
    }
    fn write(value: &Value, indent: usize, out: &mut String) {
        let pad = "  ".repeat(indent + 1);
        let (open, close, items): (_, _, Vec<_>) = match value {
            Value::Object(fields) if !fields.is_empty() => (
                '{',
                '}',
                fields
                    .iter()
                    .map(|(key, field)| (Some(key), field))
                    .collect(),
            ),
            Value::Array(items) if !items.is_empty() && !numeric(value) => {
                ('[', ']', items.iter().map(|item| (None, item)).collect())
            }
            _ => {
                out.push_str(&value.to_string());
                return;
            }
        };
        out.push(open);
        for (index, (key, item)) in items.iter().enumerate() {
            out.push_str(if index == 0 { "\n" } else { ",\n" });
            out.push_str(&pad);
            if let Some(key) = key {
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push_str(": ");
            }
            write(item, indent + 1, out);
        }
        out.push('\n');
        out.push_str(&"  ".repeat(indent));
        out.push(close);
    }
    let mut out = String::new();
    write(value, 0, &mut out);
    out.push('\n');
    out
}
