use super::rgb_table::tests::{table_text, TableSpec};
use super::*;
use crate::pipeline::AdjustmentGroup;
use crate::sidecar::default_edit_state;
use std::path::PathBuf;

fn xmp(attributes: &str, children: &str) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
   xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:SupportsAmount="False"
   crs:ProcessVersion="15.4"
   {attributes}>
   {children}
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
}

fn import(attributes: &str, children: &str) -> Result<LightroomPresetImport, PresetError> {
    LightroomPresetImport::from_xmp(xmp(attributes, children).as_bytes(), "fallback")
}

const NAME_AND_GROUP: &str = r#"
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Warm &amp; Soft</rdf:li></rdf:Alt></crs:Name>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Film Looks</rdf:li></rdf:Alt></crs:Group>"#;

#[test]
fn converts_sliders_with_matching_ranges() {
    let result = import(
        r#"crs:Exposure2012="+0.50" crs:Contrast2012="-25" crs:Highlights2012="-60"
           crs:Shadows2012="+40" crs:Whites2012="+5" crs:Blacks2012="-12"
           crs:Vibrance="+30" crs:Saturation="-10"
           crs:Texture="+15" crs:Clarity2012="+20" crs:Dehaze="-5" crs:GrainAmount="30"
           crs:Sharpness="60" crs:SharpenRadius="1.2" crs:SharpenDetail="30"
           crs:SharpenEdgeMasking="20" crs:LuminanceSmoothing="15"
           crs:ColorNoiseReduction="25" crs:LuminanceNoiseReductionDetail="40"
           crs:PostCropVignetteAmount="-30" crs:PostCropVignetteMidpoint="45"
           crs:PostCropVignetteRoundness="10" crs:PostCropVignetteFeather="60"
           crs:PostCropVignetteHighlightContrast="20""#,
        NAME_AND_GROUP,
    )
    .unwrap();
    let exposure = result.preset.edits().exposure;
    assert_eq!(exposure.exposure, 0.5);
    assert_eq!(exposure.contrast, -25.0);
    assert_eq!(exposure.highlights, -60.0);
    assert_eq!(exposure.shadows, 40.0);
    assert_eq!(exposure.whites, 5.0);
    assert_eq!(exposure.blacks, -12.0);
    assert_eq!(exposure.vibrance, 30.0);
    assert_eq!(exposure.saturation, -10.0);
    assert_eq!(exposure.texture, 15.0);
    assert_eq!(exposure.clarity, 20.0);
    assert_eq!(exposure.dehaze, -5.0);
    assert_eq!(exposure.grain_amount, 30.0);
    assert_eq!(exposure.sharpen_amount, 60.0);
    assert_eq!(exposure.sharpen_radius, 1.2);
    assert_eq!(exposure.sharpen_detail, 30.0);
    assert_eq!(exposure.sharpen_masking, 20.0);
    assert_eq!(exposure.luminance_denoise, 15.0);
    // Colour noise reduction is a fraction in CalibRaw.
    assert_eq!(exposure.chroma_denoise, 0.25);
    assert_eq!(exposure.denoise_detail, 40.0);
    assert_eq!(exposure.vignette_amount, -30.0);
    assert_eq!(exposure.vignette_midpoint, 45.0);
    assert_eq!(exposure.vignette_roundness, 10.0);
    assert_eq!(exposure.vignette_feather, 60.0);
    assert_eq!(exposure.vignette_highlights, 20.0);
    assert_eq!(result.imported_settings, 24);
    assert!(result.skipped.is_empty());
}

#[test]
fn reads_name_and_group_with_entities() {
    let result = import(r#"crs:Exposure2012="+1.0""#, NAME_AND_GROUP).unwrap();
    assert_eq!(result.preset.name(), "Warm & Soft");
    assert_eq!(result.preset.group(), "Film Looks");
}

#[test]
fn unnamed_presets_use_the_file_name_and_the_lightroom_group() {
    let result = import(r#"crs:Exposure2012="+1.0""#, "").unwrap();
    assert_eq!(result.preset.name(), "fallback");
    assert_eq!(result.preset.group(), "Lightroom");
}

#[test]
fn includes_only_the_cards_the_file_touches() {
    let result = import(
        r#"crs:Exposure2012="0" crs:Clarity2012="+10" crs:HueAdjustmentBlue="-20""#,
        "",
    )
    .unwrap();
    let selection = result.preset.selection();
    assert!(selection.adjustment_groups.contains(AdjustmentGroup::Light));
    assert!(selection
        .adjustment_groups
        .contains(AdjustmentGroup::Effects));
    assert!(selection
        .adjustment_groups
        .contains(AdjustmentGroup::ColorMixer));
    assert!(!selection.adjustment_groups.contains(AdjustmentGroup::Color));
    assert!(!selection
        .adjustment_groups
        .contains(AdjustmentGroup::Detail));
    assert!(!selection.geometry && !selection.masks && !selection.lens_correction);
}

#[test]
fn converts_the_colour_mixer_in_band_order() {
    let result = import(
        r#"crs:HueAdjustmentRed="+5" crs:SaturationAdjustmentOrange="-15"
           crs:LuminanceAdjustmentMagenta="+25" crs:HueAdjustmentAqua="+100""#,
        "",
    )
    .unwrap();
    let exposure = result.preset.edits().exposure;
    assert_eq!(exposure.hsl_hue[0], 5.0);
    assert_eq!(exposure.hsl_saturation[1], -15.0);
    assert_eq!(exposure.hsl_luminance[7], 25.0);
    assert_eq!(exposure.hsl_hue[4], 100.0);
}

#[test]
fn converts_colour_grading_wheels() {
    let result = import(
        r#"crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="30"
           crs:SplitToningHighlightHue="40" crs:SplitToningHighlightSaturation="20"
           crs:SplitToningBalance="-10" crs:ColorGradeMidtoneHue="120"
           crs:ColorGradeMidtoneSat="10" crs:ColorGradeShadowLum="-5"
           crs:ColorGradeGlobalSat="8" crs:ColorGradeBlending="70""#,
        "",
    )
    .unwrap();
    let grading = result.preset.edits().exposure.color_grading;
    assert_eq!(grading.shadows.hue, 210.0);
    assert_eq!(grading.shadows.saturation, 30.0);
    assert_eq!(grading.shadows.luminance, -5.0);
    assert_eq!(grading.highlights.hue, 40.0);
    assert_eq!(grading.highlights.saturation, 20.0);
    assert_eq!(grading.midtones.hue, 120.0);
    assert_eq!(grading.midtones.saturation, 10.0);
    assert_eq!(grading.global.saturation, 8.0);
    assert_eq!(grading.balance, -10.0);
    assert_eq!(grading.blending, 70.0);
}

#[test]
fn clamps_out_of_range_values() {
    let result = import(
        r#"crs:Exposure2012="+9" crs:Contrast2012="-300" crs:GrainAmount="-4""#,
        "",
    )
    .unwrap();
    let exposure = result.preset.edits().exposure;
    assert_eq!(exposure.exposure, 5.0);
    assert_eq!(exposure.contrast, -100.0);
    assert_eq!(exposure.grain_amount, 0.0);
}

#[test]
fn converts_tone_curves_to_the_unit_square() {
    let result = import(
        "",
        r#"<crs:ToneCurvePV2012><rdf:Seq>
             <rdf:li>0, 0</rdf:li><rdf:li>64, 51</rdf:li><rdf:li>255, 255</rdf:li>
           </rdf:Seq></crs:ToneCurvePV2012>
           <crs:ToneCurvePV2012Red><rdf:Seq>
             <rdf:li>0, 10</rdf:li><rdf:li>255, 245</rdf:li>
           </rdf:Seq></crs:ToneCurvePV2012Red>"#,
    )
    .unwrap();
    let edits = result.preset.edits();
    let curve = &edits.exposure.tone_curve;
    assert_eq!(curve.len, 3);
    assert!((curve.points[1][0] - 64.0 / 255.0).abs() < 1e-6);
    assert!((curve.points[1][1] - 0.2).abs() < 1e-6);
    assert_eq!(curve.points[2], [1.0, 1.0]);
    assert_eq!(edits.exposure.tone_curve_red.len, 2);
    assert!(edits.exposure.tone_curve_green.is_identity());
    assert!(result
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ToneCurve));
}

#[test]
fn curves_without_endpoints_are_closed_and_long_curves_simplified() {
    let points: String = (1..=20)
        .map(|index| format!("<rdf:li>{}, {}</rdf:li>", index * 12, index * 12))
        .collect();
    let result = import(
        "",
        &format!("<crs:ToneCurvePV2012><rdf:Seq>{points}</rdf:Seq></crs:ToneCurvePV2012>"),
    )
    .unwrap();
    let curve = &result.preset.edits().exposure.tone_curve;
    assert_eq!(curve.len as usize, MAX_POINT_CURVE_POINTS);
    assert_eq!(curve.points[0][0], 0.0);
    assert_eq!(curve.points[MAX_POINT_CURVE_POINTS - 1][0], 1.0);
    assert!(result
        .skipped
        .iter()
        .any(|note| note.contains("simplified")));
}

#[test]
fn reports_what_it_cannot_import() {
    let result = import(
        r#"crs:Exposure2012="+0.3" crs:Temperature="5200" crs:Tint="+8"
           crs:CameraProfile="Camera Portrait" crs:ConvertToGrayscale="True"
           crs:LensProfileEnable="1" crs:HasCrop="True" crs:ParametricShadows="10""#,
        "",
    )
    .unwrap();
    assert_eq!(
        result.skipped,
        [
            "white balance",
            "camera profile",
            "black and white conversion",
            "parametric tone curve",
            "lens corrections",
            "crop and geometry",
        ]
    );
    // White balance is never converted.
    assert_eq!(result.preset.edits().exposure.temperature, 0.0);
    assert_eq!(result.preset.edits().exposure.tint, 0.0);
}

#[test]
fn default_valued_flags_are_not_reported() {
    let result = import(
        r#"crs:Exposure2012="+0.3" crs:WhiteBalance="As Shot" crs:CameraProfile="Adobe Standard"
           crs:HasCrop="False" crs:ConvertToGrayscale="False" crs:ParametricShadows="0""#,
        "",
    )
    .unwrap();
    assert!(result.skipped.is_empty());
}

#[test]
fn nested_mask_settings_do_not_leak_into_the_global_settings() {
    let result = import(
        r#"crs:Exposure2012="+0.3""#,
        r#"<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li>
             <rdf:Description crs:Exposure2012="+4.00" crs:LocalExposure2012="+1.0">
               <crs:CorrectionMasks><rdf:Seq><rdf:li>
                 <rdf:Description crs:What="Mask/CircularGradient"/>
               </rdf:li></rdf:Seq></crs:CorrectionMasks>
             </rdf:Description>
           </rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>
           <crs:Contrast2012>+10</crs:Contrast2012>"#,
    )
    .unwrap();
    let exposure = result.preset.edits().exposure;
    assert_eq!(exposure.exposure, 0.3);
    // A simple element works like an attribute and is read after the mask.
    assert_eq!(exposure.contrast, 10.0);
    assert_eq!(result.skipped, ["local adjustments and masks"]);
}

#[test]
fn presets_with_only_unsupported_settings_explain_why() {
    let error = import(
        r#"crs:CameraProfile="Camera Vivid""#,
        r#"<crs:RGBTable>AAAA</crs:RGBTable>"#,
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(matches!(error, PresetError::Unsupported(_)));
    assert!(message.contains("camera profile"), "{message}");
    assert!(message.contains("lookup table"), "{message}");
}

#[test]
fn rejects_files_that_are_not_lightroom_presets() {
    for bytes in [
        &b"{\"format\":\"CalibRaw preset\"}"[..],
        b"<html><body>hello</body></html>",
        b"<rdf:RDF xmlns:rdf=\"x\"><rdf:Description/></rdf:RDF>",
        b"\xff\xfe\x00",
        b"",
    ] {
        assert!(
            LightroomPresetImport::from_xmp(bytes, "x").is_err(),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn rejects_deeply_nested_documents() {
    let nested = format!("{}{}", "<a>".repeat(200), "</a>".repeat(200));
    assert!(LightroomPresetImport::from_xmp(nested.as_bytes(), "x").is_err());
}

#[test]
fn imported_presets_apply_like_any_other() {
    let result = import(
        r#"crs:Exposure2012="+0.80" crs:Contrast2012="+20" crs:Texture="+10""#,
        "",
    )
    .unwrap();
    let mut photo = default_edit_state();
    photo.exposure.shadows = 33.0;
    result.preset.apply_to(&mut photo);
    assert_eq!(photo.exposure.exposure, 0.8);
    assert_eq!(photo.exposure.contrast, 20.0);
    assert_eq!(photo.exposure.texture, 10.0);
    // Cards the preset does not touch keep their values.
    assert_eq!(photo.exposure.shadows, 0.0);
    assert_eq!(photo.exposure.saturation, 0.0);
}

#[test]
fn imported_presets_survive_saving_and_loading() {
    let result = import(r#"crs:Exposure2012="+0.80""#, NAME_AND_GROUP).unwrap();
    let reloaded = Preset::decode(&result.preset.encode().unwrap()).unwrap();
    assert_eq!(reloaded, result.preset);
}

#[test]
fn recognizes_xmp_files_by_extension() {
    assert!(is_lightroom_preset_file(&PathBuf::from("Looks/Warm.xmp")));
    assert!(is_lightroom_preset_file(&PathBuf::from("Warm.XMP")));
    assert!(!is_lightroom_preset_file(&PathBuf::from(".hidden.xmp")));
    assert!(!is_lightroom_preset_file(&PathBuf::from(
        "warm.calibraw-preset"
    )));
    assert!(!is_lightroom_preset_file(&PathBuf::from(".xmp")));
}

#[test]
fn reads_a_preset_file_named_after_its_file() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("Golden Hour.xmp");
    std::fs::write(&path, xmp(r#"crs:Exposure2012="+0.2""#, "")).unwrap();
    let result = read_lightroom_preset_file(&path).unwrap();
    assert_eq!(result.preset.name(), "Golden Hour");
    assert_eq!(result.preset.edits().exposure.exposure, 0.2);
}

/// A look that halves every channel of an sRGB table, as Adobe stores it.
fn dimming_table() -> String {
    table_text(&TableSpec {
        divisions: 9,
        primaries: 0,
        gamma: 1,
        output: |rgb| rgb.map(|value| value * 0.5),
    })
}

#[test]
fn imports_the_lookup_table_of_a_profile() {
    let table = dimming_table();
    let result = import(
        &format!(r#"crs:Highlights2012="-10" crs:RGBTable="ABC123" crs:Table_ABC123="{table}""#),
        NAME_AND_GROUP,
    )
    .unwrap();
    assert!(result.skipped.is_empty(), "{:?}", result.skipped);
    let look = result
        .preset
        .edits()
        .color_lut
        .as_ref()
        .expect("the table becomes a look");
    assert_eq!(look.name, "Warm & Soft");
    assert_eq!(look.amount, 100.0);
    let [r, g, b] = look.lut.sample([0.8, 0.8, 0.8]);
    assert!(
        [r, g, b].iter().all(|value| (value - 0.4).abs() < 0.02),
        "{r} {g} {b}"
    );
    let selection = result.preset.selection();
    assert!(selection.color_lut);
    assert!(selection.adjustment_groups.contains(AdjustmentGroup::Light));
    // The slider settings come across with it.
    assert_eq!(result.preset.edits().exposure.highlights, -10.0);
    assert_eq!(result.imported_settings, 2);
}

#[test]
fn a_look_only_profile_imports_with_just_the_table() {
    let table = dimming_table();
    let result = import(
        &format!(r#"crs:RGBTable="ABC123" crs:Table_ABC123="{table}""#),
        "",
    )
    .unwrap();
    assert!(result.preset.selection().adjustment_groups.is_empty());
    assert!(result.preset.selection().color_lut);
    let mut photo = default_edit_state();
    result.preset.apply_to(&mut photo);
    assert!(photo.color_lut.is_some());
}

#[test]
fn reads_a_table_nested_in_a_look_and_names_its_profile() {
    let table = dimming_table();
    let nested = format!(
        r#"<crs:Look><rdf:Description crs:Name="Film Stock" crs:RGBTable="ABC123" crs:Table_ABC123="{table}"/></crs:Look>"#
    );
    let result = import(r#"crs:Exposure2012="0""#, &nested).unwrap();
    assert!(result.preset.edits().color_lut.is_some());
    assert!(result.skipped.is_empty(), "{:?}", result.skipped);
}

#[test]
fn adobes_built_in_profiles_are_reported_unless_they_are_the_default() {
    let named = |name: &str| {
        import(
            r#"crs:Exposure2012="0""#,
            &format!(r#"<crs:Look><rdf:Description crs:Name="{name}"/></crs:Look>"#),
        )
        .unwrap()
        .skipped
    };
    assert!(named("Adobe Color").is_empty());
    assert_eq!(named("Adobe Vivid"), ["Adobe profile “Adobe Vivid”"]);
}

#[test]
fn an_unreadable_table_is_reported_and_the_sliders_still_import() {
    let missing = import(r#"crs:Exposure2012="+1" crs:RGBTable="NOPE""#, "").unwrap();
    assert_eq!(
        missing.skipped,
        ["colour lookup table (its data is missing)"]
    );
    assert!(missing.preset.edits().color_lut.is_none());
    assert!(!missing.preset.selection().color_lut);

    let damaged = import(
        r#"crs:Exposure2012="+1" crs:RGBTable="ABC123" crs:Table_ABC123="not a table at all""#,
        "",
    )
    .unwrap();
    assert!(
        damaged.skipped[0].starts_with("colour lookup table ("),
        "{:?}",
        damaged.skipped
    );
    assert_eq!(damaged.preset.edits().exposure.exposure, 1.0);
}
