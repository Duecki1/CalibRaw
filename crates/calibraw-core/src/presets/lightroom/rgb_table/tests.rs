use super::*;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

/// The inverse of `decode_text`, as Adobe's encoder writes it.
fn encode_text(bytes: &[u8]) -> String {
    let mut text = String::new();
    for chunk in bytes.chunks(4) {
        let mut padded = [0u8; 4];
        padded[..chunk.len()].copy_from_slice(chunk);
        let mut value = u32::from_le_bytes(padded);
        for _ in 0..=chunk.len() {
            text.push(ALPHABET[(value % 85) as usize] as char);
            value /= 85;
        }
    }
    text
}

pub(in crate::presets::lightroom) struct TableSpec {
    pub(in crate::presets::lightroom) divisions: u32,
    pub(in crate::presets::lightroom) primaries: u32,
    pub(in crate::presets::lightroom) gamma: u32,
    /// The output for the colour at these indices, as `[0, 1]` values.
    pub(in crate::presets::lightroom) output: fn([f64; 3]) -> [f64; 3],
}

/// Builds the text of an RGB table the way Adobe stores it.
pub(in crate::presets::lightroom) fn table_text(spec: &TableSpec) -> String {
    let divisions = spec.divisions as usize;
    let nop = |index: usize| ((index * 0xFFFF + (divisions >> 1)) / (divisions - 1)) as u16;
    let mut stream = Vec::new();
    for value in [1u32, 1, 3, spec.divisions] {
        stream.extend(value.to_le_bytes());
    }
    for red in 0..divisions {
        for green in 0..divisions {
            for blue in 0..divisions {
                let last = (divisions - 1) as f64;
                let output =
                    (spec.output)([red as f64 / last, green as f64 / last, blue as f64 / last]);
                for (value, index) in output.into_iter().zip([red, green, blue]) {
                    let target = (value * 65535.0).round() as u16;
                    stream.extend(target.wrapping_sub(nop(index)).to_le_bytes());
                }
            }
        }
    }
    for value in [spec.primaries, spec.gamma, 0u32] {
        stream.extend(value.to_le_bytes());
    }
    stream.extend(0.0f64.to_le_bytes());
    stream.extend(1.0f64.to_le_bytes());
    stream.extend(0u32.to_le_bytes());

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&stream).unwrap();
    let mut packed = (stream.len() as u32).to_le_bytes().to_vec();
    packed.extend(encoder.finish().unwrap());
    encode_text(&packed)
}

fn identity(rgb: [f64; 3]) -> [f64; 3] {
    rgb
}

fn invert_red(rgb: [f64; 3]) -> [f64; 3] {
    [1.0 - rgb[0], rgb[1], rgb[2]]
}

fn close(left: [f32; 3], right: [f32; 3], tolerance: f32) -> bool {
    left.iter()
        .zip(right)
        .all(|(a, b)| (a - b).abs() <= tolerance)
}

#[test]
fn base85_text_round_trips_every_tail_length() {
    for length in 0..40usize {
        let bytes: Vec<u8> = (0..length).map(|index| (index * 37 + 11) as u8).collect();
        let text = encode_text(&bytes);
        assert_eq!(decode_text(&text), bytes, "length {length}");
        // Whitespace and line breaks inside the property are ignored.
        let wrapped = text
            .chars()
            .flat_map(|c| [c, '\n', ' '])
            .collect::<String>();
        assert_eq!(decode_text(&wrapped), bytes, "wrapped, length {length}");
    }
}

#[test]
fn the_alphabet_matches_adobes_character_set() {
    assert_eq!(ALPHABET.len(), 85);
    let mut seen = ALPHABET.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 85, "every character is distinct");
    // Characters that XMP and Adobe's decoder treat as separators or noise.
    for excluded in [b' ', b'"', b'&', b'<', b'>', b',', b';', b'_', b'\\', b'~'] {
        assert!(!ALPHABET.contains(&excluded), "{}", excluded as char);
    }
}

#[test]
fn identity_tables_in_any_space_give_an_identity_look() {
    // (primaries, gamma) pairs: sRGB/sRGB, ProPhoto/sRGB, ProPhoto/linear, P3/2.2, Rec2020/Rec2020.
    for (primaries, gamma) in [(0, 1), (2, 1), (2, 0), (3, 3), (4, 4), (1, 2)] {
        let text = table_text(&TableSpec {
            divisions: 9,
            primaries,
            gamma,
            output: identity,
        });
        let lut = decode_srgb_table(&text).unwrap();
        assert_eq!(lut.edge(), RESULT_EDGE);
        // Table interpolation in a non-sRGB curve bends identity slightly.
        for rgb in [
            [0.0; 3],
            [1.0; 3],
            [0.5; 3],
            [0.2, 0.5, 0.8],
            [0.9, 0.3, 0.1],
        ] {
            let found = lut.sample(rgb);
            assert!(
                close(found, rgb, 0.012),
                "primaries {primaries} gamma {gamma}: {rgb:?} -> {found:?}"
            );
        }
    }
}

#[test]
fn a_table_that_inverts_red_inverts_red() {
    let text = table_text(&TableSpec {
        divisions: 17,
        primaries: 0,
        gamma: 1,
        output: invert_red,
    });
    let lut = decode_srgb_table(&text).unwrap();
    assert!(close(lut.sample([0.0, 0.4, 0.4]), [1.0, 0.4, 0.4], 0.003));
    assert!(close(lut.sample([1.0, 0.4, 0.4]), [0.0, 0.4, 0.4], 0.003));
    assert!(close(lut.sample([0.5, 0.2, 0.9]), [0.5, 0.2, 0.9], 0.02));
}

#[test]
fn tables_in_wider_primaries_keep_neutral_colours_neutral() {
    // Greys have equal channels in every space, so any look that darkens all
    // channels equally must leave them grey after conversion.
    let text = table_text(&TableSpec {
        divisions: 17,
        primaries: 2,
        gamma: 1,
        output: |rgb| rgb.map(|value| value * 0.5),
    });
    let lut = decode_srgb_table(&text).unwrap();
    for grey in [0.2f32, 0.5, 0.8] {
        let [r, g, b] = lut.sample([grey; 3]);
        assert!(
            (r - g).abs() < 0.004 && (g - b).abs() < 0.004,
            "{grey}: {r} {g} {b}"
        );
        assert!(r < grey);
    }
}

#[test]
fn white_maps_to_white_between_the_supported_primaries() {
    let srgb = Primaries::Srgb.to_xyz_d50();
    for primaries in [
        Primaries::AdobeRgb,
        Primaries::ProPhoto,
        Primaries::DisplayP3,
        Primaries::Rec2020,
    ] {
        let into = multiply(&invert(&primaries.to_xyz_d50()), &srgb);
        let white = apply(&into, [1.0; 3]);
        assert!(
            white.iter().all(|value| (value - 1.0).abs() < 1e-3),
            "{primaries:?}: {white:?}"
        );
    }
    // sRGB red is inside ProPhoto: positive and below one.
    let into = multiply(&invert(&Primaries::ProPhoto.to_xyz_d50()), &srgb);
    let red = apply(&into, [1.0, 0.0, 0.0]);
    assert!(
        red.iter().all(|value| (0.0..1.0).contains(value)),
        "{red:?}"
    );
}

#[test]
fn transfer_curves_invert_each_other() {
    for gamma in [
        Gamma::Linear,
        Gamma::Srgb,
        Gamma::Power1_8,
        Gamma::Power2_2,
        Gamma::Rec2020,
    ] {
        for value in [0.0, 0.001, 0.02, 0.18, 0.5, 1.0] {
            let back = gamma.decode(gamma.encode(value));
            assert!((back - value).abs() < 1e-9, "{gamma:?} {value}: {back}");
        }
    }
}

#[test]
fn rejects_damaged_unsupported_and_oversized_data() {
    let good = table_text(&TableSpec {
        divisions: 3,
        primaries: 0,
        gamma: 1,
        output: identity,
    });
    assert!(decode_srgb_table(&good).is_ok());
    for (text, expected) in [
        (String::new(), "too short"),
        ("0000".to_owned(), "too short"),
        (good[..good.len() / 2].to_owned(), "LUT data"),
        (
            encode_text(
                &u32::MAX
                    .to_le_bytes()
                    .iter()
                    .chain(&[0u8; 8])
                    .copied()
                    .collect::<Vec<_>>(),
            ),
            "unreasonable",
        ),
    ] {
        let message = decode_srgb_table(&text).unwrap_err().to_string();
        assert!(message.contains(expected), "{message}");
    }

    let packed = |stream: Vec<u8>| {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&stream).unwrap();
        let mut bytes = (stream.len() as u32).to_le_bytes().to_vec();
        bytes.extend(encoder.finish().unwrap());
        encode_text(&bytes)
    };
    let header = |kind: u32, version: u32, dimensions: u32, divisions: u32| {
        [kind, version, dimensions, divisions]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<u8>>()
    };
    for (stream, expected) in [
        (header(0, 1, 3, 4), "not an RGB table"),
        (header(1, 2, 3, 4), "version"),
        (header(1, 1, 1, 4), "3D"),
        (header(1, 1, 3, 1), "invalid size"),
        (header(1, 1, 3, 33), "invalid size"),
        (header(1, 1, 3, 2), "ends early"),
    ] {
        let message = decode_srgb_table(&packed(stream)).unwrap_err().to_string();
        assert!(message.contains(expected), "{message}");
    }
}
