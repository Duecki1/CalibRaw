use super::*;

const CSV_HEADER: &str =
    "patch,reference_x,reference_y,reference_z,rendered_x,rendered_y,rendered_z";

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual} (tolerance {tolerance})"
    );
}

fn assert_lab_close(actual: [f64; 3], expected: [f64; 3]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert_close(actual, expected, 1e-9);
    }
}

#[test]
fn delta_e_matches_the_sharma_reference_pair() {
    // Sharma, Wu & Dalal (2005), pair 1: the former Python --self-check.
    let measured = delta_e_2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
    assert_close(measured, 2.0425, 5e-5);
}

#[test]
fn xyz_to_lab_handles_d50_white_black_and_near_black() {
    assert_lab_close(xyz_to_lab(D50), [100.0, 0.0, 0.0]);
    assert_lab_close(xyz_to_lab([0.0; 3]), [0.0; 3]);
    // All components exercise the linear branch of the Lab transfer curve.
    assert_lab_close(
        xyz_to_lab([0.0005, 0.001, 0.0015]),
        [0.9032962962962969, -1.8745194734258452, -1.2735218240744084],
    );
}

#[test]
fn xyz_to_lab_agrees_for_y_one_and_y_hundred_scales() {
    // The Y=100 values must exceed 2 in at least one component to be detected.
    for xyz in [D50, [0.3, 0.2, 0.1], [0.02, 0.03, 0.01]] {
        assert_lab_close(xyz_to_lab(xyz), xyz_to_lab(xyz.map(|value| value * 100.0)));
    }
}

#[test]
fn xyz_to_lab_preserves_the_absolute_component_scale_heuristic() {
    // Exactly 2 stays on the Y=1 scale; a negative component can trigger Y=100.
    assert_lab_close(
        xyz_to_lab([1.92844, 2.0, 1.65042]),
        [130.1508417878053, 0.0, 0.0],
    );
    assert_lab_close(
        xyz_to_lab([-3.0, 1.0, 0.0]),
        [8.991442404369852, -159.89615996577538, 15.502486904085956],
    );
}

#[test]
fn delta_e_is_zero_for_identical_colors_and_symmetric_for_edge_pairs() {
    for lab in [[0.0; 3], [100.0, 0.0, 0.0], [50.0, 40.0, -1.0]] {
        assert_close(delta_e_2000(lab, lab), 0.0, 1e-12);
    }

    // Golden values from the Python implementation, including a 360/0 hue wrap.
    for (left, right, expected) in [
        ([50.0, 40.0, -1.0], [50.0, 40.0, 1.0], 1.111393781857888),
        ([50.0, -40.0, -1.0], [50.0, -40.0, 1.0], 1.2559691214476374),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3668588191717523),
        ([0.0; 3], [100.0, 0.0, 0.0], 100.0),
    ] {
        assert_close(delta_e_2000(left, right), expected, 1e-9);
        assert_close(delta_e_2000(right, left), expected, 1e-9);
    }
}

#[test]
fn load_rows_reads_quoted_fields_and_computes_patch_metrics() {
    let csv = concat!(
        "stage,patch,reference_x,reference_y,reference_z,rendered_x,rendered_y,rendered_z,illuminant,neutral\r\n",
        "\"after, WB\",\"gray, \"\"card\"\"\npatch\",0.3,0.2,0.1,0.31,0.19,0.11,\"D50, booth\",yes\r\n",
    );
    let rows = load_rows(csv.as_bytes()).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.patch, "gray, \"card\"\npatch");
    assert_eq!(row.illuminant, "D50, booth");
    assert_eq!(row.stage, "after, WB");
    assert!(row.neutral);
    assert_lab_close(
        row.reference_lab,
        [51.837211526538496, 46.404708303280806, 17.989585734003168],
    );
    assert_lab_close(
        row.rendered_lab,
        [50.68720611576005, 55.085074473477434, 12.812018481451904],
    );
    assert_close(row.delta_e_2000, 4.751633048157102, 1e-9);
    assert_close(row.neutral_chroma, 56.55539980689395, 1e-9);
}

#[test]
fn load_rows_uses_the_last_duplicate_column() {
    let csv = format!("{CSV_HEADER},rendered_x\nwhite,0.96422,1,0.82521,0,1,0.82521,0.96422\n");
    let rows = load_rows(csv.as_bytes()).unwrap();
    assert_lab_close(rows[0].rendered_lab, [100.0, 0.0, 0.0]);
    assert_close(rows[0].delta_e_2000, 0.0, 1e-12);
}

#[test]
fn load_rows_defaults_missing_and_empty_optional_columns() {
    for csv in [
        format!("{CSV_HEADER}\nblack,0,0,0,0,0,0\n"),
        format!("{CSV_HEADER},illuminant,stage,neutral\nblack,0,0,0,0,0,0,,,\n"),
    ] {
        let rows = load_rows(csv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.patch, "black");
        assert_eq!(row.illuminant, "unspecified");
        assert_eq!(row.stage, "final");
        assert!(!row.neutral);
        assert_lab_close(row.reference_lab, [0.0; 3]);
        assert_lab_close(row.rendered_lab, [0.0; 3]);
        assert_close(row.delta_e_2000, 0.0, 1e-12);
        assert_close(row.neutral_chroma, 0.0, 1e-12);
    }
}

#[test]
fn load_rows_preserves_python_neutral_flag_spellings() {
    for (flag, expected) in [
        ("1", true),
        (" TrUe ", true),
        ("yes", true),
        ("Y", true),
        ("neutral", true),
        ("", false),
        ("0", false),
        ("false", false),
        ("no", false),
        ("other", false),
    ] {
        let csv = format!("{CSV_HEADER},neutral\nblack,0,0,0,0,0,0,{flag}\n");
        let rows = load_rows(csv.as_bytes()).unwrap();
        assert_eq!(rows[0].neutral, expected, "neutral flag {flag:?}");
    }
}

#[test]
fn aggregate_groups_by_illuminant_and_stage_and_only_counts_neutral_chroma() {
    let patch = |illuminant: &str, stage: &str, neutral, error, chroma| Patch {
        patch: "fixture".to_owned(),
        illuminant: illuminant.to_owned(),
        stage: stage.to_owned(),
        neutral,
        reference_lab: [0.0; 3],
        rendered_lab: [0.0; 3],
        delta_e_2000: error,
        neutral_chroma: chroma,
    };
    let rows = [
        patch("D65", "final", false, 9.0, 100.0),
        patch("D50", "final", true, 2.0, 3.0),
        patch("D50", "before", false, 7.0, 50.0),
        patch("D50", "final", false, 8.0, 90.0),
        patch("D50", "final", true, 5.0, 9.0),
    ];
    let groups = aggregate(&rows);
    assert_eq!(
        groups.keys().map(String::as_str).collect::<Vec<_>>(),
        ["D50 / before", "D50 / final", "D65 / final"]
    );
    let summary = &groups["D50 / final"];
    assert_eq!(summary.patch_count, 3);
    assert_eq!(summary.neutral_patch_count, 2);
    assert_close(summary.mean_delta_e_2000, 5.0, 1e-12);
    assert_close(summary.max_delta_e_2000, 8.0, 1e-12);
    assert_eq!(summary.mean_neutral_lab_chroma, Some(6.0));
    assert_eq!(summary.max_neutral_lab_chroma, Some(9.0));

    for (key, error) in [("D50 / before", 7.0), ("D65 / final", 9.0)] {
        let summary = &groups[key];
        assert_eq!(summary.patch_count, 1);
        assert_eq!(summary.neutral_patch_count, 0);
        assert_close(summary.mean_delta_e_2000, error, 1e-12);
        assert_close(summary.max_delta_e_2000, error, 1e-12);
        assert_eq!(summary.mean_neutral_lab_chroma, None);
        assert_eq!(summary.max_neutral_lab_chroma, None);
    }
    assert!(aggregate(&[]).is_empty());
}

#[test]
fn load_rows_rejects_empty_input_and_missing_required_columns() {
    assert!(load_rows("".as_bytes()).is_err());
    let columns: Vec<_> = CSV_HEADER.split(',').collect();
    for missing in 0..columns.len() {
        let header = columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| (index != missing).then_some(*column))
            .collect::<Vec<_>>()
            .join(",");
        let cells = ["black", "0", "0", "0", "0", "0", "0"]
            .into_iter()
            .enumerate()
            .filter_map(|(index, cell)| (index != missing).then_some(cell))
            .collect::<Vec<_>>()
            .join(",");
        let csv = format!("{header}\n{cells}\n");
        assert!(
            load_rows(csv.as_bytes()).is_err(),
            "accepted missing column {}",
            columns[missing]
        );
    }
}

#[test]
fn load_rows_rejects_malformed_records_and_numeric_values() {
    for record in [
        "patch,0,0,0,0,0",            // Missing field.
        "patch,0,0,0,0,0,0,0",        // Extra field.
        "patch,0,0,0,0,,0",           // Empty numeric value.
        "patch,0,0,nope,0,0,0",       // Invalid reference component.
        "patch,0,0,0,nope,0,0",       // Invalid rendered component.
        "\"unterminated,0,0,0,0,0,0", // Unterminated quoted record.
    ] {
        let csv = format!("{CSV_HEADER}\n{record}\n");
        assert!(
            load_rows(csv.as_bytes()).is_err(),
            "accepted malformed record {record:?}"
        );
    }
}

#[test]
fn load_rows_rejects_nonfinite_values_in_every_xyz_column() {
    for component in 1..=6 {
        for value in ["NaN", "inf", "-inf", "1e999"] {
            let mut cells = ["patch", "0", "0", "0", "0", "0", "0"];
            cells[component] = value;
            let csv = format!("{CSV_HEADER}\n{}\n", cells.join(","));
            assert!(
                load_rows(csv.as_bytes()).is_err(),
                "accepted {value} in XYZ column {component}"
            );
        }
    }
}
