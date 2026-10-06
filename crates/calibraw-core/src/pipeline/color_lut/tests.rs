use super::*;

fn identity(edge: u32) -> ColorLut {
    ColorLut::from_function(edge, |rgb| rgb).unwrap()
}

fn cube_text(edge: u32, map: impl Fn([f32; 3]) -> [f32; 3]) -> String {
    let last = (edge - 1) as f32;
    let mut text = format!("TITLE \"Test look\"\n# comment\nLUT_3D_SIZE {edge}\n\n");
    for blue in 0..edge {
        for green in 0..edge {
            for red in 0..edge {
                let [r, g, b] = map([red as f32 / last, green as f32 / last, blue as f32 / last]);
                text.push_str(&format!("{r} {g} {b}\n"));
            }
        }
    }
    text
}

fn close(left: [f32; 3], right: [f32; 3]) -> bool {
    left.iter().zip(right).all(|(a, b)| (a - b).abs() < 2e-4)
}

#[test]
fn identity_tables_return_their_input() {
    let lut = identity(17);
    for rgb in [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.2, 0.55, 0.9],
        [0.013, 0.5, 0.77],
    ] {
        assert!(
            close(lut.sample(rgb), rgb),
            "{rgb:?} -> {:?}",
            lut.sample(rgb)
        );
    }
}

#[test]
fn sampling_interpolates_between_points_and_clamps_the_input() {
    // Inverts red only, so the midpoint of a cell is half way between.
    let lut = ColorLut::from_function(2, |[r, g, b]| [1.0 - r, g, b]).unwrap();
    assert!(close(lut.sample([0.25, 0.5, 0.5]), [0.75, 0.5, 0.5]));
    assert!(close(lut.sample([-4.0, 0.5, 0.5]), [1.0, 0.5, 0.5]));
    assert!(close(lut.sample([9.0, 0.5, 0.5]), [0.0, 0.5, 0.5]));
}

#[test]
fn parses_a_cube_file_with_red_varying_fastest() {
    let text = cube_text(3, |[r, g, b]| [b, r, g]);
    let cube = CubeFile::parse(&text).unwrap();
    assert_eq!(cube.title.as_deref(), Some("Test look"));
    assert_eq!(cube.lut.edge(), 3);
    // Input (red) maps to the output's green channel in this table.
    assert!(close(cube.lut.sample([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    assert!(close(cube.lut.sample([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
    assert!(close(cube.lut.sample([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
}

#[test]
fn cube_domains_scale_the_input() {
    let mut text = String::from("LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n");
    for blue in 0..2 {
        for green in 0..2 {
            for red in 0..2 {
                text.push_str(&format!("{red} {green} {blue}\n"));
            }
        }
    }
    let lut = CubeFile::parse(&text).unwrap().lut;
    // Inputs 0..2 span the table, so an input of 1 is half way.
    assert!(close(lut.sample([1.0, 1.0, 1.0]), [0.5, 0.5, 0.5]));
    assert!(close(lut.sample([2.0, 0.0, 0.0]), [1.0, 0.0, 0.0]));
    let ranged = CubeFile::parse(&text.replace(
        "DOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2",
        "LUT_3D_INPUT_RANGE 0 2",
    ))
    .unwrap()
    .lut;
    assert_eq!(ranged, lut);
}

#[test]
fn ignores_unknown_keywords_and_trailing_comments() {
    let text = "LUT_IN_VIDEO_RANGE\nLUT_3D_SIZE 2 # size\n0 0 0\n1 0 0 # red\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
    assert!(close(
        CubeFile::parse(text).unwrap().lut.sample([1.0, 0.0, 1.0]),
        [1.0, 0.0, 1.0]
    ));
}

#[test]
fn rejects_unsupported_and_malformed_cube_files() {
    let two_by_two = |body: &str| format!("LUT_3D_SIZE 2\n{body}");
    let eight = "0 0 0\n".repeat(8);
    for (text, expected) in [
        ("LUT_1D_SIZE 16\n0 0 0\n".to_owned(), "1D"),
        ("0 0 0\n".to_owned(), "before LUT_3D_SIZE"),
        ("# nothing\n".to_owned(), "LUT_3D_SIZE is missing"),
        ("LUT_3D_SIZE 66\n".to_owned(), "more than 65"),
        ("LUT_3D_SIZE 1\n".to_owned(), "too small"),
        ("LUT_3D_SIZE two\n".to_owned(), "invalid LUT_3D_SIZE"),
        (two_by_two("0 0 0\n"), "needs 8 colours"),
        (two_by_two(&format!("{eight}0 0 0\n")), "more data"),
        (
            two_by_two(&eight.replacen("0 0 0", "0 0", 1)),
            "expected three values",
        ),
        (
            two_by_two(&eight.replacen("0 0 0", "0 x 0", 1)),
            "not a number",
        ),
        (
            two_by_two(&eight.replacen("0 0 0", "0 NaN 0", 1)),
            "not a finite number",
        ),
        (
            "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0\n".to_owned(),
            "invalid DOMAIN_MIN",
        ),
        (
            "LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 1 1 1\n".to_owned() + &eight,
            "input range",
        ),
    ] {
        let message = CubeFile::parse(&text).unwrap_err().to_string();
        assert!(message.contains(expected), "{text:?}: {message}");
    }
}

#[test]
fn outputs_outside_the_display_range_are_clamped() {
    let mut text = String::from("LUT_3D_SIZE 2\n");
    text.push_str("-0.5 0 0\n1.5 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n");
    let lut = CubeFile::parse(&text).unwrap().lut;
    assert_eq!(lut.sample([0.0, 0.0, 0.0])[0], 0.0);
    assert_eq!(lut.sample([1.0, 0.0, 0.0])[0], 1.0);
}

#[test]
fn gpu_texels_upsample_exactly_and_end_with_opaque_alpha() {
    // 17 -> 65 lands every old point on the new grid, so nothing is lost.
    let lut = ColorLut::from_function(17, |[r, g, b]| [g, b * b, 1.0 - r]).unwrap();
    let texels = lut.gpu_texels(65);
    assert_eq!(texels.len(), 65 * 65 * 65 * 4);
    let at = |red: usize, green: usize, blue: usize| {
        let base = ((blue * 65 + green) * 65 + red) * 4;
        [0, 1, 2].map(|channel| half::f16::from_bits(texels[base + channel]).to_f32())
    };
    for (red, green, blue) in [(0, 0, 0), (64, 64, 64), (16, 32, 48), (4, 8, 12)] {
        let input = [red, green, blue].map(|index| index as f32 / 64.0);
        let expected = lut.sample(input);
        let found = at(red, green, blue);
        assert!(
            expected
                .iter()
                .zip(found)
                .all(|(a, b)| (a - b).abs() < 1e-3),
            "{expected:?} vs {found:?}"
        );
    }
    assert!(texels
        .chunks_exact(4)
        .all(|texel| texel[3] == half::f16::ONE.to_bits()));
}

#[test]
fn round_trips_through_json_and_keeps_sharing_cheap() {
    let edit = ColorLutEdit::new("  Warm\u{7} film ", identity(5));
    assert_eq!(edit.name, "Warm film");
    let json = serde_json::to_string(&edit).unwrap();
    assert!(
        !json.contains("domain"),
        "default domains stay out of the file"
    );
    let restored: ColorLutEdit = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, edit);
    assert_eq!(restored.amount, 100.0);

    let mut ranged = ColorLut::new(2, [0.0; 3], [2.0; 3], vec![0; 24]).unwrap();
    ranged.domain_max = [2.0, 2.0, 4.0];
    let restored: ColorLut =
        serde_json::from_str(&serde_json::to_string(&ranged).unwrap()).unwrap();
    assert_eq!(restored, ranged);
}

#[test]
fn damaged_serialized_tables_are_rejected() {
    let good = serde_json::to_value(identity(2)).unwrap();
    let with = |key: &str, value: serde_json::Value| {
        let mut document = good.clone();
        document[key] = value;
        serde_json::from_value::<ColorLut>(document)
    };
    assert!(with("edge", 3.into()).is_err(), "wrong size for the data");
    assert!(with("edge", 99.into()).is_err(), "edge out of range");
    assert!(with("samples", "AAA=".into()).is_err(), "odd byte count");
    assert!(with("samples", "!!".into()).is_err(), "not base64");
    assert!(
        with("domain_max", serde_json::json!([0.0, 1.0, 1.0])).is_err(),
        "empty range"
    );
}

#[test]
fn validates_names_and_amounts() {
    let mut edit = ColorLutEdit::new("Look", identity(2));
    assert!(edit.validate().is_ok());
    edit.amount = 101.0;
    assert!(edit.validate().is_err());
    edit.amount = f32::NAN;
    assert!(edit.validate().is_err());
    assert_eq!(edit.mix(), 0.0);
    edit.amount = 40.0;
    edit.name = "x".repeat(MAX_COLOR_LUT_NAME_CHARS + 1);
    assert!(edit.validate().is_err());
    assert_eq!(ColorLutEdit::new("   ", identity(2)).name, "Colour LUT");
    assert_eq!(
        ColorLutEdit::new(&"y".repeat(200), identity(2))
            .name
            .chars()
            .count(),
        64
    );
}

#[test]
fn reads_cube_files_and_names_them() {
    let folder = tempfile::tempdir().unwrap();
    let titled = folder.path().join("file-name.cube");
    std::fs::write(&titled, cube_text(2, |rgb| rgb)).unwrap();
    assert_eq!(
        ColorLutEdit::from_cube_file(&titled).unwrap().name,
        "Test look"
    );

    let untitled = folder.path().join("Kodak Gold.cube");
    std::fs::write(
        &untitled,
        "LUT_3D_SIZE 2\n".to_owned() + &"0 0 0\n".repeat(8),
    )
    .unwrap();
    assert_eq!(
        ColorLutEdit::from_cube_file(&untitled).unwrap().name,
        "Kodak Gold"
    );

    let missing = folder.path().join("missing.cube");
    assert!(matches!(
        ColorLutEdit::from_cube_file(&missing),
        Err(ColorLutError::Io(_))
    ));
}
