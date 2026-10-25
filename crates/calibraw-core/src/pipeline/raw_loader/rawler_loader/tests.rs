// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use rawler::formats::tiff::{
    writer::{DirectoryWriter, TiffWriter},
    Rational, SRational, Value,
};
use tempfile::NamedTempFile;

fn fixture(
    linear: bool,
    float: bool,
    jxl: bool,
    edit: impl FnOnce(&mut DirectoryWriter),
) -> NamedTempFile {
    sized_fixture(16, linear, float, jxl, edit)
}

fn sized_fixture(
    size: u32,
    linear: bool,
    float: bool,
    jxl: bool,
    edit: impl FnOnce(&mut DirectoryWriter),
) -> NamedTempFile {
    assert!(!jxl || size == 16, "the JPEG XL fixture is 16x16");
    let pixels = (size * size) as usize;
    let file = tempfile::Builder::new().suffix(".dng").tempfile().unwrap();
    let mut writer = TiffWriter::new(file.reopen().unwrap()).unwrap();
    let mut root = DirectoryWriter::new();
    let cpp = if linear { 3 } else { 1 };
    let data: Vec<u16> = if linear {
        [8192u16, 16384, 32768].repeat(pixels)
    } else {
        (0..pixels).map(|v| v as u16 + 100).collect()
    };
    let (offset, bytes) = if jxl {
        // Repository-owned synthetic 16x16 RGB fixture (no camera/third-party content)
        // with repeated 16-bit samples [8192, 16384, 32768], generated with cjxl 0.12.0:
        // cjxl input.ppm linearraw-16bit.jxl --distance=0 --effort=3
        let bytes = include_bytes!("../../../../tests/fixtures/linearraw-16bit.jxl");
        (writer.write_data(bytes).unwrap(), bytes.len() as u32)
    } else if float {
        let data: Vec<f32> = data.iter().map(|v| *v as f32 / 65535.0).collect();
        (
            writer.write_data_f32_le(&data).unwrap(),
            data.len() as u32 * 4,
        )
    } else {
        (
            writer.write_data_u16_le(&data).unwrap(),
            data.len() as u32 * 2,
        )
    };
    root.add_tag(TiffCommonTag::ImageWidth, size);
    root.add_tag(TiffCommonTag::ImageLength, size);
    root.add_tag(
        TiffCommonTag::BitsPerSample,
        Value::Short(vec![if float { 32u16 } else { 16u16 }; cpp]),
    );
    root.add_tag(
        TiffCommonTag::Compression,
        if jxl { 52546u16 } else { 1u16 },
    );
    root.add_tag(
        TiffCommonTag::PhotometricInt,
        if linear { 34892u16 } else { 32803u16 },
    );
    root.add_tag(TiffCommonTag::SamplesPerPixel, cpp as u16);
    root.add_tag(TiffCommonTag::RowsPerStrip, size);
    root.add_tag(TiffCommonTag::StripOffsets, offset);
    root.add_tag(TiffCommonTag::StripByteCounts, bytes);
    root.add_tag(TiffCommonTag::Make, "Test");
    root.add_tag(TiffCommonTag::Model, "Synthetic");
    root.add_tag(TiffCommonTag::Orientation, 1u16);
    root.add_untyped_tag(339, if float { 3u16 } else { 1u16 });
    root.add_tag(DngTag::DNGVersion, Value::Byte(vec![1u8, 7, 0, 0]));
    root.add_tag(DngTag::DNGBackwardVersion, Value::Byte(vec![1u8, 4, 0, 0]));
    root.add_tag(DngTag::UniqueCameraModel, "Test Synthetic");
    root.add_tag(
        DngTag::ColorMatrix1,
        Value::SRational(
            (0..9)
                .map(|i| SRational::new(if i % 4 == 0 { 1 } else { 0 }, 1))
                .collect::<Vec<_>>(),
        ),
    );
    root.add_tag(DngTag::CalibrationIlluminant1, 21u16);
    root.add_tag(
        DngTag::AsShotNeutral,
        Value::Rational(vec![
            Rational::new(1, 2),
            Rational::new(1, 1),
            Rational::new(1, 4),
        ]),
    );
    root.add_tag(DngTag::BlackLevel, Value::Short(vec![0u16; cpp]));
    root.add_tag(
        DngTag::WhiteLevel,
        Value::Long(vec![if float { 1u32 } else { 65535u32 }; cpp]),
    );
    root.add_tag(DngTag::BaselineExposure, SRational::new(1, 2));
    if !linear {
        root.add_untyped_tag(33421, Value::Short(vec![2u16, 2]));
        root.add_untyped_tag(33422, Value::Byte(vec![0u8, 1, 1, 2]));
        root.add_tag(DngTag::CFAPlaneColor, Value::Byte(vec![0u8, 1, 2]));
    }
    edit(&mut root);
    writer.build(root).unwrap();
    file
}

fn load(path: &Path) -> LoadedRaw {
    load_raw_file_with_profile_selection(path, CameraProfileMode::Automatic, None, None).unwrap()
}

#[test]
fn bayer_odd_crop_rotation_preserves_samples_cfa_and_black_phase() {
    let file = fixture(false, false, false, |root| {
        root.add_tag(TiffCommonTag::Orientation, 6u16);
        root.add_tag(DngTag::DefaultCropOrigin, Value::Long(vec![1u32, 1]));
        root.add_tag(DngTag::DefaultCropSize, Value::Long(vec![4u32, 6]));
        root.add_tag(DngTag::BlackLevelRepeatDim, Value::Short(vec![2u16, 2]));
        root.add_tag(DngTag::BlackLevel, Value::Short(vec![10u16, 20, 30, 40]));
    });
    let raw = load(file.path());
    assert_eq!([raw.width, raw.height], [6, 4]);
    // 90° clockwise: top-left is sensor (1,6), top-right is (1,1).
    assert_eq!(raw.raw_pixels[0], 197);
    assert_eq!(raw.raw_pixels[5], 117);
    assert_eq!(raw.color_indices[0], 1);
    assert_eq!(raw.color_indices[5], 2);
    assert_eq!(raw.black_levels_per_pixel[0], 20.0);
    assert_eq!(raw.black_levels_per_pixel[5], 40.0);
    assert_eq!(raw.black_levels, [10.0, 20.0, 40.0, 30.0]);
    assert_eq!(raw.color_indices.storage_slice().len(), 4);
    assert_eq!(raw.wb_coeffs, [2.0, 1.0, 4.0, 1.0]);
    assert_eq!(
        load_raw_display_metadata(file.path()).unwrap().dimensions,
        [6, 4]
    );
}

#[test]
fn integer_and_float_linearraw_keep_normalized_camera_rgb() {
    for float in [false, true] {
        let file = fixture(true, float, false, |root| {
            root.add_tag(DngTag::DefaultCropOrigin, Value::Long(vec![1u32, 2]));
            root.add_tag(DngTag::DefaultCropSize, Value::Long(vec![5u32, 7]));
            root.add_tag(TiffCommonTag::Orientation, 8u16);
        });
        let raw = load(file.path());
        assert_eq!([raw.width, raw.height], [7, 5]);
        assert!(raw.is_camera_linear_raster());
        assert!(raw.raw_pixels.is_empty());
        for (actual, expected) in raw.scene_linear_raster().unwrap()[..3].iter().zip([
            8192.0 / 65535.0,
            16384.0 / 65535.0,
            32768.0 / 65535.0,
        ]) {
            assert!((*actual - expected).abs() < 1e-6);
        }
        assert_eq!(raw.wb_coeffs, [2.0, 1.0, 4.0, 1.0]);
        assert!((raw.profile_color.camera_profile.default_exposure_ev - 0.5).abs() < 1e-6);
        assert!(raw.as_shot_white_balance().is_some());
        assert_ne!(
            raw.adjusted_white_balance_and_camera_transform(0.2, 0.0).0,
            raw.wb_coeffs
        );
    }
}

#[test]
fn jpeg_xl_dng_decodes_through_public_dng_route() {
    let file = fixture(true, false, true, |_| {});
    let raw = super::super::load_raw_file(file.path()).unwrap();
    assert!(raw.is_camera_linear_raster());
    assert_eq!([raw.width, raw.height], [16, 16]);
    for (actual, expected) in raw.scene_linear_raster().unwrap()[..3].iter().zip([
        8192.0 / 65535.0,
        16384.0 / 65535.0,
        32768.0 / 65535.0,
    ]) {
        assert!((*actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}

#[test]
#[cfg(any(target_os = "linux", target_os = "android"))]
fn jpeg_xl_dng_decodes_through_android_descriptor_path() {
    use std::os::fd::AsRawFd;

    let file = fixture(true, false, true, |root| {
        root.add_tag(DngTag::OpcodeList2, Value::Byte(vec![0u8; 4]));
    });
    let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    assert!(path.extension().is_none());
    assert!(super::super::path_is_dng(&path));

    let expected = super::super::load_raw_file(file.path()).unwrap();
    let actual = super::super::load_raw_file(&path).unwrap();
    assert!(actual.is_camera_linear_raster());
    assert_eq!(actual.scene_linear_raster(), expected.scene_linear_raster());
    assert_eq!(
        super::super::load_raw_display_dimensions(&path).unwrap(),
        [16, 16]
    );
    let thumbnail = super::super::load_raw_thumbnail(&path, 8).unwrap();
    assert!(thumbnail.width <= 8 && thumbnail.height <= 8);
    // This fixture has no preview: Rawler's diagnostic must survive the
    // LibRaw fallback, proving embedded-thumbnail requests route through it too.
    let error = super::super::load_raw_embedded_thumbnail(&path, 8).unwrap_err();
    assert!(format!("{error:#}").contains("Rawler embedded thumbnail decode failed first"));
}

#[test]
fn rejects_oversized_headers_and_tiles_before_pixel_decode() {
    for tiled in [false, true] {
        let file = fixture(true, false, true, |root| {
            if tiled {
                root.remove_tag(TiffCommonTag::StripOffsets);
                root.remove_tag(TiffCommonTag::StripByteCounts);
                root.add_tag(TiffCommonTag::TileOffsets, 8u32);
                root.add_tag(TiffCommonTag::TileByteCounts, 1u32);
                root.add_tag(TiffCommonTag::TileWidth, super::super::MAX_SENSOR_EDGE + 1);
                root.add_tag(TiffCommonTag::TileLength, 16u32);
            } else {
                root.add_tag(TiffCommonTag::ImageWidth, super::super::MAX_SENSOR_EDGE + 1);
            }
        });
        let error = load_raw_file_with_profile_selection(
            file.path(),
            CameraProfileMode::Automatic,
            None,
            None,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("sensor"), "{error:#}");
    }
}

#[test]
fn samsung_expert_raw_style_linear_jxl_layout_is_accepted() {
    let file = fixture(true, false, true, |root| {
        root.add_tag(
            TiffCommonTag::BitsPerSample,
            Value::Short(vec![14u16, 14, 14]),
        );
        root.add_tag(TiffCommonTag::Orientation, 6u16);
        root.add_tag(DngTag::BlackLevelRepeatDim, Value::Short(vec![2u16, 2]));
        root.add_tag(DngTag::BlackLevel, Value::Short(vec![0u16; 12]));
        root.add_tag(DngTag::WhiteLevel, Value::Long(vec![16383u32; 3]));
        root.add_tag(
            DngTag::CameraCalibration1,
            Value::SRational(vec![
                SRational::new(65, 64),
                SRational::new(0, 1),
                SRational::new(0, 1),
                SRational::new(0, 1),
                SRational::new(1, 1),
                SRational::new(0, 1),
                SRational::new(0, 1),
                SRational::new(0, 1),
                SRational::new(261, 256),
            ]),
        );
        root.add_tag(
            DngTag::ForwardMatrix1,
            Value::SRational(
                (0..9)
                    .map(|i| SRational::new(if i % 4 == 0 { 1 } else { 0 }, 1))
                    .collect(),
            ),
        );
        root.add_tag(DngTag::OpcodeList2, Value::Byte(vec![0u8; 4]));
    });
    let raw = super::super::load_raw_file(file.path()).unwrap();
    assert!(raw.is_camera_linear_raster());
    assert_eq!([raw.width, raw.height], [16, 16]);
}

#[test]
fn jpeg_xl_linearraw_with_opcode_list2_is_accepted() {
    let file = fixture(true, false, true, |root| {
        root.add_tag(DngTag::OpcodeList2, Value::Byte(vec![0u8; 4]));
    });
    let raw = super::super::load_raw_file(file.path()).unwrap();
    assert!(raw.is_camera_linear_raster());
    assert_eq!([raw.width, raw.height], [16, 16]);
}

#[test]
fn opcode_list2_does_not_block_phone_style_dngs() {
    let file = fixture(true, false, false, |root| {
        root.add_tag(DngTag::OpcodeList2, Value::Byte(vec![0u8; 4]))
    });
    let raw =
        load_raw_file_with_profile_selection(file.path(), CameraProfileMode::Automatic, None, None)
            .unwrap();
    assert!(raw.is_camera_linear_raster());
}

/// SOI, SOF3, an optional DRI and SOS; the entropy data is irrelevant because
/// only the header is inspected.
fn ljpeg_header(restart_interval: Option<u16>) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8];
    bytes.extend([0xFF, 0xC3, 0, 11, 12, 0, 16, 0, 16, 1, 0, 0x11, 0]);
    if let Some(ri) = restart_interval {
        bytes.extend([0xFF, 0xDD, 0, 4]);
        bytes.extend(ri.to_be_bytes());
    }
    bytes.extend([0xFF, 0xDA, 0, 8, 1, 0, 0, 1, 0, 0, 0, 0]);
    bytes
}

#[test]
fn ljpeg_restart_interval_is_found_only_in_the_header() {
    assert!(ljpeg_has_restart_interval(&ljpeg_header(Some(64000))));
    assert!(!ljpeg_has_restart_interval(&ljpeg_header(Some(0))));
    assert!(!ljpeg_has_restart_interval(&ljpeg_header(None)));
    let mut after_scan = ljpeg_header(None);
    after_scan.extend([0xFF, 0xDD, 0, 4, 0, 16]);
    assert!(!ljpeg_has_restart_interval(&after_scan));
    assert!(!ljpeg_has_restart_interval(&ljpeg_header(Some(16))[2..]));
}

fn lossless_jpeg_dng(restart_interval: Option<u16>) -> NamedTempFile {
    let file = tempfile::Builder::new().suffix(".dng").tempfile().unwrap();
    let mut writer = TiffWriter::new(file.reopen().unwrap()).unwrap();
    let stream = ljpeg_header(restart_interval);
    let offset = writer.write_data(&stream).unwrap();
    let mut root = DirectoryWriter::new();
    root.add_tag(TiffCommonTag::ImageWidth, 16u32);
    root.add_tag(TiffCommonTag::ImageLength, 16u32);
    root.add_tag(TiffCommonTag::BitsPerSample, Value::Short(vec![12u16; 3]));
    root.add_tag(TiffCommonTag::Compression, 7u16);
    root.add_tag(TiffCommonTag::PhotometricInt, 34892u16);
    root.add_tag(TiffCommonTag::SamplesPerPixel, 3u16);
    root.add_tag(TiffCommonTag::RowsPerStrip, 16u32);
    root.add_tag(TiffCommonTag::StripOffsets, offset);
    root.add_tag(TiffCommonTag::StripByteCounts, stream.len() as u32);
    root.add_tag(TiffCommonTag::Make, "Test");
    root.add_tag(TiffCommonTag::Model, "Synthetic");
    root.add_tag(DngTag::DNGVersion, Value::Byte(vec![1u8, 6, 0, 0]));
    root.add_tag(DngTag::UniqueCameraModel, "Test Synthetic");
    writer.build(root).unwrap();
    file
}

#[test]
fn lossless_jpeg_restart_markers_route_samples_through_libraw() {
    for (interval, expected) in [
        (Some(16), SampleDecoder::LibRaw),
        (Some(0), SampleDecoder::Rawler),
        (None, SampleDecoder::Rawler),
    ] {
        let file = lossless_jpeg_dng(interval);
        let input = open(file.path()).unwrap();
        assert_eq!(validate_layout(&input).unwrap(), expected, "{interval:?}");
    }
}

#[test]
fn libraw_dng_samples_match_rawler_file_order() {
    // LibRaw rejects images smaller than 22 pixels on an edge.
    for linear in [false, true] {
        let file = sized_fixture(32, linear, false, false, |_| {});
        let input = open(file.path()).unwrap();
        let image = input
            .decoder
            .raw_image(&input.source, &RawDecodeParams::default(), false)
            .unwrap();
        let RawImageData::Integer(expected) = image.data else {
            panic!("integer fixture");
        };
        let samples =
            shared::unpack_dng_samples(file.path(), [image.width, image.height], image.cpp)
                .unwrap();
        assert_eq!(samples, expected, "linear: {linear}");
    }
}

#[test]
fn unsupported_opcode_lists_black_level_deltas_and_channels_are_rejected() {
    for tag in [DngTag::OpcodeList1, DngTag::OpcodeList3] {
        let file = fixture(true, false, false, |root| {
            root.add_tag(tag, Value::Byte(vec![0u8; 4]))
        });
        let error = load_raw_file_with_profile_selection(
            file.path(),
            CameraProfileMode::Automatic,
            None,
            None,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("correction"));
    }
    for tag in [DngTag::BlackLevelDeltaH, DngTag::BlackLevelDeltaV] {
        let file = fixture(true, false, false, |root| {
            root.add_tag(tag, Value::Byte(vec![0u8; 4]))
        });
        let error = load_raw_file_with_profile_selection(
            file.path(),
            CameraProfileMode::Automatic,
            None,
            None,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("black-level correction"));
    }
    let file = fixture(true, false, false, |root| {
        root.add_tag(TiffCommonTag::SamplesPerPixel, 4u16)
    });
    assert!(load_raw_file_with_profile_selection(
        file.path(),
        CameraProfileMode::Automatic,
        None,
        None
    )
    .is_err());
}

#[test]
fn dng_thumbnail_uses_preview_chain_and_bounded_rawler_fallback() {
    // This synthetic DNG deliberately has no root NewSubFileType/JPEG preview.
    // The old hand-rolled thumbnail parser rejected it immediately and the UI
    // entered the retry loop. The public thumbnail path must still produce a
    // preview, allowing Rawler development only because this source is tiny.
    let file = fixture(true, false, false, |_| {});
    let thumbnail = super::super::load_raw_thumbnail(file.path(), 8).unwrap();
    assert!(thumbnail.width > 0 && thumbnail.height > 0);
    assert!(thumbnail.width <= 8 && thumbnail.height <= 8);
    assert_eq!(
        thumbnail.rgba.len(),
        thumbnail.width as usize * thumbnail.height as usize * 4
    );
}

#[test]
fn rawler_thumbnail_development_is_limited_to_small_sources() {
    assert!(super::rawler_thumbnail_fallback_allowed(2048, 2048));
    assert!(!super::rawler_thumbnail_fallback_allowed(2049, 2048));
    assert!(!super::rawler_thumbnail_fallback_allowed(2048, 2049));
}

#[test]
fn malformed_and_truncated_dngs_return_errors() {
    let file = tempfile::Builder::new().suffix(".dng").tempfile().unwrap();
    std::fs::write(file.path(), b"II*\0\xff\xff\xff\xff").unwrap();
    assert!(load_raw_display_metadata(file.path()).is_err());
    assert!(super::super::load_raw_file(file.path()).is_err());
    let file = fixture(true, false, false, |root| {
        root.add_tag(TiffCommonTag::StripByteCounts, u32::MAX)
    });
    let error =
        load_raw_file_with_profile_selection(file.path(), CameraProfileMode::Automatic, None, None)
            .unwrap_err();
    assert!(format!("{error:#}").contains("outside the file"));
}

#[test]
fn explicit_dcp_selection_is_applied() {
    let file = fixture(true, false, false, |_| {});
    let profile = fixture(true, false, false, |root| {
        root.add_tag(DngTag::ProfileName, "Selected profile");
        root.add_tag(DngTag::BaselineExposureOffset, SRational::new(1, 4));
    });
    let raw = load_raw_file_with_dcp(file.path(), profile.path()).unwrap();
    assert_eq!(
        raw.profile_color.camera_profile_source.as_deref(),
        Some(profile.path())
    );
    assert_eq!(
        raw.profile_color.camera_profile.name.as_deref(),
        Some("Selected profile")
    );
    assert!((raw.profile_color.camera_profile.default_exposure_ev - 0.75).abs() < 1e-6);
}

#[test]
fn proprietary_geometry_applies_crop_and_exif_orientation() {
    let geometry =
        Geometry::from_sensor_crop(6064, 4040, Some((8, 4, 6048, 4032)), Some(6)).unwrap();
    assert_eq!(geometry.dimensions(), [4032, 6048]);
    let (x, y) = geometry.source(0, 0);
    assert!((8..6056).contains(&x) && (4..4036).contains(&y));

    // Missing or invalid EXIF orientation keeps the sensor upright.
    for orientation in [None, Some(0), Some(9)] {
        let geometry = Geometry::from_sensor_crop(64, 40, None, orientation).unwrap();
        assert_eq!(geometry.dimensions(), [64, 40]);
        assert_eq!(geometry.source(3, 2), (3, 2));
    }
    assert!(Geometry::from_sensor_crop(64, 40, Some((8, 4, 57, 36)), None).is_err());
    assert!(Geometry::from_sensor_crop(64, 40, Some((8, 4, 56, 37)), None).is_err());
}

#[test]
fn camera_database_profile_prefers_a_and_d65_endpoints() {
    let matrix = |seed: f32| (0..9).map(|i| seed + i as f32).collect::<Vec<_>>();
    let matrices = HashMap::from([
        (Illuminant::Daylight, matrix(100.0)),
        (Illuminant::D65, matrix(20.0)),
        (Illuminant::A, matrix(10.0)),
        (Illuminant::D50, vec![1.0; 12]),
    ]);
    let color = CameraColor::from_camera_database(&matrices).unwrap();
    let [first, second] = &color.embedded.matrices;
    assert_eq!(first.illuminant, Some(17));
    assert_eq!(second.illuminant, Some(21));
    assert_eq!(
        first.color_matrix,
        Some([
            [10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0],
            [16.0, 17.0, 18.0],
            [0.0; 3]
        ])
    );
    assert!(!color.declares_as_shot_neutral);
    assert_eq!(color.baseline_exposure, None);

    let unusable = HashMap::from([
        (Illuminant::D65, vec![1.0; 12]),
        (Illuminant::A, vec![f32::NAN; 9]),
    ]);
    assert!(CameraColor::from_camera_database(&unusable).is_err());
}

#[test]
fn proprietary_loader_leaves_dngs_to_the_dng_backend() {
    let file = fixture(false, false, false, |_| {});
    let error = load_proprietary_raw_file_with_profile_selection(
        file.path(),
        CameraProfileMode::Automatic,
        None,
        None,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Rawler DNG backend"));
}

/// A DCP for the fixture camera named `name`, whose colour matrix scales red
/// by `red` and which offsets the default exposure by `offset` quarters.
fn dcp_fixture(folder: &Path, name: &str, red: i32, offset: i32) -> std::path::PathBuf {
    let file = fixture(false, false, false, |root| {
        root.add_tag(DngTag::ProfileName, name);
        root.add_tag(
            DngTag::ColorMatrix1,
            Value::SRational(
                (0..9)
                    .map(|i| match i {
                        0 => SRational::new(red, 4),
                        4 | 8 => SRational::new(1, 1),
                        _ => SRational::new(0, 1),
                    })
                    .collect::<Vec<_>>(),
            ),
        );
        root.add_tag(DngTag::BaselineExposureOffset, SRational::new(offset, 4));
    });
    let path = folder.join(format!("{name}.dcp"));
    std::fs::copy(file.path(), &path).unwrap();
    path
}

/// Asserts that `recolored` has exactly the colour, profile list and
/// samples of `decoded`.
fn assert_same_colour(recolored: &LoadedRaw, decoded: &LoadedRaw) {
    let (left, right) = (&recolored.profile_color, &decoded.profile_color);
    assert_eq!(
        left.cam_to_srgb.map(|row| row.map(f32::to_bits)),
        right.cam_to_srgb.map(|row| row.map(f32::to_bits))
    );
    assert_eq!(
        format!("{:?}", left.camera_profile),
        format!("{:?}", right.camera_profile)
    );
    assert_eq!(left.camera_profile_source, right.camera_profile_source);
    assert_eq!(left.white_balance_model, right.white_balance_model);
    assert_eq!(
        recolored.available_camera_profiles,
        decoded.available_camera_profiles
    );
    assert_eq!(recolored.wb_coeffs, decoded.wb_coeffs);
    assert_eq!(recolored.raw_pixels, decoded.raw_pixels);
    assert_eq!(
        recolored.scene_linear_raster.as_deref(),
        decoded.scene_linear_raster.as_deref()
    );
}

/// Decoding with one camera profile and switching to another gives exactly
/// what decoding with the other gives, for every choice a decode can make.
fn assert_profile_switches_match_decodes(
    decode: impl Fn(&Path, CameraProfileMode, Option<&Path>, Option<&Path>) -> LoadedRaw,
    size: u32,
    linear: bool,
) {
    let folder = tempfile::tempdir().unwrap();
    let first = dcp_fixture(folder.path(), "First", 5, 1);
    let second = dcp_fixture(folder.path(), "Second", 3, -2);
    let file = sized_fixture(size, linear, false, false, |_| {});
    let folder = Some(folder.path());
    for mode in [CameraProfileMode::Automatic, CameraProfileMode::DcpProfiles] {
        let decoded_first = decode(file.path(), mode, folder, Some(&first));
        assert_eq!(
            decoded_first.profile_color.camera_profile_source.as_deref(),
            Some(first.as_path())
        );
        let decoded_second = decode(file.path(), mode, folder, Some(&second));
        assert_ne!(
            decoded_first.profile_color.cam_to_srgb, decoded_second.profile_color.cam_to_srgb,
            "the fixture DCPs give different colour"
        );
        for selected in [Some(second.as_path()), None, Some(first.as_path())] {
            let decoded = decode(file.path(), mode, folder, selected);
            let recolored = decoded_first
                .with_camera_profile_selection(mode, folder, selected)
                .expect("the decode keeps its colour source")
                .unwrap();
            assert_same_colour(&recolored, &decoded);
        }
    }

    // A decode without DCPs cannot match one, so it is decoded again.
    let matrix_only = decode(file.path(), CameraProfileMode::MatrixOnly, folder, None);
    assert!(matrix_only
        .with_camera_profile_selection(CameraProfileMode::Automatic, folder, Some(&second))
        .is_none());
    let recolored = matrix_only
        .with_camera_profile_selection(CameraProfileMode::MatrixOnly, folder, Some(&second))
        .unwrap()
        .unwrap();
    assert_same_colour(
        &recolored,
        &decode(
            file.path(),
            CameraProfileMode::MatrixOnly,
            folder,
            Some(&second),
        ),
    );
}

#[test]
fn switching_camera_profiles_matches_decoding_with_them() {
    let rawler = |path: &Path, mode, folder: Option<&Path>, selected: Option<&Path>| {
        load_raw_file_with_profile_selection(path, mode, folder, selected).unwrap()
    };
    assert_profile_switches_match_decodes(rawler, 16, false);
    assert_profile_switches_match_decodes(rawler, 16, true);
}

#[test]
fn switching_camera_profiles_matches_decoding_with_them_through_libraw() {
    let libraw = |path: &Path, mode, folder: Option<&Path>, selected: Option<&Path>| {
        shared::load_raw_file_with_profile_selection(path, mode, folder, selected).unwrap()
    };
    // LibRaw rejects images smaller than 22 pixels on an edge.
    assert_profile_switches_match_decodes(libraw, 32, false);
}

#[test]
fn a_profile_switch_shares_the_samples_and_starts_derived_work_again() {
    let folder = tempfile::tempdir().unwrap();
    let second = dcp_fixture(folder.path(), "Second", 3, -2);
    let file = fixture(false, false, false, |_| {});
    let decoded = load(file.path());
    decoded
        .set_ai_denoised_image(
            crate::pipeline::AiDenoisedImage::new_bayer_cfa(16, 16, vec![1; 256]).unwrap(),
        )
        .unwrap();
    let recolored = decoded
        .with_camera_profile_selection(
            CameraProfileMode::Automatic,
            Some(folder.path()),
            Some(&second),
        )
        .unwrap()
        .unwrap();
    assert!(std::ptr::eq(
        decoded.raw_pixels.as_ptr(),
        recolored.raw_pixels.as_ptr()
    ));
    assert!(recolored.ai_denoised_image().is_none());
    assert!(!Arc::ptr_eq(
        &decoded.opposed_chroma_cache,
        &recolored.opposed_chroma_cache
    ));

    // A view derived from the decode, such as its lens-corrected copy, takes
    // the new colour and shares the recoloured decode's estimate.
    let view = decoded.derive_with(
        16,
        16,
        decoded.raw_pixels.to_vec(),
        decoded.color_indices.clone(),
        decoded.black_levels_per_pixel.clone(),
    );
    let recolored_view = view.with_profile_color_of(&recolored);
    assert!(Arc::ptr_eq(
        &recolored_view.profile_color,
        &recolored.profile_color
    ));
    assert!(Arc::ptr_eq(
        &recolored_view.opposed_chroma_cache,
        &recolored.opposed_chroma_cache
    ));
}
