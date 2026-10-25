use super::*;
use crate::pipeline::{AdjustmentGroup, MaskEffect, PointCurve, WhiteBalance};
use crate::presets::{Preset, PresetError};

fn document(attributes: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" {attributes}>
{body}
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>"#
    )
}

fn import(attributes: &str, body: &str) -> LightroomImport {
    import_lightroom_xmp(document(attributes, body).as_bytes()).expect("the XMP preset imports")
}

fn curve_points(curve: &PointCurve) -> Vec<[f32; 2]> {
    curve.points[..curve.len as usize].to_vec()
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1e-5,
        "expected {expected}, got {actual}"
    );
}

fn assert_close_point(actual: [f32; 2], expected: [f32; 2]) {
    assert_close(actual[0], expected[0]);
    assert_close(actual[1], expected[1]);
}

#[test]
fn imports_global_settings_and_ignores_the_embedded_preset() {
    let import = import(
        r#"crs:WhiteBalance="As Shot"
           crs:Exposure2012="0.25"
           crs:Whites2012="+18"
           crs:Blacks2012="-22"
           crs:Vibrance="14"
           crs:Saturation="-2"
           crs:Texture="7"
           crs:Clarity2012="11"
           crs:Dehaze="-3"
           crs:Sharpness="40"
           crs:SaturationAdjustmentOrange="-12"
           crs:LuminanceAdjustmentOrange="8"
           crs:ColorGradeGlobalHue="20""#,
        r#"
<crs:Name>
 <rdf:Alt><rdf:li xml:lang="x-default">Synthetic Look</rdf:li></rdf:Alt>
</crs:Name>
<crs:Group>
 <rdf:Alt><rdf:li xml:lang="x-default">Test Group</rdf:li></rdf:Alt>
</crs:Group>
<crs:ToneCurvePV2012>
 <rdf:Seq>
  <rdf:li>0, 10</rdf:li>
  <rdf:li>64, 80</rdf:li>
  <rdf:li>192, 210</rdf:li>
  <rdf:li>255, 255</rdf:li>
 </rdf:Seq>
</crs:ToneCurvePV2012>
<crs:ToneCurvePV2012Red>
 <rdf:Seq>
  <rdf:li>0, 0</rdf:li>
  <rdf:li>255, 255</rdf:li>
 </rdf:Seq>
</crs:ToneCurvePV2012Red>
<crs:ToneCurvePV2012Green>
 <rdf:Seq>
  <rdf:li>0, 0</rdf:li>
  <rdf:li>128, 140</rdf:li>
  <rdf:li>255, 255</rdf:li>
 </rdf:Seq>
</crs:ToneCurvePV2012Green>
<crs:ToneCurvePV2012Blue>
 <rdf:Seq>
  <rdf:li>0, 0</rdf:li>
  <rdf:li>32, 20</rdf:li>
  <rdf:li>224, 240</rdf:li>
  <rdf:li>255, 255</rdf:li>
 </rdf:Seq>
</crs:ToneCurvePV2012Blue>
<crs:Preset>
 <rdf:Description crs:Exposure2012="2.0" crs:Whites2012="+90"/>
</crs:Preset>
"#,
    );
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);

    let preset = import.preset;
    assert_eq!(preset.name(), "Synthetic Look");
    assert_eq!(preset.group(), "Test Group");
    for group in AdjustmentGroup::ALL {
        assert!(
            preset.selection().adjustment_groups.contains(group),
            "{group:?} should be selected"
        );
    }

    let exposure = &preset.edits().exposure;
    assert_eq!(exposure.white_balance, WhiteBalance::AsShot);
    // The nested crs:Preset in the sidecar belongs to another preset.
    assert_close(exposure.exposure, 0.25);
    assert_close(exposure.whites, 18.0);
    assert_close(exposure.blacks, -22.0);
    assert_close(exposure.texture, 7.0);
    assert_close(exposure.clarity, 11.0);
    assert_close(exposure.dehaze, -3.0);
    assert_close(exposure.vibrance, 14.0);
    assert_close(exposure.saturation, -2.0);
    assert_close(exposure.hsl_saturation[1], -12.0);
    assert_close(exposure.hsl_luminance[1], 8.0);

    assert_eq!(exposure.tone_curve.len, 4);
    assert_close_point(exposure.tone_curve.points[0], [0.0, 10.0 / 255.0]);
    assert_close_point(exposure.tone_curve.points[1], [64.0 / 255.0, 80.0 / 255.0]);
    assert_close_point(
        exposure.tone_curve.points[2],
        [192.0 / 255.0, 210.0 / 255.0],
    );
    assert_close_point(exposure.tone_curve.points[3], [1.0, 1.0]);
    assert_eq!(exposure.tone_curve_red.len, 2);
    assert_eq!(exposure.tone_curve_green.len, 3);
    assert_eq!(exposure.tone_curve_blue.len, 4);
    assert_close_point(
        exposure.tone_curve_blue.points[1],
        [32.0 / 255.0, 20.0 / 255.0],
    );

    let decoded = Preset::decode(&preset.encode().unwrap()).expect("the preset round-trips");
    assert_eq!(decoded, preset);
}

#[test]
fn reads_the_settings_description_inside_parameters() {
    let import = import(
        "",
        r#"
<crs:Name>
 <rdf:Alt><rdf:li xml:lang="x-default">Studio</rdf:li></rdf:Alt>
</crs:Name>
<crs:Group>
 <rdf:Alt><rdf:li xml:lang="x-default">Portraits</rdf:li></rdf:Alt>
</crs:Group>
<crs:Parameters>
 <rdf:Description
  crs:ProcessVersion="15.0"
  crs:WhiteBalance="Daylight"
  crs:Temperature="5500"
  crs:Tint="+5"
  crs:Exposure2012="0.5"
  crs:Contrast2012="10"
  crs:Vibrance="20"
  crs:Sharpness="40"
  crs:SharpenRadius="1.2"
  crs:SharpenDetail="30"
  crs:SharpenEdgeMasking="25"
  crs:LuminanceSmoothing="20"
  crs:ColorNoiseReduction="50"
  crs:LuminanceNoiseReductionDetail="60"
  crs:Texture="10"
  crs:Clarity2012="20"
  crs:Dehaze="30"
  crs:GrainAmount="30"
  crs:GrainSize="25"
  crs:GrainFrequency="70"
  crs:PostCropVignetteAmount="-40"
  crs:PostCropVignetteMidpoint="60"
  crs:PostCropVignetteFeather="80"
  crs:PostCropVignetteRoundness="20"
  crs:PostCropVignetteHighlightContrast="40">
 </rdf:Description>
</crs:Parameters>
"#,
    );
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);

    let preset = import.preset;
    assert_eq!(preset.name(), "Studio");
    assert_eq!(preset.group(), "Portraits");
    for group in [
        AdjustmentGroup::Light,
        AdjustmentGroup::Color,
        AdjustmentGroup::Detail,
        AdjustmentGroup::Effects,
    ] {
        assert!(
            preset.selection().adjustment_groups.contains(group),
            "{group:?} should be selected"
        );
    }
    for group in [
        AdjustmentGroup::ToneCurve,
        AdjustmentGroup::ColorMixer,
        AdjustmentGroup::ColorGrading,
    ] {
        assert!(
            !preset.selection().adjustment_groups.contains(group),
            "{group:?} should not be selected"
        );
    }

    let exposure = &preset.edits().exposure;
    assert_eq!(
        exposure.white_balance,
        WhiteBalance::Custom {
            kelvin: 5500.0,
            tint: 5.0,
            older_reader_offsets: [0.0, 0.0],
        }
    );
    assert_close(exposure.exposure, 0.5);
    assert_close(exposure.contrast, 10.0);
    assert_close(exposure.vibrance, 20.0);
    assert_close(exposure.sharpen_amount, 40.0);
    assert_close(exposure.sharpen_radius, 1.2);
    assert_close(exposure.sharpen_detail, 30.0);
    assert_close(exposure.sharpen_masking, 25.0);
    assert_close(exposure.luminance_denoise, 20.0);
    assert_close(exposure.chroma_denoise, 0.5);
    assert_close(exposure.denoise_detail, 60.0);
    assert_close(exposure.texture, 10.0);
    assert_close(exposure.clarity, 20.0);
    assert_close(exposure.dehaze, 30.0);

    let effects = &preset.edits().masks.global_effects;
    let grain = effects
        .iter()
        .find(|effect| effect.effect == MaskEffect::Grain)
        .expect("grain is imported");
    assert_close(grain.settings.grain.amount, 30.0);
    assert_close(grain.settings.grain.size, 1.0);
    assert_close(grain.settings.grain.roughness, 70.0);

    let vignette = effects
        .iter()
        .find(|effect| effect.effect == MaskEffect::Vignette)
        .expect("the post-crop vignette is imported");
    assert_close(vignette.settings.vignette.amount, -40.0);
    assert_close(vignette.settings.vignette.midpoint, 60.0);
    assert_close(vignette.settings.vignette.feather, 80.0);
    assert_close(vignette.settings.vignette.roundness, 20.0);
    assert_close(vignette.settings.vignette.highlights, 40.0);
    assert_eq!(vignette.settings.vignette.center, [50.0, 50.0]);
}

#[test]
fn maps_legacy_lightroom_keys_and_curves() {
    let import = import(
        r#"crs:Exposure="0.75" crs:Contrast="12" crs:Clarity="8" crs:ProcessVersion="10.0""#,
        r#"
<crs:ToneCurve>
 <rdf:Seq>
  <rdf:li>0, 10</rdf:li>
  <rdf:li>128, 130</rdf:li>
  <rdf:li>255, 250</rdf:li>
 </rdf:Seq>
</crs:ToneCurve>
"#,
    );

    let preset = import.preset;
    let exposure = &preset.edits().exposure;
    assert_close(exposure.exposure, 0.75);
    assert_close(exposure.contrast, 12.0);
    assert_close(exposure.clarity, 8.0);
    assert_eq!(exposure.tone_curve.len, 3);
    assert_close_point(
        exposure.tone_curve.points[1],
        [128.0 / 255.0, 130.0 / 255.0],
    );
    assert!(preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ToneCurve));
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("Process version 10")));
}

#[test]
fn mixes_modern_and_legacy_curves_per_channel() {
    let import = import(
        r#"crs:Exposure2012="0""#,
        r#"
<crs:ToneCurvePV2012>
 <rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 250</rdf:li></rdf:Seq>
</crs:ToneCurvePV2012>
<crs:ToneCurveRed>
 <rdf:Seq><rdf:li>0, 10</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq>
</crs:ToneCurveRed>
"#,
    );

    let exposure = &import.preset.edits().exposure;
    assert_eq!(exposure.tone_curve.len, 2);
    assert_close_point(exposure.tone_curve.points[1], [1.0, 250.0 / 255.0]);
    assert_eq!(exposure.tone_curve_red.len, 2);
    assert_close_point(exposure.tone_curve_red.points[0], [0.0, 10.0 / 255.0]);
}

#[test]
fn resamples_curves_longer_than_the_point_limit() {
    let points: String = (0..20)
        .map(|index| {
            let value = index * 13;
            format!("<rdf:li>{value}, {value}</rdf:li>")
        })
        .collect();
    let import = import(
        r#"crs:Exposure2012="0""#,
        &format!("<crs:ToneCurvePV2012><rdf:Seq>{points}</rdf:Seq></crs:ToneCurvePV2012>"),
    );

    let curve = &import.preset.edits().exposure.tone_curve;
    assert_eq!(curve.len as usize, MAX_POINT_CURVE_POINTS);
    assert_close_point(curve_points(curve)[0], [0.0, 0.0]);
    assert_close_point(
        *curve_points(curve).last().unwrap(),
        [247.0 / 255.0, 247.0 / 255.0],
    );
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("reduced to 16")));
}

#[test]
fn warns_about_settings_that_cannot_be_imported() {
    let import = import(
        r#"crs:Exposure2012="0"
           crs:LensProfileEnable="1"
           crs:PerspectiveVertical="10"
           crs:DefringePurpleAmount="5"
           crs:ParametricShadows="10"
           crs:ProcessVersion="10.0"
           crs:CameraProfile="Adobe Standard"
           crs:RedHue="5"
           crs:ColorNoiseReductionSmoothness="70"
           crs:ColorNoiseReductionDetail="70"
           crs:PostCropVignetteStyle="2"
           crs:HasCrop="True""#,
        "",
    );

    for expected in [
        "Lens corrections were not imported.",
        "Perspective corrections were not imported.",
        "Defringing was not imported.",
        "The parametric tone curve was not imported.",
        "Process version 10 is older than 11",
        "Camera profile “Adobe Standard” was not imported.",
        "Camera calibration color adjustments were not imported.",
        "Some noise reduction detail settings were not imported.",
        "The post-crop vignette style was not imported.",
        "Crop settings were not imported.",
    ] {
        assert!(
            import
                .warnings
                .iter()
                .any(|warning| warning.contains(expected)),
            "missing warning {expected:?} in {:?}",
            import.warnings
        );
    }
}

#[test]
fn ignores_a_lens_profile_name_while_the_profile_is_disabled() {
    let disabled = import(
        r#"crs:Exposure2012="0"
           crs:LensProfileEnable="0"
           crs:LensProfileName="Adobe (Canon EF 50mm f/1.8 STM)""#,
        "",
    );
    assert!(!disabled
        .warnings
        .iter()
        .any(|warning| warning.contains("Lens corrections")));

    let name_without_flag = import(
        r#"crs:Exposure2012="0"
           crs:LensProfileName="Adobe (Canon EF 50mm f/1.8 STM)""#,
        "",
    );
    assert!(name_without_flag
        .warnings
        .iter()
        .any(|warning| warning.contains("Lens corrections")));
}

#[test]
fn uses_lightroom_vignette_defaults_for_omitted_sliders() {
    let import = import(r#"crs:PostCropVignetteAmount="-30""#, "");
    let vignette = import
        .preset
        .edits()
        .masks
        .global_effects
        .iter()
        .find(|effect| effect.effect == MaskEffect::Vignette)
        .expect("the post-crop vignette is imported");
    assert_close(vignette.settings.vignette.amount, -30.0);
    assert_close(vignette.settings.vignette.midpoint, 50.0);
    assert_close(vignette.settings.vignette.feather, 50.0);
    assert_close(vignette.settings.vignette.roundness, 0.0);
    assert_close(vignette.settings.vignette.highlights, 0.0);
    assert_eq!(vignette.settings.vignette.center, [50.0, 50.0]);
}

#[test]
fn accepts_the_highlight_priority_vignette_style_only() {
    let highlight_priority = import(r#"crs:Exposure2012="0" crs:PostCropVignetteStyle="0""#, "");
    assert!(!highlight_priority
        .warnings
        .iter()
        .any(|warning| warning.contains("vignette style")));

    for style in ["1", "2"] {
        let other = import(
            &format!(r#"crs:Exposure2012="0" crs:PostCropVignetteStyle="{style}""#),
            "",
        );
        assert!(
            other
                .warnings
                .iter()
                .any(|warning| warning.contains("vignette style")),
            "style {style} should warn"
        );
    }
}

#[test]
fn decodes_entities_in_names() {
    let import = import(
        r#"crs:Exposure2012="0""#,
        r#"
<crs:Name>
 <rdf:Alt><rdf:li xml:lang="x-default">Warm &amp; Bright &#233;</rdf:li></rdf:Alt>
</crs:Name>
"#,
    );
    assert_eq!(import.preset.name(), "Warm & Bright é");
}

#[test]
fn falls_back_to_the_file_name() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("My Look.xmp");
    std::fs::write(&path, document(r#"crs:Exposure2012="0.25""#, "").as_bytes()).unwrap();

    let import = read_lightroom_preset_file(&path).expect("the file imports");
    assert_eq!(import.preset.name(), "My Look");
    assert_eq!(import.preset.group(), DEFAULT_PRESET_GROUP);
    assert_close(import.preset.edits().exposure.exposure, 0.25);
}

#[test]
fn accepts_an_xml_declaration() {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
        document(r#"crs:Exposure2012="0.25""#, "")
    );
    let import = import_lightroom_xmp(xml.as_bytes()).expect("the declaration is accepted");
    assert_close(import.preset.edits().exposure.exposure, 0.25);
}

#[test]
fn reads_a_self_closing_description() {
    let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Name="Self closing" crs:Exposure2012="1.0" />
</rdf:RDF>
</x:xmpmeta>"#;
    let import = import_lightroom_xmp(xml.as_bytes()).expect("the file imports");
    assert_eq!(import.preset.name(), "Self closing");
    assert_close(import.preset.edits().exposure.exposure, 1.0);
}

#[test]
fn rejects_files_without_importable_settings() {
    let error = import_lightroom_xmp(document(r#"crs:LensProfileEnable="1""#, "").as_bytes())
        .expect_err("there is nothing to import");
    assert!(matches!(error, PresetError::Unsupported(_)));
    let message = error.to_string();
    assert!(message.contains("no Lightroom settings"));
    assert!(message.contains("Lens corrections were not imported."));
}

#[test]
fn rejects_invalid_and_utf16_documents() {
    assert!(matches!(
        import_lightroom_xmp(b"not xml"),
        Err(PresetError::Invalid(_))
    ));
    assert!(matches!(
        import_lightroom_xmp(&[0xFF, 0xFE, 0x00, 0x00]),
        Err(PresetError::Unsupported(_))
    ));
}

#[test]
fn maps_white_balance_variants() {
    let as_shot = import(r#"crs:WhiteBalance="As Shot" crs:Temperature="7000""#, "");
    assert_eq!(
        as_shot.preset.edits().exposure.white_balance,
        WhiteBalance::AsShot
    );

    let tungsten = import(r#"crs:WhiteBalance="Tungsten""#, "");
    assert_eq!(
        tungsten.preset.edits().exposure.white_balance,
        WhiteBalance::Custom {
            kelvin: 2850.0,
            tint: 0.0,
            older_reader_offsets: [0.0, 0.0],
        }
    );

    let custom = import(
        r#"crs:WhiteBalance="Custom" crs:Temperature="7000" crs:Tint="-20""#,
        "",
    );
    assert_eq!(
        custom.preset.edits().exposure.white_balance,
        WhiteBalance::Custom {
            kelvin: 7000.0,
            tint: -20.0,
            older_reader_offsets: [0.0, 0.0],
        }
    );

    let temperature_only = import(r#"crs:Temperature="5000" crs:Tint="5""#, "");
    assert_eq!(
        temperature_only.preset.edits().exposure.white_balance,
        WhiteBalance::Custom {
            kelvin: 5000.0,
            tint: 5.0,
            older_reader_offsets: [0.0, 0.0],
        }
    );

    let unknown = import(r#"crs:WhiteBalance="Custom""#, "");
    assert_eq!(
        unknown.preset.edits().exposure.white_balance,
        WhiteBalance::AsShot
    );
    assert!(unknown
        .warnings
        .iter()
        .any(|warning| warning.contains("custom white balance")));
}

#[test]
fn maps_color_mixer_and_color_grading() {
    let import = import(
        r#"crs:HueAdjustmentRed="50"
           crs:SaturationAdjustmentOrange="-15"
           crs:LuminanceAdjustmentOrange="9"
           crs:SplitToningShadowHue="30"
           crs:SplitToningShadowSaturation="40"
           crs:SplitToningBalance="10"
           crs:ColorGradeMidtoneHue="120"
           crs:ColorGradeMidtoneSat="30"
           crs:ColorGradeMidtoneLum="5"
           crs:ColorGradeHighlightHue="300"
           crs:ColorGradeHighlightSat="20"
           crs:ColorGradeGlobalHue="200"
           crs:ColorGradeGlobalSat="10"
           crs:ColorGradeGlobalLum="-5"
           crs:ColorGradeBlending="70""#,
        "",
    );

    let exposure = &import.preset.edits().exposure;
    assert_close(exposure.hsl_hue[0], 100.0);
    assert_close(exposure.hsl_saturation[1], -15.0);
    assert_close(exposure.hsl_luminance[1], 9.0);

    let grading = exposure.color_grading;
    assert_close(grading.shadows.hue, 30.0);
    assert_close(grading.shadows.saturation, 40.0);
    assert_close(grading.midtones.hue, 120.0);
    assert_close(grading.midtones.saturation, 30.0);
    assert_close(grading.midtones.luminance, 5.0);
    assert_close(grading.highlights.hue, 300.0);
    assert_close(grading.highlights.saturation, 20.0);
    assert_close(grading.global.hue, 200.0);
    assert_close(grading.global.saturation, 10.0);
    assert_close(grading.global.luminance, -5.0);
    assert_close(grading.blending, 70.0);
    assert_close(grading.balance, 10.0);

    assert!(import
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ColorMixer));
    assert!(import
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ColorGrading));
}

#[test]
fn legacy_split_toning_uses_full_blending() {
    let import = import(
        r#"crs:SplitToningShadowHue="30"
           crs:SplitToningShadowSaturation="40"
           crs:SplitToningHighlightHue="200"
           crs:SplitToningHighlightSaturation="20"
           crs:SplitToningBalance="-10""#,
        "",
    );

    let grading = import.preset.edits().exposure.color_grading;
    assert_close(grading.shadows.hue, 30.0);
    assert_close(grading.shadows.saturation, 40.0);
    assert_close(grading.highlights.hue, 200.0);
    assert_close(grading.highlights.saturation, 20.0);
    assert_close(grading.balance, -10.0);
    assert_close(grading.blending, 100.0);
    assert!(import
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ColorGrading));
}

#[test]
fn converts_to_grayscale_and_warns_about_the_mixer() {
    let import = import(r#"crs:ConvertToGrayscale="True" crs:GrayMixerRed="20""#, "");
    assert_close(import.preset.edits().exposure.saturation, -100.0);
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("Grayscale mix")));
}

#[test]
fn clamps_values_to_the_calibraw_ranges() {
    let import = import(
        r#"crs:Exposure2012="9"
           crs:Contrast2012="-200"
           crs:HueAdjustmentRed="200"
           crs:Sharpness="999"
           crs:SharpenRadius="0.1"
           crs:ColorNoiseReduction="150"
           crs:PostCropVignetteAmount="-150""#,
        "",
    );

    let exposure = &import.preset.edits().exposure;
    assert_close(exposure.exposure, 5.0);
    assert_close(exposure.contrast, -100.0);
    assert_close(exposure.hsl_hue[0], 200.0);
    assert_close(exposure.sharpen_amount, 150.0);
    assert_close(exposure.sharpen_radius, 0.5);
    assert_close(exposure.chroma_denoise, 1.0);
    let vignette = import
        .preset
        .edits()
        .masks
        .global_effects
        .iter()
        .find(|effect| effect.effect == MaskEffect::Vignette)
        .expect("the vignette is imported");
    assert_close(vignette.settings.vignette.amount, -100.0);
}

#[test]
fn recognises_xmp_files_without_case() {
    assert!(is_lightroom_preset_file(Path::new("look.xmp")));
    assert!(is_lightroom_preset_file(Path::new("look.XMP")));
    assert!(!is_lightroom_preset_file(Path::new("look.calibraw-preset")));
    assert!(!is_lightroom_preset_file(Path::new("look")));
}
