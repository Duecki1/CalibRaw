//! Compare reference and rendered D50 XYZ ColorChecker patches with CIEDE2000.

use crate::{Result, XtaskError};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;

const D50: [f64; 3] = [0.96422, 1.0, 0.82521];
const REQUIRED_COLUMNS: [&str; 7] = [
    "patch",
    "reference_x",
    "reference_y",
    "reference_z",
    "rendered_x",
    "rendered_y",
    "rendered_z",
];

pub(super) fn run(args: Vec<OsString>) -> Result<()> {
    let mut input = None;
    let mut json = None;
    let mut positional_only = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let value = argument.to_string_lossy();
        match value.as_ref() {
            "--help" | "-h" if !positional_only => {
                println!(
                    "Usage: cargo xtask colorchecker-wb-validate CSV [--json PATH]\n\n\
                     Compare reference and rendered D50 XYZ ColorChecker patches.\n\
                     Required CSV columns: {}\n\
                     Optional columns: illuminant,stage,neutral\n\
                     Both sides must use the same XYZ scale (Y=1 or Y=100).\n\
                     --json PATH also writes the summary and per-patch results.\n\n\
                     Run checks: cargo test --locked -p xtask colorchecker",
                    REQUIRED_COLUMNS.join(",")
                );
                return Ok(());
            }
            "--json" if !positional_only => {
                json =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        XtaskError::usage("--json requires a path")
                    })?));
            }
            value if !positional_only && value.starts_with("--json=") => {
                json = Some(PathBuf::from(&value["--json=".len()..]));
            }
            "--" if !positional_only => positional_only = true,
            value if !positional_only && value.starts_with('-') => {
                return Err(XtaskError::usage(format!(
                    "unknown colorchecker-wb-validate option: {value}"
                )));
            }
            _ if input.is_none() => input = Some(PathBuf::from(argument)),
            _ => {
                return Err(XtaskError::usage(
                    "colorchecker-wb-validate accepts exactly one CSV path",
                ));
            }
        }
    }
    let input = input.ok_or_else(|| XtaskError::usage("CSV path is required"))?;
    let file = File::open(&input)
        .map_err(|error| XtaskError::new(format!("{}: {error}", input.display())))?;
    let patches = load_rows(file)?;
    let summary = aggregate(&patches);
    println!("{}", serde_json::to_string_pretty(&summary)?);

    let mut worst: Vec<_> = patches.iter().collect();
    worst.sort_by(|left, right| right.delta_e_2000.total_cmp(&left.delta_e_2000));
    println!("\nWorst patches:");
    for patch in worst.into_iter().take(8) {
        println!(
            "  {} / {} / {}: dE00={:.4}, C*ab={:.4}",
            patch.illuminant, patch.stage, patch.patch, patch.delta_e_2000, patch.neutral_chroma
        );
    }
    if let Some(path) = json {
        let report = Report { summary, patches };
        fs::write(&path, serde_json::to_vec_pretty(&report)?)
            .map_err(|error| XtaskError::new(format!("{}: {error}", path.display())))?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct Patch {
    patch: String,
    illuminant: String,
    stage: String,
    neutral: bool,
    reference_lab: [f64; 3],
    rendered_lab: [f64; 3],
    delta_e_2000: f64,
    neutral_chroma: f64,
}

#[derive(Debug, Serialize)]
struct Summary {
    patch_count: usize,
    mean_delta_e_2000: f64,
    max_delta_e_2000: f64,
    neutral_patch_count: usize,
    mean_neutral_lab_chroma: Option<f64>,
    max_neutral_lab_chroma: Option<f64>,
}

#[derive(Serialize)]
struct Report {
    summary: BTreeMap<String, Summary>,
    patches: Vec<Patch>,
}

fn load_rows(reader: impl Read) -> Result<Vec<Patch>> {
    let mut reader = csv::Reader::from_reader(reader);
    let headers = reader.headers().map_err(csv_error)?.clone();
    let column = |name: &str| {
        let mut found = None;
        for (index, header) in headers.iter().enumerate() {
            if header == name {
                found = Some(index);
            }
        }
        found
    };
    let missing: Vec<_> = REQUIRED_COLUMNS
        .iter()
        .copied()
        .filter(|name| column(name).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(XtaskError::new(format!(
            "missing CSV columns: {}",
            missing.join(", ")
        )));
    }

    let mut patches = Vec::new();
    for record in reader.records() {
        let record = record.map_err(csv_error)?;
        let field = |name: &str| column(name).and_then(|index| record.get(index));
        let line = record.position().map_or(0, csv::Position::line);
        let number = |name: &str| -> Result<f64> {
            field(name)
                .unwrap_or_default()
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    XtaskError::new(format!("CSV line {line}: {name} must be a finite number"))
                })
        };
        let reference_lab = xyz_to_lab([
            number("reference_x")?,
            number("reference_y")?,
            number("reference_z")?,
        ]);
        let rendered_lab = xyz_to_lab([
            number("rendered_x")?,
            number("rendered_y")?,
            number("rendered_z")?,
        ]);
        let delta_e_2000 = delta_e_2000(reference_lab, rendered_lab);
        let neutral_chroma = rendered_lab[1].hypot(rendered_lab[2]);
        if !reference_lab
            .iter()
            .chain(&rendered_lab)
            .chain([&delta_e_2000, &neutral_chroma])
            .all(|value| value.is_finite())
        {
            return Err(XtaskError::new(format!(
                "CSV line {line}: XYZ values exceed the supported numeric range"
            )));
        }
        patches.push(Patch {
            patch: field("patch").unwrap_or_default().to_owned(),
            illuminant: field("illuminant")
                .filter(|value| !value.is_empty())
                .unwrap_or("unspecified")
                .to_owned(),
            stage: field("stage")
                .filter(|value| !value.is_empty())
                .unwrap_or("final")
                .to_owned(),
            neutral: matches!(
                field("neutral")
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "1" | "true" | "yes" | "y" | "neutral"
            ),
            reference_lab,
            rendered_lab,
            delta_e_2000,
            neutral_chroma,
        });
    }
    if patches.is_empty() {
        return Err(XtaskError::new("CSV contains no patches"));
    }
    Ok(patches)
}

fn csv_error(error: csv::Error) -> XtaskError {
    XtaskError::new(format!("invalid CSV: {error}"))
}

fn aggregate(rows: &[Patch]) -> BTreeMap<String, Summary> {
    let mut groups: BTreeMap<_, Vec<&Patch>> = BTreeMap::new();
    for row in rows {
        groups
            .entry((&row.illuminant, &row.stage))
            .or_default()
            .push(row);
    }
    groups
        .into_iter()
        .map(|((illuminant, stage), group)| {
            let neutrals: Vec<_> = group.iter().filter(|row| row.neutral).collect();
            let summary = Summary {
                patch_count: group.len(),
                mean_delta_e_2000: group.iter().map(|row| row.delta_e_2000).sum::<f64>()
                    / group.len() as f64,
                max_delta_e_2000: group.iter().map(|row| row.delta_e_2000).fold(0.0, f64::max),
                neutral_patch_count: neutrals.len(),
                mean_neutral_lab_chroma: (!neutrals.is_empty()).then(|| {
                    neutrals.iter().map(|row| row.neutral_chroma).sum::<f64>()
                        / neutrals.len() as f64
                }),
                max_neutral_lab_chroma: neutrals
                    .iter()
                    .map(|row| row.neutral_chroma)
                    .reduce(f64::max),
            };
            (format!("{illuminant} / {stage}"), summary)
        })
        .collect()
}

fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let scale = if xyz.iter().any(|value| value.abs() > 2.0) {
        100.0
    } else {
        1.0
    };
    let delta: f64 = 6.0 / 29.0;
    let transform = |value: f64| {
        if value > delta.powi(3) {
            value.powf(1.0 / 3.0)
        } else {
            value / (3.0 * delta * delta) + 4.0 / 29.0
        }
    };
    let [fx, fy, fz] = std::array::from_fn(|index| transform(xyz[index] / scale / D50[index]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn delta_e_2000([l1, a1, b1]: [f64; 3], [l2, a2, b2]: [f64; 3]) -> f64 {
    // Sharma, Wu & Dalal (2005), kL=kC=kH=1.
    let c_bar = 0.5 * (a1.hypot(b1) + a2.hypot(b2));
    let c_bar7 = c_bar.powi(7);
    let g = if c_bar == 0.0 {
        0.0
    } else {
        0.5 * (1.0 - (c_bar7 / (c_bar7 + 25.0_f64.powi(7))).sqrt())
    };
    let ap1 = (1.0 + g) * a1;
    let ap2 = (1.0 + g) * a2;
    let cp1 = ap1.hypot(b1);
    let cp2 = ap2.hypot(b2);
    let hue = |a: f64, b: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let hp1 = hue(ap1, b1);
    let hp2 = hue(ap2, b2);
    let dh = hp2 - hp1;
    let dhp = if cp1 * cp2 == 0.0 {
        0.0
    } else if dh.abs() <= 180.0 {
        dh
    } else if dh > 180.0 {
        dh - 360.0
    } else {
        dh + 360.0
    };
    let dhp = 2.0 * (cp1 * cp2).sqrt() * (dhp / 2.0).to_radians().sin();
    let l_bar = 0.5 * (l1 + l2);
    let cp_bar = 0.5 * (cp1 + cp2);
    let h_sum = hp1 + hp2;
    let hp_bar = if cp1 * cp2 == 0.0 {
        h_sum
    } else if (hp1 - hp2).abs() <= 180.0 {
        0.5 * h_sum
    } else if h_sum < 360.0 {
        0.5 * (h_sum + 360.0)
    } else {
        0.5 * (h_sum - 360.0)
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let cp_bar7 = cp_bar.powi(7);
    let rc = 2.0 * (cp_bar7 / (cp_bar7 + 25.0_f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    let x = (l2 - l1) / sl;
    let y = (cp2 - cp1) / sc;
    let z = dhp / sh;
    (x * x + y * y + z * z + rt * y * z).sqrt()
}

#[cfg(test)]
mod tests;
