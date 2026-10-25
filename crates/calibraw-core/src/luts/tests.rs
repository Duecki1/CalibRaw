use super::*;
use std::fmt::Write as _;

/// A `.cube` file whose entries are `map` of the lattice coordinates (0–1).
fn cube_text(size: u32, header: &str, map: impl Fn([f32; 3]) -> [f32; 3]) -> String {
    let mut text = format!("{header}LUT_3D_SIZE {size}\n");
    let last = (size - 1) as f32;
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let [x, y, z] = map([r as f32 / last, g as f32 / last, b as f32 / last]);
                writeln!(text, "{x:.6} {y:.6} {z:.6}").unwrap();
            }
        }
    }
    text
}

fn warm([r, g, b]: [f32; 3]) -> [f32; 3] {
    [(r * 1.1).min(1.0), g * g, b * 0.8 + 0.05]
}

fn close(left: [f32; 3], right: [f32; 3], tolerance: f32) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (left - right).abs() <= tolerance)
}

#[test]
fn parses_keywords_comments_and_crlf() {
    let text = cube_text(
        2,
        "\u{feff}# made by a grading tool\r\nTITLE \"Warm Film\"\r\nLUT_IN_VIDEO_RANGE\r\n",
        |rgb| rgb,
    )
    .replace('\n', "\r\n");
    let lut = parse_cube(text.as_bytes()).unwrap();
    assert_eq!(lut.title(), Some("Warm Film"));
    assert_eq!(lut.size(), 2);
    assert_eq!(lut.table().len(), 8);
    assert_eq!(lut.table()[1], [1.0, 0.0, 0.0], "red changes fastest");
    assert_eq!(lut.domain_min(), [0.0; 3]);
    assert_eq!(lut.domain_max(), [1.0; 3]);
    assert_eq!(lut.id(), LutId::of_bytes(text.as_bytes()));
}

#[test]
fn reads_both_domain_forms() {
    let lut =
        parse_cube(cube_text(2, "DOMAIN_MIN 0 0 -0.5\nDOMAIN_MAX 1 2 1\n", |rgb| rgb).as_bytes())
            .unwrap();
    assert_eq!(lut.domain_min(), [0.0, 0.0, -0.5]);
    assert_eq!(lut.domain_max(), [1.0, 2.0, 1.0]);

    let lut =
        parse_cube(cube_text(2, "LUT_3D_INPUT_RANGE 0.0 4.0\n", |rgb| rgb).as_bytes()).unwrap();
    assert_eq!(lut.domain_max(), [4.0; 3]);
    assert!(close(
        lut.sample_tetrahedral([2.0, 1.0, 4.0]),
        [0.5, 0.25, 1.0],
        1e-6
    ));
}

#[test]
fn refuses_malformed_files() {
    let identity = |rgb| rgb;
    for (text, expected) in [
        ("0 0 0\n".to_owned(), CubeLutError::MissingSize),
        ("TITLE \"none\"\n".to_owned(), CubeLutError::MissingSize),
        (
            "LUT_1D_SIZE 2\n0 0 0\n1 1 1\n".to_owned(),
            CubeLutError::Unsupported1d,
        ),
        (
            "LUT_3D_SIZE 1\n0 0 0\n".to_owned(),
            CubeLutError::SizeOutOfRange(1),
        ),
        (
            "LUT_3D_SIZE 66\n".to_owned(),
            CubeLutError::SizeOutOfRange(66),
        ),
        (
            cube_text(2, "", identity).replacen("1.000000 1.000000 1.000000\n", "", 1),
            CubeLutError::WrongEntryCount {
                expected: 8,
                found: 7,
            },
        ),
        (
            cube_text(2, "", identity) + "0 0 0\n",
            CubeLutError::WrongEntryCount {
                expected: 8,
                found: 9,
            },
        ),
        (
            cube_text(2, "", identity).replacen("0.000000 0.000000 0.000000", "0 0", 1),
            CubeLutError::InvalidLine(2),
        ),
        (
            cube_text(2, "", identity).replacen("0.000000 0.000000 0.000000", "0 NaN 0", 1),
            CubeLutError::NonFinite(2),
        ),
        (
            cube_text(2, "DOMAIN_MIN 0 1 0\n", identity),
            CubeLutError::InvalidDomain,
        ),
        (
            cube_text(2, "", identity) + "TITLE \"late\"\n",
            CubeLutError::InvalidLine(10),
        ),
    ] {
        assert_eq!(parse_cube(text.as_bytes()), Err(expected), "{text}");
    }
}

#[test]
fn tetrahedral_interpolation_is_exact_on_the_lattice_and_linear_maps() {
    let lut = parse_cube(cube_text(5, "", warm).as_bytes()).unwrap();
    for b in 0..5 {
        for g in 0..5 {
            for r in 0..5 {
                let rgb = [r as f32 / 4.0, g as f32 / 4.0, b as f32 / 4.0];
                assert!(
                    close(lut.sample_tetrahedral(rgb), warm(rgb), 1e-6),
                    "{rgb:?}"
                );
            }
        }
    }

    // Tetrahedral interpolation reproduces any affine map exactly.
    let affine = |[r, g, b]: [f32; 3]| [0.2 + 0.5 * r + 0.1 * g, 0.9 * g - 0.2 * b, 0.3 * r + b];
    let lut = parse_cube(cube_text(3, "", affine).as_bytes()).unwrap();
    for rgb in [
        [0.13, 0.71, 0.42],
        [0.9, 0.2, 0.55],
        [0.33, 0.33, 0.9],
        [0.6, 0.6, 0.6],
    ] {
        assert!(
            close(lut.sample_tetrahedral(rgb), affine(rgb), 1e-5),
            "{rgb:?}"
        );
    }
}

#[test]
fn inputs_outside_the_domain_are_clamped() {
    let lut = parse_cube(cube_text(3, "", warm).as_bytes()).unwrap();
    assert_eq!(
        lut.sample_tetrahedral([-1.0, 2.0, 0.5]),
        lut.sample_tetrahedral([0.0, 1.0, 0.5])
    );
}

#[test]
fn lut_ids_round_trip_as_hex() {
    let id = LutId::of_bytes(b"LUT_3D_SIZE 2");
    let text = serde_json::to_string(&id).unwrap();
    assert_eq!(text.len(), 18);
    assert_eq!(serde_json::from_str::<LutId>(&text).unwrap(), id);
    assert!(serde_json::from_str::<LutId>("\"12\"").is_err());
    assert!(serde_json::from_str::<LutId>("\"zzzzzzzzzzzzzzzz\"").is_err());
}

#[test]
fn look_luts_default_to_the_full_amount() {
    let id = LutId::of_bytes(b"x");
    let look: LookLut =
        serde_json::from_str(&format!("{{\"id\":\"{id}\",\"name\":\"Film\"}}")).unwrap();
    assert_eq!(look.amount, LookLut::DEFAULT_AMOUNT);
}

#[test]
fn the_library_imports_renames_and_removes_luts() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("luts");
    let mut library = LutLibrary::open(path.clone()).unwrap();
    assert!(library.entries().is_empty());

    let titled = cube_text(2, "TITLE \"Film\"\n", warm);
    let imported = library.import(titled.as_bytes(), "ignored.cube").unwrap();
    assert!(imported.added);
    assert_eq!(imported.entry.name, "Film");
    assert_eq!(imported.entry.size, 2);
    let again = library.import(titled.as_bytes(), "copy.cube").unwrap();
    assert!(!again.added);
    assert_eq!(again.entry, imported.entry);

    let untitled = cube_text(3, "", warm);
    let second = library.import(untitled.as_bytes(), "Film.CUBE").unwrap();
    assert_eq!(second.entry.name, "Film 2", "names stay unique");

    assert!(library.rename(second.entry.id, " film ").is_err());
    assert_eq!(library.rename(second.entry.id, "Arctic").unwrap(), "Arctic");
    let names: Vec<_> = library
        .entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["Arctic", "Film"]);

    let reopened = LutLibrary::open(path.clone()).unwrap();
    assert_eq!(reopened.entries(), library.entries());
    let lut = reopened.store().load(second.entry.id).unwrap();
    assert_eq!(lut.size(), 3);
    assert!(Arc::ptr_eq(
        &lut,
        &reopened.store().load(second.entry.id).unwrap()
    ));

    library.remove(imported.entry.id).unwrap();
    assert!(library.entry(imported.entry.id).is_none());
    assert!(matches!(
        library.store().load(imported.entry.id),
        Err(LutLibraryError::NotFound(_))
    ));
    assert_eq!(LutLibrary::open(path).unwrap().entries().len(), 1);
}

#[test]
fn the_library_refuses_invalid_files_and_drops_missing_ones() {
    let folder = tempfile::tempdir().unwrap();
    let mut library = LutLibrary::open(folder.path().to_owned()).unwrap();
    assert!(matches!(
        library.import(b"LUT_3D_SIZE 2\n", "broken.cube"),
        Err(LutLibraryError::Cube(CubeLutError::WrongEntryCount { .. }))
    ));
    assert!(library.entries().is_empty());

    let imported = library
        .import(cube_text(2, "", warm).as_bytes(), "warm.cube")
        .unwrap();
    std::fs::remove_file(folder.path().join(format!("{}.cube", imported.entry.id))).unwrap();
    assert!(LutLibrary::open(folder.path().to_owned())
        .unwrap()
        .entries()
        .is_empty());
}
