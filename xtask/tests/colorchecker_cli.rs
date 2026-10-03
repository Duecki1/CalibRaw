use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const CSV_HEADER: &str =
    "patch,reference_x,reference_y,reference_z,rendered_x,rendered_y,rendered_z";

fn command(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .current_dir(directory)
        .arg("colorchecker-wb-validate");
    command
}

fn assert_failed(output: &Output) {
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !output.stderr.is_empty(),
        "failed without an error diagnostic: {output:?}"
    );
}

#[test]
fn colorchecker_writes_the_report_schema_and_prints_summary_and_worst_patches() {
    let directory = tempfile::tempdir().unwrap();
    let csv_path = directory.path().join("patch samples.csv");
    let report_path = directory.path().join("report.json");
    fs::write(
        &csv_path,
        format!(
            "{CSV_HEADER},illuminant,stage,neutral\n\
             white,0.96422,1,0.82521,0.96422,1,0.82521,D50,final,yes\n\
             shifted,0.96422,1,0.82521,0,0,0,D50,final,no\n\
             black,0,0,0,0,0,0,D65,before,false\n"
        ),
    )
    .unwrap();

    let output = command(directory.path())
        .arg(&csv_path)
        .arg("--json")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report.as_object().unwrap().len(), 2);
    assert_eq!(
        report["summary"],
        json!({
            "D50 / final": {
                "patch_count": 2,
                "neutral_patch_count": 1,
                "mean_delta_e_2000": 50.0,
                "max_delta_e_2000": 100.0,
                "mean_neutral_lab_chroma": 0.0,
                "max_neutral_lab_chroma": 0.0,
            },
            "D65 / before": {
                "patch_count": 1,
                "neutral_patch_count": 0,
                "mean_delta_e_2000": 0.0,
                "max_delta_e_2000": 0.0,
                "mean_neutral_lab_chroma": null,
                "max_neutral_lab_chroma": null,
            },
        })
    );
    assert_eq!(
        report["patches"],
        json!([
            {
                "patch": "white",
                "illuminant": "D50",
                "stage": "final",
                "neutral": true,
                "reference_lab": [100.0, 0.0, 0.0],
                "rendered_lab": [100.0, 0.0, 0.0],
                "delta_e_2000": 0.0,
                "neutral_chroma": 0.0,
            },
            {
                "patch": "shifted",
                "illuminant": "D50",
                "stage": "final",
                "neutral": false,
                "reference_lab": [100.0, 0.0, 0.0],
                "rendered_lab": [0.0, 0.0, 0.0],
                "delta_e_2000": 100.0,
                "neutral_chroma": 0.0,
            },
            {
                "patch": "black",
                "illuminant": "D65",
                "stage": "before",
                "neutral": false,
                "reference_lab": [0.0, 0.0, 0.0],
                "rendered_lab": [0.0, 0.0, 0.0],
                "delta_e_2000": 0.0,
                "neutral_chroma": 0.0,
            },
        ])
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let (summary, worst) = stdout.split_once("Worst patches:").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(summary).unwrap(),
        report["summary"]
    );
    let lines: Vec<_> = worst
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0].trim(),
        "D50 / final / shifted: dE00=100.0000, C*ab=0.0000"
    );
    assert!(worst.contains("D50 / final / white: dE00=0.0000, C*ab=0.0000"));
    assert!(worst.contains("D65 / before / black: dE00=0.0000, C*ab=0.0000"));
}

#[test]
fn colorchecker_prints_at_most_eight_worst_patches_without_a_json_option() {
    let directory = tempfile::tempdir().unwrap();
    let csv_path = directory.path().join("patches.csv");
    let mut csv = format!("{CSV_HEADER}\n");
    for index in 0..10 {
        csv.push_str(&format!("patch-{index},0,0,0,0,0,0\n"));
    }
    fs::write(&csv_path, csv).unwrap();
    let output = command(directory.path()).arg(&csv_path).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let (summary, worst) = stdout.split_once("Worst patches:").unwrap();
    let summary: Value = serde_json::from_str(summary).unwrap();
    assert_eq!(summary["unspecified / final"]["patch_count"], 10);
    assert_eq!(
        worst.lines().filter(|line| line.contains("dE00=")).count(),
        8
    );
}

#[test]
fn colorchecker_help_succeeds_without_a_csv_path() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(directory.path()).arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("colorchecker-wb-validate"));
    assert!(stdout.contains("--json"));
}

#[test]
fn colorchecker_invalid_inputs_fail_without_writing_a_report() {
    let directory = tempfile::tempdir().unwrap();
    let csv_path = directory.path().join("invalid.csv");
    let report_path = directory.path().join("report.json");
    let invalid_inputs = [
        String::new(),
        format!("{CSV_HEADER}\n"),
        "patch,reference_x\nblack,0\n".to_owned(),
        format!("{CSV_HEADER}\nblack,0,0,0,0,0\n"),
        format!("{CSV_HEADER}\nblack,0,0,0,not-a-number,0,0\n"),
        format!("{CSV_HEADER}\nblack,0,0,0,NaN,0,0\n"),
    ];
    for csv in invalid_inputs {
        fs::write(&csv_path, csv).unwrap();
        let output = command(directory.path())
            .arg(&csv_path)
            .arg("--json")
            .arg(&report_path)
            .output()
            .unwrap();
        assert_failed(&output);
        assert!(!report_path.exists());
    }

    let output = command(directory.path())
        .arg(directory.path().join("missing.csv"))
        .output()
        .unwrap();
    assert_failed(&output);
}

#[test]
fn colorchecker_invalid_arguments_and_the_removed_self_check_fail() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("patches.csv"),
        format!("{CSV_HEADER}\nblack,0,0,0,0,0,0\n"),
    )
    .unwrap();
    let cases: &[&[&str]] = &[
        &[],
        &["patches.csv", "--json"],
        &["patches.csv", "--unknown"],
        &["patches.csv", "extra.csv"],
        &["--self-check"],
        &["patches.csv", "--self-check"],
    ];
    for args in cases {
        let output = command(directory.path()).args(*args).output().unwrap();
        assert_failed(&output);
    }
}

#[test]
fn colorchecker_accepts_json_equals_and_a_dash_prefixed_input_path() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("-patches.csv"),
        format!("{CSV_HEADER}\nblack,0,0,0,0,0,0\n"),
    )
    .unwrap();
    let output = command(directory.path())
        .args(["--json=report file.json", "--", "-patches.csv"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value =
        serde_json::from_slice(&fs::read(directory.path().join("report file.json")).unwrap())
            .unwrap();
    assert_eq!(report["patches"][0]["patch"], "black");
}
