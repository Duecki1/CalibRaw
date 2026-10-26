use super::*;
use crate::pipeline::{
    luminance_range_weight, AdjustmentGroup, MaskCombineMode, MaskEffect, MaskGeometry, MaskKind,
    PointCurve, WhiteBalance, MAX_POINT_CURVE_POINTS,
};
use crate::presets::{Preset, PresetError};
use crate::sidecar::default_edit_state;

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

fn import(attributes: &str, body: &str) -> XmpImport {
    import_xmp(document(attributes, body).as_bytes()).expect("the XMP preset imports")
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

fn mask_group(corrections: &[String]) -> String {
    format!(
        "<crs:MaskGroupBasedCorrections><rdf:Seq>{}</rdf:Seq></crs:MaskGroupBasedCorrections>",
        corrections.concat()
    )
}

fn correction(name: &str, attributes: &str, components: &[String], curves: &str) -> String {
    format!(
        r#"<rdf:li><rdf:Description crs:What="Correction" crs:CorrectionName="{name}" {attributes}>
{curves}
<crs:CorrectionMasks><rdf:Seq>{}</rdf:Seq></crs:CorrectionMasks>
</rdf:Description></rdf:li>"#,
        components.concat()
    )
}

fn component(name: &str, subtype: &str, attributes: &str) -> String {
    format!(
        r#"<rdf:li crs:What="Mask/Image" crs:MaskName="{name}" crs:MaskSubType="{subtype}" {attributes}/>"#
    )
}

fn background_component(name: &str, attributes: &str) -> String {
    format!(
        r#"<rdf:li crs:What="Mask/Image" crs:MaskName="{name}" crs:MaskSubType="0" crs:MaskSubCategoryID="22" {attributes}/>"#
    )
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
fn maps_legacy_keys_and_curves() {
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
fn simplifies_curves_longer_than_the_point_limit() {
    // 20 points on a smooth bend: more than CalibRaw stores.
    let points: String = (0..20)
        .map(|index| {
            let x = index as f32 / 19.0;
            format!("<rdf:li>{}, {}</rdf:li>", x * 255.0, x * x * 255.0)
        })
        .collect();
    let import = import(
        r#"crs:Exposure2012="0""#,
        &format!("<crs:ToneCurvePV2012><rdf:Seq>{points}</rdf:Seq></crs:ToneCurvePV2012>"),
    );

    let curve = &import.preset.edits().exposure.tone_curve;
    let kept = curve_points(curve);
    assert!(kept.len() <= MAX_POINT_CURVE_POINTS);
    assert_close_point(kept[0], [0.0, 0.0]);
    assert_close_point(*kept.last().unwrap(), [1.0, 1.0]);
    for [x, y] in kept {
        assert_close(y, x * x);
    }
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("it was simplified to")));
}

#[test]
fn warns_about_settings_that_cannot_be_imported() {
    let import = import(
        r#"crs:Exposure2012="0"
           crs:PerspectiveVertical="10"
           crs:DefringePurpleAmount="5"
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
        "Perspective corrections were not imported.",
        "Defringing was not imported.",
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
fn uses_the_format_vignette_defaults_for_omitted_sliders() {
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
    let highlight_priority = import(r#"crs:Exposure2012="0" crs:PostCropVignetteStyle="1""#, "");
    assert!(!highlight_priority
        .warnings
        .iter()
        .any(|warning| warning.contains("vignette style")));

    for style in ["2", "3"] {
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

    let (import, _) = read_xmp_preset_file(&path).expect("the file imports");
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
    let import = import_xmp(xml.as_bytes()).expect("the declaration is accepted");
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
    let import = import_xmp(xml.as_bytes()).expect("the file imports");
    assert_eq!(import.preset.name(), "Self closing");
    assert_close(import.preset.edits().exposure.exposure, 1.0);
}

#[test]
fn keeps_files_without_supported_settings_as_empty_placeholders() {
    let import = import_xmp(document(r#"crs:PerspectiveVertical="10""#, "").as_bytes())
        .expect("the preset stays listed");
    assert!(import.preset.selection().is_empty());
    assert_eq!(import.preset.edits(), &default_edit_state());
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("No settings in this preset")));
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("Perspective corrections were not imported.")));
}

#[test]
fn rewrites_name_and_group_attributes_and_keeps_other_bytes() {
    let xml = document(
        r#"crs:Name="Old name" crs:Group="Old group" crs:Exposure2012="0.5""#,
        "",
    );
    let rewritten =
        rewrite_xmp_labels(xml.as_bytes(), "New & name", "New group").expect("labels rewrite");
    let text = String::from_utf8(rewritten).unwrap();
    assert_eq!(
        text,
        xml.replace(r#"crs:Name="Old name""#, r#"crs:Name="New &amp; name""#)
            .replace(r#"crs:Group="Old group""#, r#"crs:Group="New group""#)
    );
    let import = import_xmp(text.as_bytes()).expect("the rewritten XMP imports");
    assert_eq!(import.preset.name(), "New & name");
    assert_eq!(import.preset.group(), "New group");
    assert_close(import.preset.edits().exposure.exposure, 0.5);
}

#[test]
fn rewrites_name_and_group_elements_and_ignores_the_embedded_preset() {
    let xml = document(
        r#"crs:Name="Old name" crs:Group="Old group" crs:Exposure2012="0.5""#,
        r#"
<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Old name</rdf:li></rdf:Alt></crs:Name>
<crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Old group</rdf:li></rdf:Alt></crs:Group>
<crs:Preset><rdf:Description crs:Name="Embedded" crs:Group="Embedded group" crs:Exposure2012="1"/></crs:Preset>
"#,
    );
    let rewritten = rewrite_xmp_labels(xml.as_bytes(), "New", "Group").expect("labels rewrite");
    let text = String::from_utf8(rewritten).unwrap();
    assert!(text.contains(
        r#"<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">New</rdf:li></rdf:Alt></crs:Name>"#
    ));
    assert!(text.contains(
        r#"<crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Group</rdf:li></rdf:Alt></crs:Group>"#
    ));
    assert!(text.contains(r#"crs:Name="Embedded""#));
    let import = import_xmp(text.as_bytes()).expect("the rewritten XMP imports");
    assert_eq!(import.preset.name(), "New");
    assert_eq!(import.preset.group(), "Group");
}

#[test]
fn adds_missing_labels_to_the_top_description() {
    let xml = document(r#"crs:Exposure2012="0.5""#, "");
    let rewritten =
        rewrite_xmp_labels(xml.as_bytes(), "Added", "Added group").expect("labels rewrite");
    let import = import_xmp(&rewritten).expect("the rewritten XMP imports");
    assert_eq!(import.preset.name(), "Added");
    assert_eq!(import.preset.group(), "Added group");
    assert_close(import.preset.edits().exposure.exposure, 0.5);
}

#[test]
fn leaves_labels_alone_when_they_already_match() {
    let xml = document(
        r#"crs:Name="Same" crs:Group="Same group" crs:Exposure2012="0.5""#,
        "",
    );
    let rewritten =
        rewrite_xmp_labels(xml.as_bytes(), "Same", "Same group").expect("labels rewrite");
    assert_eq!(rewritten, xml.as_bytes());
}

#[test]
fn preserves_a_utf8_bom() {
    let xml = document(
        r#"crs:Name="Old" crs:Group="Old group" crs:Exposure2012="0.5""#,
        "",
    );
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(xml.as_bytes());

    let rewritten = rewrite_xmp_labels(&bytes, "New", "Group").expect("labels rewrite");
    assert!(rewritten.starts_with(&[0xEF, 0xBB, 0xBF]));
    let text = std::str::from_utf8(&rewritten[3..]).unwrap();
    assert_eq!(
        text,
        xml.replace(r#"crs:Name="Old""#, r#"crs:Name="New""#)
            .replace(r#"crs:Group="Old group""#, r#"crs:Group="Group""#)
    );
}

#[test]
fn adds_the_crs_namespace_even_when_a_comment_mentions_it() {
    let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<!-- the comment mentions xmlns:crs without declaring it -->
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" crs:Exposure2012="0.5"/>
</rdf:RDF>
</x:xmpmeta>"#;
    let rewritten =
        rewrite_xmp_labels(xml.as_bytes(), "Added", "Added group").expect("labels rewrite");
    let text = std::str::from_utf8(&rewritten).unwrap();
    assert!(text.contains(r#"crs:Name="Added""#));
    assert!(text.contains(r#"crs:Group="Added group""#));
    assert!(text.contains(r#"xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/""#));
    let import = import_xmp(&rewritten).expect("the rewritten XMP imports");
    assert_eq!(import.preset.name(), "Added");
    assert_eq!(import.preset.group(), "Added group");
    assert_close(import.preset.edits().exposure.exposure, 0.5);
}

#[test]
fn rewrites_label_elements_in_a_second_description() {
    let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="0.5"/>
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Old</rdf:li></rdf:Alt></crs:Name>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>"#;
    let rewritten = rewrite_xmp_labels(xml.as_bytes(), "New", "Group").expect("labels rewrite");
    let import = import_xmp(&rewritten).expect("the rewritten XMP imports");
    assert_eq!(import.preset.name(), "New");
    assert_eq!(import.preset.group(), "Group");
    assert_close(import.preset.edits().exposure.exposure, 0.5);
}

/// A profile reference as XMP writers store it after the preset's labels.
const LOOK: &str = r#"
<crs:Look>
 <rdf:Description crs:Name="Profile name" crs:Amount="1">
  <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Profiles</rdf:li></rdf:Alt></crs:Group>
  <crs:Parameters>
   <rdf:Description crs:Exposure2012="3" crs:CameraProfile="Profile base">
    <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 40</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
   </rdf:Description>
  </crs:Parameters>
 </rdf:Description>
</crs:Look>
"#;

#[test]
fn ignores_the_profile_labels_and_settings() {
    let import = import(
        r#"crs:Exposure2012="0.5""#,
        &format!(
            r#"<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Warm</rdf:li></rdf:Alt></crs:Name>
<crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Film</rdf:li></rdf:Alt></crs:Group>
{LOOK}"#
        ),
    );
    assert_eq!(import.preset.name(), "Warm");
    assert_eq!(import.preset.group(), "Film");
    assert_close(import.preset.edits().exposure.exposure, 0.5);
    assert!(!import
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ToneCurve));
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
}

#[test]
fn rewriting_labels_leaves_the_profile_untouched() {
    let xml = document(
        r#"crs:Exposure2012="0.5""#,
        &format!(
            r#"<crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Warm</rdf:li></rdf:Alt></crs:Name>
<crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Film</rdf:li></rdf:Alt></crs:Group>
{LOOK}"#
        ),
    );
    let rewritten = rewrite_xmp_labels(xml.as_bytes(), "Hot", "Analog").expect("labels rewrite");
    let text = String::from_utf8(rewritten).unwrap();
    assert!(text.contains(LOOK));
    assert_eq!(
        text,
        xml.replace(">Warm<", ">Hot<").replace(">Film<", ">Analog<")
    );
}

#[test]
fn skips_masks_whose_components_are_written_as_descriptions() {
    let range = r#"<rdf:li><rdf:Description crs:What="Mask/RangeMask" crs:MaskBlendMode="2">
<crs:CorrectionRangeMask crs:Type="1"/>
</rdf:Description></rdf:li>"#;
    let correction = correction(
        "Subject in range",
        "",
        &[component("Subject 1", "1", ""), range.to_owned()],
        "",
    );
    let import = import("", &mask_group(&[correction]));

    assert!(import.preset.edits().masks.masks.is_empty());
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("“Subject in range” was not imported")));
}

#[test]
fn rejects_invalid_and_utf16_documents() {
    assert!(matches!(
        import_xmp(b"not xml"),
        Err(PresetError::Invalid(_))
    ));
    assert!(matches!(
        import_xmp(&[0xFF, 0xFE, 0x00, 0x00]),
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
    assert!(is_xmp_preset_file(Path::new("look.xmp")));
    assert!(is_xmp_preset_file(Path::new("look.XMP")));
    assert!(!is_xmp_preset_file(Path::new("look.calibraw-preset")));
    assert!(!is_xmp_preset_file(Path::new("look")));
}

#[test]
fn imports_subject_background_and_sky_masks() {
    let subject = correction(
        "Subject mask",
        r#"crs:CorrectionAmount="0.8""#,
        &[component("Subject 1", "1", "")],
        "",
    );
    let background = correction(
        "Background mask",
        "",
        &[background_component("Background 1", "")],
        "",
    );
    let sky = correction("Sky mask", "", &[component("Sky 1", "2", "")], "");
    let import = import(
        r#"crs:Exposure2012="0.5""#,
        &mask_group(&[subject, background, sky]),
    );

    let masks = &import.preset.edits().masks.masks;
    assert_eq!(masks.len(), 3);
    assert_eq!(masks[0].name, "Subject mask");
    assert_close(masks[0].opacity, 0.8);
    assert_eq!(masks[0].components[0].kind, MaskKind::Subject);
    assert_eq!(masks[0].components[0].name, "Subject 1");
    assert_eq!(masks[1].components[0].kind, MaskKind::Background);
    assert_eq!(masks[2].components[0].kind, MaskKind::Sky);
    for mask in masks {
        assert!(matches!(
            mask.components[0].geometry,
            MaskGeometry::Ai { mask: None, .. }
        ));
    }
    assert!(import.preset.selection().ai_masks);
    assert!(!import.preset.selection().masks);
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
}

#[test]
fn maps_mask_components_and_their_state() {
    let correction = correction(
        "Composite",
        r#"crs:CorrectionAmount="0.5" crs:CorrectionActive="false""#,
        &[
            background_component("Background 1", r#"crs:MaskBlendMode="0""#),
            component(
                "Subject 1",
                "1",
                r#"crs:MaskBlendMode="1" crs:MaskInverted="true" crs:MaskActive="false""#,
            ),
            component("Sky 1", "2", r#"crs:MaskBlendMode="2""#),
        ],
        "",
    );
    let import = import("", &mask_group(&[correction]));
    let mask = &import.preset.edits().masks.masks[0];

    assert!(!mask.enabled);
    assert_close(mask.opacity, 0.5);
    assert_eq!(mask.components.len(), 3);
    assert_eq!(mask.components[0].combine, MaskCombineMode::Add);
    assert_eq!(mask.components[1].combine, MaskCombineMode::Subtract);
    assert!(mask.components[1].invert);
    assert!(!mask.components[1].enabled);
    assert_eq!(mask.components[2].combine, MaskCombineMode::Intersect);
}

#[test]
fn maps_local_adjustments_and_tone_curves() {
    let curves = r#"
<crs:MainCurve><rdf:Seq>
 <rdf:li>0,0</rdf:li>
 <rdf:li>128,64</rdf:li>
 <rdf:li>255,255</rdf:li>
</rdf:Seq></crs:MainCurve>
<crs:BlueCurve><rdf:Seq>
 <rdf:li>0,0</rdf:li>
 <rdf:li>64,128</rdf:li>
 <rdf:li>255,255</rdf:li>
</rdf:Seq></crs:BlueCurve>
"#;
    let correction = correction(
        "Local",
        r#"crs:LocalExposure2012="0.5"
           crs:LocalContrast2012="0.25"
           crs:LocalHighlights2012="-0.5"
           crs:LocalShadows2012="0.75"
           crs:LocalWhites2012="-0.1"
           crs:LocalBlacks2012="0.1"
           crs:LocalTemperature="0.2"
           crs:LocalTint="-0.3"
           crs:LocalHue="0.5"
           crs:LocalSaturation="-0.4"
           crs:LocalTexture="0.6"
           crs:LocalClarity2012="-0.7"
           crs:LocalDehaze="0.8""#,
        &[component("Subject 1", "1", "")],
        curves,
    );
    let import = import("", &mask_group(&[correction]));
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    let adjustments = &import.preset.edits().masks.masks[0].adjustments;

    assert_close(adjustments.exposure, 0.5);
    assert_close(adjustments.contrast, 25.0);
    assert_close(adjustments.highlights, -50.0);
    assert_close(adjustments.shadows, 75.0);
    assert_close(adjustments.whites, -10.0);
    assert_close(adjustments.blacks, 10.0);
    assert_close(adjustments.temperature, 20.0);
    assert_close(adjustments.tint, -30.0);
    assert_close(adjustments.hue, 90.0);
    assert_close(adjustments.saturation, -40.0);
    assert_close(adjustments.texture, 60.0);
    assert_close(adjustments.clarity, -70.0);
    assert_close(adjustments.dehaze, 80.0);

    assert_eq!(
        curve_points(&adjustments.tone_curve),
        vec![[0.0, 0.0], [128.0 / 255.0, 64.0 / 255.0], [1.0, 1.0]]
    );
    assert_eq!(
        curve_points(&adjustments.tone_curve_blue),
        vec![[0.0, 0.0], [64.0 / 255.0, 128.0 / 255.0], [1.0, 1.0]]
    );
    assert_eq!(
        curve_points(&adjustments.tone_curve_red),
        vec![[0.0, 0.0], [1.0, 1.0]]
    );
}

#[test]
fn skips_unsupported_mask_components() {
    let brush = correction(
        "Brush",
        "",
        &[r#"<rdf:li crs:What="Mask/Aggregate" crs:MaskName="Brush 1"/>"#.to_owned()],
        "",
    );
    let person = correction(
        "Person",
        "",
        &[component("Person 1", "3", r#"crs:MaskSubCategoryID="13""#)],
        "",
    );
    let empty = correction("Empty", "", &[], "");
    let subtract = correction(
        "Subtract",
        "",
        &[component("Subject 1", "1", r#"crs:MaskBlendMode="1""#)],
        "",
    );
    let unknown_blend = correction(
        "Unknown blend",
        "",
        &[component("Sky 1", "2", r#"crs:MaskBlendMode="7""#)],
        "",
    );
    let subject = correction("Imported", "", &[component("Subject 1", "1", "")], "");
    let import = import(
        "",
        &mask_group(&[brush, person, empty, subtract, unknown_blend, subject]),
    );

    let masks = &import.preset.edits().masks.masks;
    assert_eq!(masks.len(), 1);
    assert_eq!(masks[0].name, "Imported");
    assert_eq!(masks[0].components[0].kind, MaskKind::Subject);
    for expected in [
        "brush",
        "person",
        "has no components",
        "starts with a subtracted component",
        "unknown blend mode",
    ] {
        assert!(
            import
                .warnings
                .iter()
                .any(|warning| warning.contains(expected)),
            "missing warning containing {expected:?}: {:?}",
            import.warnings
        );
    }
}

#[test]
fn warns_about_unsupported_local_adjustments() {
    let correction = correction(
        "Local",
        r#"crs:LocalSharpness="0.5"
           crs:LocalLuminanceNoise="0.2"
           crs:LocalMoire="0.1"
           crs:LocalDefringe="0.3"
           crs:LocalGrain="0.4"
           crs:LocalBrightness="0.1"
           crs:LocalToningHue="20"
           crs:LocalToningSaturation="0.1"
           crs:LocalCorrectedDepth="0.2"
           crs:LocalCurveRefineSaturation="50""#,
        &[component("Subject 1", "1", "")],
        r#"<crs:LocalPointColors><rdf:Seq><rdf:li>0.1, 0.2</rdf:li></rdf:Seq></crs:LocalPointColors>"#,
    );
    let import = import("", &mask_group(&[correction]));

    assert_eq!(import.preset.edits().masks.masks.len(), 1);
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("Some local adjustments")));
}

#[test]
fn mask_only_files_import_and_apply_as_ai_masks() {
    let correction = correction("Subject mask", "", &[component("Subject 1", "1", "")], "");
    let import = import("", &mask_group(&[correction]));

    assert!(import.preset.selection().adjustment_groups.is_empty());
    assert!(import.preset.selection().ai_masks);
    assert!(!import.preset.selection().masks);
    assert_eq!(import.preset.edits().masks.masks.len(), 1);

    let mut destination = default_edit_state();
    import.preset.apply_to(&mut destination);
    assert_eq!(destination.masks.masks.len(), 1);
    assert_eq!(
        destination.masks.masks[0].components[0].kind,
        MaskKind::Subject
    );
    assert!(destination.ai_masks_need_update);
}

#[test]
fn ignores_masks_inside_an_embedded_preset() {
    let nested = format!(
        r#"<crs:Preset><rdf:Description crs:Exposure2012="2.0">{}</rdf:Description></crs:Preset>"#,
        mask_group(&[correction(
            "Nested",
            "",
            &[component("Subject 1", "1", "")],
            "",
        )])
    );
    let body = format!(
        "{nested}{}",
        mask_group(&[correction(
            "Sky mask",
            "",
            &[component("Sky 1", "2", "")],
            "",
        )])
    );
    let import = import(r#"crs:Exposure2012="0.25""#, &body);

    assert_close(import.preset.edits().exposure.exposure, 0.25);
    let masks = &import.preset.edits().masks.masks;
    assert_eq!(masks.len(), 1);
    assert_eq!(masks[0].name, "Sky mask");
    assert_eq!(masks[0].components[0].kind, MaskKind::Sky);
}

#[test]
fn reads_masks_inside_the_parameters_description() {
    let body = format!(
        r#"<crs:Parameters><rdf:Description>{}</rdf:Description></crs:Parameters>"#,
        mask_group(&[correction(
            "Subject mask",
            "",
            &[component("Subject 1", "1", "")],
            "",
        )])
    );
    let import = import(r#"crs:Exposure2012="0.25""#, &body);

    assert_close(import.preset.edits().exposure.exposure, 0.25);
    assert_eq!(import.preset.edits().masks.masks.len(), 1);
    assert_eq!(
        import.preset.edits().masks.masks[0].components[0].kind,
        MaskKind::Subject
    );
}

#[test]
fn imported_masks_round_trip_and_apply() {
    let correction = correction(
        "Subject mask",
        r#"crs:CorrectionAmount="0.6" crs:LocalExposure2012="0.4""#,
        &[component("Subject 1", "1", "")],
        "",
    );
    let import = import(r#"crs:Exposure2012="0.5""#, &mask_group(&[correction]));

    let encoded = import.preset.encode().expect("the preset encodes");
    let decoded = Preset::decode(&encoded).expect("the preset decodes");
    assert_eq!(decoded, import.preset);

    let mut destination = default_edit_state();
    decoded.apply_to(&mut destination);
    assert_close(destination.exposure.exposure, 0.5);
    assert_eq!(destination.masks.masks.len(), 1);
    assert_close(destination.masks.masks[0].opacity, 0.6);
    assert_close(destination.masks.masks[0].adjustments.exposure, 0.4);
    assert!(destination.ai_masks_need_update);
}

fn tone_curve_import(attributes: &str, body: &str) -> PointCurve {
    let import = import(attributes, body);
    assert!(import
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ToneCurve));
    import.preset.edits().exposure.tone_curve
}

/// The output of the polyline through `curve`'s points at `x`.
fn curve_at(curve: &PointCurve, x: f32) -> f32 {
    let points = curve_points(curve);
    let next = points.iter().position(|point| point[0] >= x).unwrap();
    if next == 0 {
        return points[0][1];
    }
    let ([x0, y0], [x1, y1]) = (points[next - 1], points[next]);
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}

#[test]
fn folds_the_parametric_curve_into_the_rgb_curve() {
    let curve = tone_curve_import(
        r#"crs:ParametricHighlights="+60" crs:ParametricShadows="-40""#,
        "",
    );
    let points = curve_points(&curve);
    assert_close_point(points[0], [0.0, 0.0]);
    assert_close_point(*points.last().unwrap(), [1.0, 1.0]);
    assert!(points.windows(2).all(|pair| pair[1][1] >= pair[0][1]));
    assert!(curve_at(&curve, 0.875) > 0.875 + 0.02, "highlights rise");
    assert!(curve_at(&curve, 0.125) < 0.125 - 0.01, "shadows fall");
    // Fitting keeps the curve within about 1/1000 of the target.
    assert!((curve_at(&curve, 0.5) - 0.5).abs() < 2e-3);

    // A neutral parametric curve leaves the tone curve alone.
    let neutral = import(
        r#"crs:Exposure2012="0" crs:ParametricShadows="0" crs:ParametricShadowSplit="30""#,
        "",
    );
    assert!(!neutral
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ToneCurve));
}

#[test]
fn applies_the_parametric_curve_before_the_point_curve() {
    let halving = r#"<crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 128</rdf:li></rdf:Seq></crs:ToneCurvePV2012>"#;
    let lifted = tone_curve_import(r#"crs:ParametricLights="+80""#, "");
    let composed = tone_curve_import(r#"crs:ParametricLights="+80""#, halving);
    for x in [0.25, 0.5, 0.625, 0.75] {
        let expected = curve_at(&lifted, x) * 128.0 / 255.0;
        assert!(
            (curve_at(&composed, x) - expected).abs() < 0.01,
            "at {x}: {} vs {expected}",
            curve_at(&composed, x)
        );
    }
}

fn radial_component(attributes: &str) -> String {
    format!(
        r#"<rdf:li crs:What="Mask/CircularGradient" crs:MaskName="Radial Gradient 1" crs:Top="-0.1" crs:Left="0.2" crs:Bottom="0.9" crs:Right="0.6" crs:Angle="30" crs:Feather="40" {attributes}/>"#
    )
}

#[test]
fn imports_radial_and_linear_gradients() {
    let radial = correction(
        "Vignette",
        r#"crs:LocalExposure2012="-0.5""#,
        &[radial_component(
            r#"crs:MaskInverted="true" crs:Flipped="false""#,
        )],
        "",
    );
    let linear = correction(
        "Sky",
        "",
        &[
            r#"<rdf:li crs:What="Mask/LinearGradient" crs:ZeroX="0.5" crs:ZeroY="0.6" crs:FullX="0.5" crs:FullY="0.1"/>"#
                .to_owned(),
        ],
        "",
    );
    let import = import("", &mask_group(&[radial, linear]));
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    assert!(import.preset.selection().masks);
    assert!(!import.preset.selection().ai_masks);

    let masks = &import.preset.edits().masks.masks;
    let radial = &masks[0].components[0];
    assert_eq!(radial.kind, MaskKind::Radial);
    assert!(radial.invert);
    let MaskGeometry::Radial {
        center,
        radius,
        rotation,
        feather,
        initialized,
    } = radial.geometry
    else {
        panic!("a radial geometry");
    };
    assert_close_point(center, [0.4, 0.4]);
    assert_close_point(radius, [0.2, 0.5]);
    assert_close(rotation, 30.0_f32.to_radians());
    assert_close(feather, 0.4);
    assert!(initialized);
    assert_close(masks[0].adjustments.exposure, -0.5);

    let linear = &masks[1].components[0];
    assert_eq!(linear.kind, MaskKind::Linear);
    assert_eq!(
        linear.geometry,
        MaskGeometry::Linear {
            start: [0.5, 0.1],
            end: [0.5, 0.6],
            feather: 1.0,
            initialized: true,
        }
    );
}

#[test]
fn flipped_inverts_the_older_radial_filter() {
    let correction = correction(
        "Old radial",
        "",
        &[radial_component(r#"crs:Flipped="true""#)],
        "",
    );
    let import = import("", &mask_group(&[correction]));
    assert!(import.preset.edits().masks.masks[0].components[0].invert);
}

#[test]
fn reads_the_older_per_tool_correction_lists() {
    let gradient = r#"<crs:GradientBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" crs:LocalExposure2012="0.3">
<crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/Gradient" crs:ZeroX="0" crs:ZeroY="0" crs:FullX="1" crs:FullY="1"/></rdf:Seq></crs:CorrectionMasks>
<crs:CorrectionRangeMask crs:Version="2" crs:Type="0"/>
</rdf:Description></rdf:li></rdf:Seq></crs:GradientBasedCorrections>"#;
    let ranged = r#"<crs:CircularGradientBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction">
<crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/CircularGradient" crs:Top="0" crs:Left="0" crs:Bottom="1" crs:Right="1" crs:Angle="0" crs:Feather="50"/></rdf:Seq></crs:CorrectionMasks>
<crs:CorrectionRangeMask crs:Version="3" crs:Type="2" crs:LumRange="0 0.2 0.6 0.8"/>
</rdf:Description></rdf:li></rdf:Seq></crs:CircularGradientBasedCorrections>"#;
    let brush = r#"<crs:PaintBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction">
<crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/Paint" crs:Radius="0.1"/></rdf:Seq></crs:CorrectionMasks>
</rdf:Description></rdf:li></rdf:Seq></crs:PaintBasedCorrections>"#;
    let import = import("", &format!("{gradient}{ranged}{brush}"));

    let masks = &import.preset.edits().masks.masks;
    assert_eq!(masks.len(), 2);
    assert_eq!(masks[0].components.len(), 1);
    assert_eq!(masks[0].components[0].kind, MaskKind::Linear);
    assert_close(masks[0].adjustments.exposure, 0.3);
    let kinds: Vec<_> = masks[1]
        .components
        .iter()
        .map(|component| (component.kind, component.combine))
        .collect();
    assert_eq!(
        kinds,
        [
            (MaskKind::Radial, MaskCombineMode::Add),
            (MaskKind::LuminanceRange, MaskCombineMode::Intersect),
        ]
    );
    assert!(import.preset.selection().masks && import.preset.selection().ai_masks);
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("(brush)")));
}

fn range_component(name: &str, lum_range: &str) -> String {
    format!(
        r#"<rdf:li><rdf:Description crs:What="Mask/RangeMask" crs:MaskName="{name}" crs:MaskBlendMode="0">
<crs:CorrectionRangeMask crs:Version="3" crs:Type="2" crs:SampleType="0" crs:LumRange="{lum_range}"/>
</rdf:Description></rdf:li>"#
    )
}

#[test]
fn imports_luminance_ranges() {
    let highlights = correction(
        "Highlights",
        "",
        &[range_component("Bright", "0.6 0.8 1 1")],
        "",
    );
    let shadows = correction("Shadows", "", &[range_component("Dark", "0 0 0.1 0.5")], "");
    let import = import("", &mask_group(&[highlights, shadows]));
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    assert!(import.preset.selection().ai_masks);

    let masks = &import.preset.edits().masks.masks;
    let MaskGeometry::LuminanceRange {
        low,
        high,
        feather,
        high_feather,
        ..
    } = masks[0].components[0].geometry
    else {
        panic!("a luminance range");
    };
    let weight = |lightness: f32| {
        luminance_range_weight(
            crate::pipeline::lightness_to_luminance(lightness),
            low,
            high,
            [feather, high_feather.unwrap()],
        )
    };
    assert_close(weight(0.5), 0.0);
    assert_close(weight(0.9), 1.0);
    // Full up to white keeps white selected.
    assert_close(weight(1.0), 1.0);
    assert!(weight(0.7) > 0.0 && weight(0.7) < 1.0);

    let MaskGeometry::LuminanceRange {
        low,
        high,
        feather,
        high_feather,
        ..
    } = masks[1].components[0].geometry
    else {
        panic!("a luminance range");
    };
    let weight = |lightness: f32| {
        luminance_range_weight(
            crate::pipeline::lightness_to_luminance(lightness),
            low,
            high,
            [feather, high_feather.unwrap()],
        )
    };
    assert_close(weight(0.05), 1.0);
    assert_close(weight(0.6), 0.0);
}

#[test]
fn skips_masks_with_color_ranges() {
    let colored = r#"<rdf:li><rdf:Description crs:What="Mask/RangeMask" crs:MaskName="Color Range 1">
<crs:CorrectionRangeMask crs:Version="3" crs:Type="1" crs:ColorAmount="0.5"/>
</rdf:Description></rdf:li>"#;
    let correction = correction("Colored", "", &[colored.to_owned()], "");
    let import = import("", &mask_group(&[correction]));
    assert!(import.preset.edits().masks.masks.is_empty());
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("“Color Range 1” (color range)")));
}

#[test]
fn imports_the_lens_profile_and_aberration_removal() {
    let profile = import(
        r#"crs:LensProfileEnable="1" crs:LensProfileName="Some lens" crs:AutoLateralCA="1" crs:LensProfileVignettingScale="0""#,
        "",
    );
    assert!(profile.warnings.is_empty(), "{:?}", profile.warnings);
    assert!(profile.preset.selection().lens_correction);
    let lens = &profile.preset.edits().lens;
    assert!(lens.enabled);
    assert!(
        lens.maker.is_empty() && lens.model.is_empty(),
        "matched on the target photo"
    );
    assert!(lens.geometry);
    assert!(!lens.vignetting);
    assert_eq!(lens.manual, None);
    assert_close(profile.preset.edits().exposure.chromatic_aberration, 100.0);

    let partial = import(
        r#"crs:LensProfileEnable="1" crs:LensProfileDistortionScale="50""#,
        "",
    );
    assert!(partial
        .warnings
        .iter()
        .any(|warning| warning.contains("strengths other than 0 and 100")));
}

#[test]
fn imports_manual_lens_corrections_instead_of_the_profile() {
    let import = import(
        r#"crs:LensProfileEnable="1" crs:LensManualDistortionAmount="12" crs:VignetteAmount="-30""#,
        "",
    );
    let lens = &import.preset.edits().lens;
    assert!(lens.enabled);
    let manual = lens.manual.expect("manual corrections");
    assert_close(manual.distortion, 12.0);
    assert_close(manual.vignetting, -30.0);
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("cannot be combined")));
}

#[test]
fn leaves_lens_corrections_alone_when_the_preset_turns_none_on() {
    let import = import(
        r#"crs:Exposure2012="0" crs:LensProfileEnable="0" crs:LensProfileName="Some lens" crs:AutoLateralCA="0" crs:LensManualDistortionAmount="0""#,
        "",
    );
    assert!(!import.preset.selection().lens_correction);
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
}

/// A swatch: neutral gray sample, the given shifts and the default ranges.
fn gray_swatch(value: f32, shifts: [f32; 3]) -> String {
    let [hue, saturation, luminance] = shifts;
    format!(
        "0, 0, {value}, {hue}, {saturation}, {luminance}, 0.5, 0, 0.33, 0.67, 1, 0, 0.33, 0.67, 1, 0, 0.33, 0.67, 1"
    )
}

const UNUSED_SWATCH: &str =
    "-1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1";

#[test]
fn imports_global_point_colors() {
    let body = format!(
        r#"<crs:PointColors><rdf:Seq><rdf:li>{}</rdf:li><rdf:li>{UNUSED_SWATCH}</rdf:li></rdf:Seq></crs:PointColors>
<crs:ColorVariance><rdf:Seq><rdf:li>0</rdf:li><rdf:li>-50</rdf:li></rdf:Seq></crs:ColorVariance>"#,
        gray_swatch(0.5, [1.0, -0.5, 0.2])
    );
    let import = import("", &body);
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    assert!(import
        .preset
        .selection()
        .adjustment_groups
        .contains(AdjustmentGroup::ColorMixer));

    let colors = &import.preset.edits().exposure.point_colors;
    assert_eq!(colors.len(), 1);
    let color = &colors[0];
    // Gray stays gray from ProPhoto to Rec.2020.
    assert_close(color.sample_hsl[1], 0.0);
    assert_close(color.sample_hsl[2], 0.5);
    assert_close(color.hue_shift, 35.0 / 360.0 * 200.0);
    assert_close(color.saturation_shift, -50.0);
    assert_close(color.luminance_shift, 0.2 * 100.0 * 0.5);
    assert_close(color.range, 50.0);
    assert_close(color.hue_range.min, -0.125);
    assert_close(color.hue_range.max, 0.125);
    assert_close(color.luminance_range.inner_max, (2.0 * 0.67 - 1.0) * 0.5);
}

#[test]
fn imports_local_point_colors_and_reports_variance() {
    let correction = correction(
        "Skin",
        "",
        &[component("Subject 1", "1", "")],
        &format!(
            r#"<crs:LocalPointColors><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></crs:LocalPointColors>
<crs:LocalColorVariance><rdf:Seq><rdf:li>-0.11</rdf:li></rdf:Seq></crs:LocalColorVariance>"#,
            gray_swatch(0.8, [0.0, 0.1, 0.0])
        ),
    );
    let import = import("", &mask_group(&[correction]));
    let colors = &import.preset.edits().masks.masks[0]
        .adjustments
        .point_colors;
    assert_eq!(colors.len(), 1);
    assert_close(colors[0].saturation_shift, 10.0);
    assert!(import
        .warnings
        .iter()
        .any(|warning| warning.contains("Point color variance of the mask “Skin”")));
}
