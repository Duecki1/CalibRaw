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
    let file = tempfile::Builder::new().suffix(".dng").tempfile().unwrap();
    let mut writer = TiffWriter::new(file.reopen().unwrap()).unwrap();
    let mut root = DirectoryWriter::new();
    let cpp = if linear { 3 } else { 1 };
    let data: Vec<u16> = if linear {
        [8192u16, 16384, 32768].repeat(256)
    } else {
        (0..256).map(|v| v as u16 + 100).collect()
    };
    let (offset, size) = if jxl {
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
    root.add_tag(TiffCommonTag::ImageWidth, 16u32);
    root.add_tag(TiffCommonTag::ImageLength, 16u32);
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
    root.add_tag(TiffCommonTag::RowsPerStrip, 16u32);
    root.add_tag(TiffCommonTag::StripOffsets, offset);
    root.add_tag(TiffCommonTag::StripByteCounts, size);
    root.add_tag(TiffCommonTag::Make, "Test");
    root.add_tag(TiffCommonTag::Model, "Synthetic");
    root.add_tag(TiffCommonTag::Orientation, 1u16);
    root.add_untyped_tag(339, if float { 3u16 } else { 1u16 });
    root.add_tag(DngTag::DNGVersion, Value::Byte(vec![1u8, 7, 0, 0]));
    root.add_tag(DngTag::DNGBackwardVersion, Value::Byte(vec![1u8, 4, 0, 0]));
    root.add_tag(DngTag::UniqueCameraModel, "Test Synthetic");
    root.add_tag(
        DngTag::ColorMatrix1,
        Value::SRational((0..9)
            .map(|i| SRational::new(if i % 4 == 0 { 1 } else { 0 }, 1))
            .collect::<Vec<_>>()),
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
    assert_eq!(raw.black_levels_per_pixel.storage_slice().len(), 4);
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
        assert!((raw.camera_profile.default_exposure_ev - 0.5).abs() < 1e-6);
        assert!(raw.as_shot_white_balance().is_some());
        assert_ne!(
            raw.adjusted_white_balance_and_camera_transform(0.2, 0.0).0,
            raw.wb_coeffs
        );
    }
}

#[test]
fn jpeg_xl_dng_decodes_through_public_fallback() {
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
fn unsupported_corrections_and_channels_are_rejected() {
    for tag in [
        DngTag::OpcodeList1,
        DngTag::OpcodeList2,
        DngTag::OpcodeList3,
        DngTag::BlackLevelDeltaH,
        DngTag::BlackLevelDeltaV,
    ] {
        let file = fixture(true, false, false, |root| root.add_tag(tag, Value::Byte(vec![0u8; 4])));
        let error = load_raw_file_with_profile_selection(
            file.path(),
            CameraProfileMode::Automatic,
            None,
            None,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("correction"));
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
    assert_eq!(raw.camera_profile_source.as_deref(), Some(profile.path()));
    assert_eq!(raw.camera_profile.name.as_deref(), Some("Selected profile"));
    assert!((raw.camera_profile.default_exposure_ev - 0.75).abs() < 1e-6);
}
