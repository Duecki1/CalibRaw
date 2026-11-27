//! Rust/WGSL buffer-layout and shared-constant contracts.
//!
//! The checks inspect the modules `ShaderManager` composes in production, so
//! imports and source specialization match what the GPU compiles. A
//! `#[repr(C)]`/`bytemuck::Pod` Rust type does not prove WGSL compatibility:
//! Rust aligns `[f32; 4]` to 4 bytes, while a WGSL `vec4<f32>` aligns to 16 and
//! a uniform `mat3x3<f32>` has a 16-byte column stride. Each member's byte
//! offset and size are therefore compared explicitly.

use super::output_parts::{
    effects_before_creative, effects_before_view, scene_tone_before_creative,
    scene_tone_before_view, slot_before_view, OutputPart,
};
use super::{
    effect_programs::{dispatched_effects, effect_def, EffectSet},
    processing_work_format, scene_emission_light_rows,
    shader_manager::ShaderManager,
    shaders, work_shader_source, CameraUniforms, CfaKind, EffectsUniforms, MaskData,
    PackedPointColor, ProcessingQuality, RemoveCompositeParams, SceneToneUniforms,
    ViewTransformMode, CONTOUR_BRIGHTNESS_BINS, CONTOUR_CELL_BYTES, CONTOUR_CONTRAST_BINS,
    CONTOUR_GRID_LONG, FILM_MODE_MONOCHROME, FILM_MODE_POSITIVE, HAZE_AIRLIGHT_BYTES,
    HAZE_CELL_BYTES, HAZE_GRID_LONG, IMAGE_LIGHT_BANDS, IMAGE_LIGHT_GRID_LONG,
    LEGACY_LUMINANCE_BLACKS, LEGACY_LUMINANCE_CONTRAST, LEGACY_LUMINANCE_TONE_CURVE,
    MASK_EFFECT_ID_SHIFT, MASK_SCENE_BAKE_LIGHT_BITS, MAX_RENDER_MASK_SLOTS, MEDIUM_CELLS,
    SCENE_EMISSION_GRID_LONG, SCENE_EMISSION_SURFACE_GRID_LONG, SCENE_SHADOW_LIGHTS,
    TONE_HISTOGRAM_BIN_COUNT, TONE_STATS_SIZE_BYTES, TREATMENT_MONOCHROME,
};
use super::{DETAIL_PYRAMID_LEVELS, DETAIL_PYRAMID_TEXTURE_LEVELS};
use crate::pipeline::{effect_lanes::options, MaskEffect, ReachGroup, OPTION_COUNT};
use calibraw_core::pipeline::{
    CA_WINDOW_RADIUS, DETAIL_CLARITY_SUPPORT, DETAIL_TEXTURE_SUPPORT, HAZE_BOX_SUPPORT,
    HAZE_GUIDE_SUPPORT, MAX_LATERAL_SHIFT, MAX_RESOLUTION_SCALE, TONE_GUIDE_RADIUS_CELLS,
};
use naga::proc::Layouter;
use std::collections::BTreeSet;

/// A Rust struct's size and `(field, offset, size)` list, in declaration order.
pub(super) type RustLayout = (usize, Vec<(&'static str, usize, usize)>);

/// Builds a [`RustLayout`] from `offset_of!` and the sizes of a zeroed value.
macro_rules! rust_layout {
    ($ty:ty { $($field:ident),+ $(,)? }) => {{
        let value = <$ty as bytemuck::Zeroable>::zeroed();
        (
            std::mem::size_of::<$ty>(),
            vec![$((
                stringify!($field),
                std::mem::offset_of!($ty, $field),
                std::mem::size_of_val(&value.$field),
            )),+],
        )
    }};
}

/// The name of a type, constant or global without naga_oil's module suffix.
fn undecorated(name: &str) -> &str {
    name.split("X_naga_oil_mod_X").next().unwrap_or(name)
}

/// WGSL member names cannot end in a digit after naga_oil composition, so
/// shaders spell e.g. Rust's `adjust_0` as `adjust_0_field`.
fn rust_member_name(wgsl_member: &str) -> &str {
    wgsl_member.strip_suffix("_field").unwrap_or(wgsl_member)
}

/// Asserts that the WGSL struct `wgsl_name` in `module` has exactly the Rust
/// layout: equal total size, member count, order, offsets and sizes.
/// Returns false when the module does not contain the struct.
pub(super) fn check_struct_layout(
    module: &naga::Module,
    wgsl_name: &str,
    rust: &RustLayout,
    context: &str,
) -> bool {
    let mut layouter = Layouter::default();
    layouter
        .update(module.to_ctx())
        .unwrap_or_else(|error| panic!("{context}: WGSL layout failed: {error}"));
    let Some((_, ty)) = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref().map(undecorated) == Some(wgsl_name))
    else {
        return false;
    };
    let naga::TypeInner::Struct { members, span } = &ty.inner else {
        panic!("{context}: {wgsl_name} is not a struct");
    };
    let (rust_size, rust_fields) = rust;
    assert_eq!(
        *span as usize, *rust_size,
        "{context}: {wgsl_name} WGSL span differs from the Rust size"
    );
    let wgsl_fields = members
        .iter()
        .map(|member| {
            (
                rust_member_name(member.name.as_deref().unwrap_or_default()).to_owned(),
                member.offset as usize,
                layouter[member.ty].size as usize,
            )
        })
        .collect::<Vec<_>>();
    let rust_fields = rust_fields
        .iter()
        .map(|(name, offset, size)| ((*name).to_owned(), *offset, *size))
        .collect::<Vec<_>>();
    assert_eq!(
        wgsl_fields, rust_fields,
        "{context}: {wgsl_name} members (name, offset, size) differ between WGSL and Rust"
    );
    true
}

/// The value of a `u32` constant declared in `module`, by undecorated name.
pub(super) fn u32_constant(module: &naga::Module, name: &str) -> Option<u32> {
    module.constants.iter().find_map(|(_, constant)| {
        (constant.name.as_deref().map(undecorated) == Some(name)).then(|| {
            match module.global_expressions[constant.init] {
                naga::Expression::Literal(naga::Literal::U32(value)) => value,
                naga::Expression::Literal(naga::Literal::AbstractInt(value)) => {
                    u32::try_from(value).expect("constant fits in u32")
                }
                naga::Expression::Literal(naga::Literal::I32(value)) => {
                    u32::try_from(value).expect("constant is not negative")
                }
                ref other => panic!("{name} is not a u32 literal: {other:?}"),
            }
        })
    })
}

/// Every production entry module, composed for both qualities with the
/// registrations of the sensor layout it is built for.
fn production_modules() -> Vec<(String, naga::Module)> {
    let mut modules = Vec::new();
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let format = processing_work_format(quality);
        for cfa_kind in [CfaKind::Bayer, CfaKind::XTrans] {
            let mut manager = ShaderManager::new(format, cfa_kind).expect("register WGSL modules");
            for shader in shaders::ALL_ENTRY_SHADERS {
                if shader.sensor.unwrap_or(CfaKind::Bayer) != cfa_kind {
                    continue;
                }
                let module_text = shader.source.module_text();
                let text = if shader.work_format {
                    work_shader_source(&module_text, format).expect("specialize work format")
                } else {
                    std::borrow::Cow::Borrowed(module_text.as_ref())
                };
                let module = manager
                    .compose_naga_module(
                        text.as_ref(),
                        shader.source.file_name,
                        EffectSet::all().shader_defs(),
                    )
                    .unwrap_or_else(|error| panic!("{} did not compose: {error:#}", shader.label));
                modules.push((
                    format!("{} ({quality:?}, {cfa_kind:?})", shader.label),
                    module,
                ));
            }
        }
    }
    modules
}

/// naga passes a texture or sampler argument as a pointer parameter, which
/// is valid SPIR-V but crashes the Mali-G710 driver (r54p3, Pixel 7 Pro) in
/// `vkCreateComputePipelines`: the app dies while compiling the shader, which
/// no error scope can catch. Shaders read their textures directly instead.
#[test]
fn no_handle_arguments() {
    for (label, module) in production_modules() {
        for (_, function) in module.functions.iter() {
            for argument in &function.arguments {
                let is_handle = matches!(
                    module.types[argument.ty].inner,
                    naga::TypeInner::Image { .. }
                        | naga::TypeInner::Sampler { .. }
                        | naga::TypeInner::BindingArray { .. }
                        | naga::TypeInner::AccelerationStructure { .. }
                );
                assert!(
                    !is_handle,
                    "{label}: {} takes the texture or sampler {}",
                    function.name.as_deref().unwrap_or("?"),
                    argument.name.as_deref().unwrap_or("?"),
                );
            }
        }
    }
}

#[test]
fn shared_uniform_and_mask_structs_match_rust_layouts() {
    let contracts: [(&str, RustLayout); 4] = [
        (
            "CameraUniforms",
            rust_layout!(CameraUniforms {
                black_point,
                highlight_clip,
                chroma_denoise,
                highlight_reconstruction,
                tone_analysis_scale,
                tone_guide_radius,
                demosaic_mode,
                dual_threshold,
                frequency_chroma,
                view_transform_mode,
                padding_0,
                padding_1,
                highlight_options,
                noise_shot,
                noise_read,
                noise_options,
                wb,
                cam_to_srgb_0,
                cam_to_srgb_1,
                cam_to_srgb_2,
                black_levels,
                white_levels,
                width,
                height,
                tile_origin_x,
                tile_origin_y,
                full_width,
                full_height,
                abi_version,
                abi_size_bytes,
                tone_histogram_bounds,
                profile_hue_sat,
                profile_look,
                profile_tone,
                profile_flags,
                ai_denoise_enabled,
                user_exposure_bits,
                ai_cfa_weight,
                source_short_edge,
                chromatic_aberration,
                lateral_ca_red,
                lateral_ca_blue,
                film_base,
                film_exponents,
                film_gains,
                capture_sharpening,
                capture_clip,
            }),
        ),
        (
            "SceneToneUniforms",
            rust_layout!(SceneToneUniforms {
                exposure,
                saturation,
                vibrance,
                legacy_luminance_tone,
                basic_tone,
                sigmoid_curve,
                sigmoid_power,
                reference_sigmoid_curve,
                reference_sigmoid_power,
                tone_curves,
                hsl_hue_0,
                hsl_hue_1,
                hsl_saturation_0,
                hsl_saturation_1,
                hsl_luminance_0,
                hsl_luminance_1,
                mask_counts,
                grade_shadows,
                grade_midtones,
                grade_highlights,
                grade_global,
                grade_options,
                rec2020_to_xyz,
                xyz_to_rec2020,
                xyz_to_bradford,
                bradford_to_xyz,
                point_colors,
                point_color_meta,
                look_lut,
                look_lut_domain_min,
                look_lut_domain_scale,
                raster_temperature,
                treatment,
            }),
        ),
        (
            "EffectsUniforms",
            rust_layout!(EffectsUniforms {
                presence,
                sharpen,
                glow_diffusion,
                vignette_frame,
                vignette_projection,
                vignette_dark_half_fit,
                vignette_dark_full_fit,
                vignette_light_half_fit,
                vignette_light_full_fit,
                capture_scale_sigma,
                capture_thresholds,
                capture_mask_coherence,
            }),
        ),
        (
            "PointColor",
            rust_layout!(PackedPointColor {
                sample_range,
                hue_range,
                saturation_range,
                luminance_range,
                shifts,
            }),
        ),
    ];

    let modules = production_modules();
    let mut checked = BTreeSet::new();
    for (context, module) in &modules {
        for (wgsl_name, rust) in &contracts {
            if check_struct_layout(module, wgsl_name, rust, context) {
                checked.insert(*wgsl_name);
            }
        }
    }
    let expected = contracts
        .iter()
        .map(|(name, _)| *name)
        .collect::<BTreeSet<_>>();
    assert_eq!(checked, expected, "every contract struct reaches a module");
}

/// The mask slot table is read as rows of 16 bytes (`mask_rows` in
/// common.wgsl): the slot's row count and the first row of each member the
/// accessors read must match the Rust `MaskData`.
/// [`MaskData`]'s [`RustLayout`]: one field per `MASK_ROW_*` constant.
fn mask_data_layout() -> RustLayout {
    rust_layout!(MaskData {
        metadata,
        adjust_0,
        adjust_1,
        adjust_2,
        adjust_3,
        effect_options,
        curves,
        grade_shadows,
        grade_midtones,
        grade_highlights,
        grade_global,
        grade_options,
        curves_red,
        curves_green,
        curves_blue,
        hsl_hue_0,
        hsl_hue_1,
        hsl_saturation_0,
        hsl_saturation_1,
        hsl_luminance_0,
        hsl_luminance_1,
        point_colors,
        point_color_meta,
    })
}

#[test]
fn mask_slot_rows_match_the_rust_layout() {
    const ROW_BYTES: usize = 16;
    let (size, fields) = mask_data_layout();
    let modules = production_modules();
    let mut seen = BTreeSet::new();
    for (context, module) in &modules {
        if let Some(rows) = u32_constant(module, "MASK_SLOT_ROWS") {
            assert_eq!(rows as usize * ROW_BYTES, size, "{context}: MASK_SLOT_ROWS");
            seen.insert("MASK_SLOT_ROWS".to_owned());
        }
        for (field, offset, _) in &fields {
            let name = format!("MASK_ROW_{}", field.to_uppercase());
            if let Some(row) = u32_constant(module, &name) {
                assert_eq!(row as usize * ROW_BYTES, *offset, "{context}: {name}");
                seen.insert(name);
            }
        }
    }
    assert_eq!(
        seen.len(),
        fields.len() + 1,
        "every row constant reaches a module: {seen:?}"
    );
    assert_eq!(
        std::mem::size_of::<PackedPointColor>(),
        5 * ROW_BYTES,
        "mask_point_colors reads five rows per point colour"
    );
}

#[test]
fn shared_buffer_bindings_match_the_rust_bind_group_layouts() {
    // (global, group, binding, address space) as bound by `create_bind_group_layouts`.
    let expected = [
        ("camera_uniforms", 0, 0, naga::AddressSpace::Uniform),
        ("scene_tone_uniforms", 1, 0, naga::AddressSpace::Uniform),
        ("effects_uniforms", 2, 0, naga::AddressSpace::Uniform),
        (
            "mask_rows",
            0,
            33,
            naga::AddressSpace::Storage {
                access: naga::StorageAccess::LOAD,
            },
        ),
    ];
    let modules = production_modules();
    let mut seen = BTreeSet::new();
    for (context, module) in &modules {
        for (_, global) in module.global_variables.iter() {
            let Some(name) = global.name.as_deref().map(undecorated) else {
                continue;
            };
            let Some((_, group, binding, space)) =
                expected.iter().find(|(expected, ..)| *expected == name)
            else {
                continue;
            };
            let resource = global.binding.as_ref().expect("buffer globals are bound");
            assert_eq!(
                (resource.group, resource.binding, global.space),
                (*group, *binding, *space),
                "{context}: {name} binding"
            );
            seen.insert(name.to_owned());
        }
    }
    assert_eq!(seen.len(), expected.len(), "every shared buffer is used");
}

#[test]
fn the_look_lut_texture_matches_the_view_layout() {
    let modules = production_modules();
    let mut seen = false;
    for (context, module) in &modules {
        for (_, global) in module.global_variables.iter() {
            if global.name.as_deref().map(undecorated) != Some("look_lut_tex") {
                continue;
            }
            let resource = global.binding.as_ref().expect("textures are bound");
            assert_eq!((resource.group, resource.binding), (0, 61), "{context}");
            assert!(
                matches!(
                    module.types[global.ty].inner,
                    naga::TypeInner::Image {
                        dim: naga::ImageDimension::D3,
                        arrayed: false,
                        class: naga::ImageClass::Sampled {
                            kind: naga::ScalarKind::Float,
                            multi: false,
                        },
                    }
                ),
                "{context}: look_lut_tex is an unfiltered 3D float texture"
            );
            seen = true;
        }
    }
    assert!(seen, "the view pass reads the LUT");
}

/// The value of a u32 constant, which every module that keeps it must agree on.
fn shared_u32_constant(modules: &[(String, naga::Module)], name: &str) -> u32 {
    let values = modules
        .iter()
        .filter_map(|(_, module)| u32_constant(module, name))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        values.len(),
        1,
        "{name} has one value across modules: {values:?}"
    );
    values.into_iter().next().unwrap()
}

#[test]
fn shared_constants_and_effect_ids_match_rust() {
    let modules = production_modules();
    let constant = |name: &str| shared_u32_constant(&modules, name);

    assert_eq!(
        constant("MAX_RENDER_MASK_SLOTS") as usize,
        MAX_RENDER_MASK_SLOTS
    );
    assert_eq!(constant("MASK_EFFECT_ID_SHIFT"), MASK_EFFECT_ID_SHIFT);
    assert_eq!(
        constant("MASK_SCENE_BAKE_LIGHT_BITS"),
        MASK_SCENE_BAKE_LIGHT_BITS
    );
    const {
        assert!(MASK_SCENE_BAKE_LIGHT_BITS as usize >= SCENE_SHADOW_LIGHTS);
        assert!(MASK_SCENE_BAKE_LIGHT_BITS < 1 << MASK_EFFECT_ID_SHIFT);
    }
    for (name, mode) in [
        ("VIEW_TRANSFORM_PASSTHROUGH", ViewTransformMode::Passthrough),
        (
            "VIEW_TRANSFORM_SCENE_SIGMOID",
            ViewTransformMode::SceneSigmoid,
        ),
        (
            "VIEW_TRANSFORM_DISPLAY_RETONE",
            ViewTransformMode::DisplayRetone,
        ),
        (
            "VIEW_TRANSFORM_PROFILE_TONE",
            ViewTransformMode::ProfileTone,
        ),
        (
            "VIEW_TRANSFORM_PROFILE_TONE_RETONE",
            ViewTransformMode::ProfileToneRetone,
        ),
    ] {
        assert_eq!(constant(name), mode.shader_value(), "{name}");
    }
    assert_eq!(
        constant("TONE_HISTOGRAM_BIN_COUNT"),
        TONE_HISTOGRAM_BIN_COUNT
    );
    assert_eq!(constant("FILM_MODE_POSITIVE"), FILM_MODE_POSITIVE);
    assert_eq!(constant("FILM_MODE_MONOCHROME"), FILM_MODE_MONOCHROME);
    assert_eq!(constant("TREATMENT_MONOCHROME"), TREATMENT_MONOCHROME);
    assert_eq!(
        constant("LEGACY_LUMINANCE_TONE_CURVE"),
        LEGACY_LUMINANCE_TONE_CURVE
    );
    assert_eq!(
        constant("LEGACY_LUMINANCE_CONTRAST"),
        LEGACY_LUMINANCE_CONTRAST
    );
    assert_eq!(constant("LEGACY_LUMINANCE_BLACKS"), LEGACY_LUMINANCE_BLACKS);
    // An adjustment slot keeps them between its view flags and its effect ID.
    const {
        assert!(LEGACY_LUMINANCE_TONE_CURVE > 4);
        assert!(LEGACY_LUMINANCE_BLACKS < 1 << MASK_EFFECT_ID_SHIFT);
    }
    // The image-light grid buffer and textures are sized from these.
    assert_eq!(constant("IMAGE_LIGHT_GRID_LONG"), IMAGE_LIGHT_GRID_LONG);
    assert_eq!(constant("IMAGE_LIGHT_BANDS"), IMAGE_LIGHT_BANDS);
    // So are the haze grid's.
    assert_eq!(constant("HAZE_GRID_LONG"), HAZE_GRID_LONG);
    // The contour grid buffer and the scene emission textures are sized from these.
    assert_eq!(constant("CONTOUR_GRID_LONG"), CONTOUR_GRID_LONG);
    assert_eq!(constant("CONTOUR_CONTRAST_BINS"), CONTOUR_CONTRAST_BINS);
    assert_eq!(constant("CONTOUR_BRIGHTNESS_BINS"), CONTOUR_BRIGHTNESS_BINS);
    assert_eq!(
        constant("CONTOUR_BINS"),
        CONTOUR_CONTRAST_BINS * CONTOUR_BRIGHTNESS_BINS
    );
    assert_eq!(
        constant("SCENE_EMISSION_GRID_LONG"),
        SCENE_EMISSION_GRID_LONG
    );
    assert_eq!(
        constant("SCENE_EMISSION_SURFACE_GRID_LONG"),
        SCENE_EMISSION_SURFACE_GRID_LONG
    );
    assert_eq!(constant("MEDIUM_CELLS"), MEDIUM_CELLS);
    // The CPU allocates the Glow map and the shader computes its sampling grid.
    assert_eq!(
        constant("STYLED_GLOW_MAP_LONG"),
        super::styled_glow::STYLED_GLOW_MAP_LONG
    );
    // Lens Blur's textures and sums are sized from these.
    assert_eq!(
        constant("DEFOCUS_LOCAL_BINS") as usize,
        super::defocus::LOCAL_BINS
    );
    assert_eq!(
        constant("DEFOCUS_SOURCE_LAYERS"),
        super::defocus::SOURCE_LAYERS
    );
    assert_eq!(constant("DEFOCUS_SLAB_LAYERS"), super::defocus::SLAB_LAYERS);
    assert_eq!(
        u64::from(constant("DEFOCUS_CELL_VECTORS")),
        super::defocus::CELL_VECTORS
    );
    assert_eq!(
        constant("SCENE_EMISSION_LIGHT_ROWS"),
        scene_emission_light_rows(
            SCENE_EMISSION_GRID_LONG,
            SCENE_EMISSION_SURFACE_GRID_LONG,
            MEDIUM_CELLS
        )
    );
    for effect in MaskEffect::ALL {
        if effect == MaskEffect::Adjustment {
            assert_eq!(effect.shader_id(), 0, "adjustment masks use shader ID 0");
            continue;
        }
        let name = format!(
            "MASK_EFFECT_{}_ID",
            screaming_snake_case(&format!("{effect:?}"))
        );
        assert_eq!(constant(&name), effect.shader_id(), "{name}");
    }
}

/// The pass counts of the Texture and Clarity cascade and the export-tile
/// support reserved for it follow from detail_scale_space.wgsl: level k filters
/// at a step of 2^k pixels with `DETAIL_PYRAMID_KERNEL_RADIUS` taps a side.
#[test]
fn detail_pyramid_levels_and_tile_support_match_the_shader() {
    let modules = production_modules();
    let constant = |name: &str| shared_u32_constant(&modules, name);
    let levels = constant("DETAIL_PYRAMID_LEVEL_COUNT");
    let texture_levels = constant("DETAIL_PYRAMID_TEXTURE_LEVEL_COUNT");
    let radius = constant("DETAIL_PYRAMID_KERNEL_RADIUS");
    assert_eq!(levels as usize, DETAIL_PYRAMID_LEVELS);
    assert_eq!(texture_levels as usize, DETAIL_PYRAMID_TEXTURE_LEVELS);
    // Level 0 writes the texture the scene effects read and the levels
    // alternate, so both counts must be odd.
    assert!(levels % 2 == 1 && texture_levels % 2 == 1);
    let footprint = |levels: u32| radius * ((1 << levels) - 1);
    assert_eq!(footprint(levels), DETAIL_CLARITY_SUPPORT);
    assert_eq!(footprint(texture_levels), DETAIL_TEXTURE_SUPPORT);
}

/// Chromatic aberration reads no farther past a tile core than the export halo
/// reserves (`CHROMATIC_ABERRATION_SUPPORT`), and clamps lateral shifts to the
/// largest the estimate allows.
#[test]
fn chromatic_aberration_reach_matches_the_tile_support() {
    let modules = production_modules();
    let constant = |name: &str| shared_u32_constant(&modules, name);
    assert_eq!(constant("CA_MAX_LATERAL_SHIFT") as f32, MAX_LATERAL_SHIFT);
    assert_eq!(constant("CA_WINDOW_RADIUS"), CA_WINDOW_RADIUS);
}

/// The tone-guide blur in tone_analysis.wgsl takes enough taps for its radius
/// at the largest resolution scale, which TONE_GUIDE_SUPPORT in tiles.rs reserves.
#[test]
fn tone_guide_taps_cover_the_largest_scaled_radius() {
    let modules = production_modules();
    let taps = shared_u32_constant(&modules, "TONE_GUIDE_MAX_RADIUS_CELLS");
    assert!(TONE_GUIDE_RADIUS_CELLS * MAX_RESOLUTION_SCALE <= taps);
}

/// The registry's lanes and options reach the shaders as constants
/// (effect_wgsl.rs), and no shader declares lanes of its own.
#[test]
fn effect_lanes_and_options_reach_the_shaders() {
    let modules = production_modules();
    let (_, creative) = modules
        .iter()
        .find(|(name, _)| name.starts_with(shaders::CREATIVE_EFFECTS_ENTRY.label))
        .expect("creative effects module");
    let constant = |name: &str| {
        u32_constant(creative, name).unwrap_or_else(|| panic!("WGSL declares {name}")) as usize
    };
    let mut expected = BTreeSet::new();
    for effect in MaskEffect::ALL {
        let prefix = effect.shader_name().to_uppercase();
        for lane in effect.lanes() {
            let wgsl_name = format!("{prefix}_{}_LANE", lane.name);
            assert_eq!(constant(&wgsl_name), lane.index, "{wgsl_name}");
            expected.insert(wgsl_name);
        }
    }
    for (name, index) in options::ALL {
        let wgsl_name = format!("{name}_OPTION");
        assert_eq!(constant(&wgsl_name), index, "{wgsl_name}");
        assert!((0..OPTION_COUNT).contains(&index), "{wgsl_name}");
        expected.insert(wgsl_name);
    }
    let declared: BTreeSet<_> = creative
        .constants
        .iter()
        .filter_map(|(_, constant)| constant.name.as_deref().map(undecorated))
        .filter(|name| name.ends_with("_LANE") || name.ends_with("_OPTION"))
        .map(str::to_owned)
        .collect();
    assert_eq!(declared, expected);
}

#[test]
fn tone_statistics_buffers_match_their_wgsl_declarations() {
    let modules = production_modules();
    let (_, tone_analysis) = modules
        .iter()
        .find(|(name, _)| name.starts_with(shaders::TONE_ANALYSIS_ENTRY.label))
        .expect("tone analysis module");
    let mut layouter = Layouter::default();
    layouter.update(tone_analysis.to_ctx()).unwrap();
    let size_of = |wgsl_name: &str| {
        let (handle, _) = tone_analysis
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref().map(undecorated) == Some(wgsl_name))
            .unwrap_or_else(|| panic!("{wgsl_name} is declared"));
        u64::from(layouter[handle].size)
    };
    assert_eq!(size_of("ToneStats"), TONE_STATS_SIZE_BYTES);
    assert_eq!(
        size_of("ToneHistogram"),
        u64::from(TONE_HISTOGRAM_BIN_COUNT) * std::mem::size_of::<u32>() as u64
    );
}

/// The haze buffers Rust allocates hold the WGSL structs, and the export
/// halo reserves how far the haze grid's boxes and Dehaze's guide scene reach
/// past a tile core.
#[test]
fn haze_buffers_and_supports_match_their_wgsl_declarations() {
    let modules = production_modules();
    let (_, haze_analysis) = modules
        .iter()
        .find(|(name, _)| name.starts_with(shaders::HAZE_ANALYSIS_ENTRY.label))
        .expect("haze analysis module");
    let mut layouter = Layouter::default();
    layouter.update(haze_analysis.to_ctx()).unwrap();
    let size_of = |wgsl_name: &str| {
        let (handle, _) = haze_analysis
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref().map(undecorated) == Some(wgsl_name))
            .unwrap_or_else(|| panic!("{wgsl_name} is declared"));
        u64::from(layouter[handle].size)
    };
    assert_eq!(size_of("HazeCell"), HAZE_CELL_BYTES);
    assert_eq!(size_of("HazeAirlight"), HAZE_AIRLIGHT_BYTES);
    let box_max = u32_constant(haze_analysis, "HAZE_BOX_MAX").expect("WGSL declares HAZE_BOX_MAX");
    assert_eq!(box_max - 1, HAZE_BOX_SUPPORT);
    assert_eq!(
        shared_u32_constant(&modules, "HAZE_GUIDE_MAX_RADIUS"),
        HAZE_GUIDE_SUPPORT
    );
}

/// Color Fringe caps its shift at the reach its registry row reserves in the
/// export halo.
#[test]
fn color_fringe_halo_matches_its_wgsl_reach() {
    let modules = production_modules();
    assert_eq!(
        MaskEffect::ColorFringe.reach().group(),
        Some((
            ReachGroup::Creative,
            shared_u32_constant(&modules, "COLOR_FRINGE_SUPPORT")
        ))
    );
}

/// The contour grid buffer Rust allocates holds the WGSL cell.
#[test]
fn contour_cell_matches_its_wgsl_declaration() {
    let modules = production_modules();
    let (_, tone_analysis) = modules
        .iter()
        .find(|(name, _)| name.starts_with(shaders::TONE_ANALYSIS_ENTRY.label))
        .expect("tone analysis module");
    let mut layouter = Layouter::default();
    layouter.update(tone_analysis.to_ctx()).unwrap();
    let (handle, _) = tone_analysis
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref().map(undecorated) == Some("ContourCell"))
        .expect("ContourCell is declared");
    assert_eq!(u64::from(layouter[handle].size), CONTOUR_CELL_BYTES);
}

#[test]
fn remove_composite_params_match_rust_layout() {
    let rust = rust_layout!(RemoveCompositeParams { origin, extent });
    let modules = production_modules();
    let checked = modules
        .iter()
        .filter(|(name, _)| name.starts_with(shaders::REMOVE_COMPOSITE_ENTRY.label))
        .filter(|(context, module)| {
            check_struct_layout(module, "RemoveCompositeParams", &rust, context)
        })
        .count();
    assert_eq!(
        checked, 2,
        "both work formats declare RemoveCompositeParams"
    );
}

/// The export geometry pass's parameters, flags and workgroup edge match its WGSL.
#[test]
fn geometry_resample_matches_its_wgsl_declarations() {
    use super::geometry_resample::{
        GeometryResampleParams, FOOTPRINT_MIN_FLIP, GEOMETRY_AFFINE, GEOMETRY_LENS,
        GEOMETRY_WORKGROUP_EDGE,
    };
    let rust = rust_layout!(GeometryResampleParams {
        source_from_output,
        source_size,
        raster_size,
        window,
        lens_map_size,
        lens_grid_size,
        band,
        region,
        chunk,
        cell_edge,
        cells_per_row,
        flags,
        horizon_epsilon
    });
    let modules = production_modules();
    let geometry: Vec<_> = modules
        .iter()
        .filter(|(name, _)| name.starts_with(shaders::GEOMETRY_RESAMPLE_ENTRY.label))
        .collect();
    assert!(!geometry.is_empty(), "export geometry is composed");
    for (context, module) in geometry {
        assert!(
            check_struct_layout(module, "GeometryResampleParams", &rust, context),
            "{context} declares GeometryResampleParams"
        );
        assert_eq!(
            u32_constant(module, "GEOMETRY_AFFINE"),
            Some(GEOMETRY_AFFINE)
        );
        assert_eq!(u32_constant(module, "GEOMETRY_LENS"), Some(GEOMETRY_LENS));
        assert_eq!(
            u32_constant(module, "FOOTPRINT_MIN_FLIP"),
            Some(FOOTPRINT_MIN_FLIP)
        );
        for entry in &module.entry_points {
            assert_eq!(
                entry.workgroup_size,
                [GEOMETRY_WORKGROUP_EDGE, GEOMETRY_WORKGROUP_EDGE, 1],
                "{context}: {}",
                entry.name
            );
        }
    }
}

fn screaming_snake_case(camel: &str) -> String {
    let mut output = String::new();
    for (index, character) in camel.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            output.push('_');
        }
        output.push(character.to_ascii_uppercase());
    }
    output
}

/// The scene bake's and the light bake's uniforms, the lens blur buffer and
/// the pyramid match the WGSL the bakes compile (scene/, shaders/scene/).
#[test]
fn scene_bake_uniforms_and_buffers_match_their_wgsl_declarations() {
    use super::scene::camera::SceneCameraUniform;
    use super::scene::lights::{
        SceneAirLightUniform, SceneShadowLightUniform, SceneShadowLightsUniform,
    };
    use super::scene::{
        SCENE_FOCUS_BLUR_BINS, SCENE_FOCUS_BYTES, SCENE_FOCUS_DEPTH_BINS, SCENE_LAYER_LEVELS,
    };
    use super::SceneBakeUniforms;
    let modules = production_modules();
    let contracts: [(&str, RustLayout); 5] = [
        (
            "SceneBakeUniforms",
            rust_layout!(SceneBakeUniforms {
                size,
                input_size,
                camera,
                ground_plane,
                ground_horizon,
                focus_fit
            }),
        ),
        (
            "SceneCamera",
            rust_layout!(SceneCameraUniform { lens, anchors }),
        ),
        (
            "SceneShadowLight",
            rust_layout!(SceneShadowLightUniform {
                position,
                shape,
                air
            }),
        ),
        (
            "SceneShadowLights",
            rust_layout!(SceneShadowLightsUniform { lights, taps }),
        ),
        (
            "SceneAirLight",
            rust_layout!(SceneAirLightUniform {
                region,
                march,
                image
            }),
        ),
    ];
    // The old depth path declares a `SceneCamera` of its own until P7 of
    // docs/rework/DEPTH_EFFECTS_PLAN.md; only the bakes' modules are checked.
    let bake_modules: Vec<_> = modules
        .iter()
        .filter(|(context, _)| {
            [
                "scene bake",
                "calibraw scene bake",
                "scene shadow",
                "scene visibility",
                "scene air light",
            ]
            .iter()
            .any(|prefix| context.starts_with(prefix))
        })
        .collect();
    for (wgsl_name, rust) in &contracts {
        let found = bake_modules
            .iter()
            .filter(|(context, module)| check_struct_layout(module, wgsl_name, rust, context))
            .count();
        assert!(found > 0, "{wgsl_name} is declared in the bake's modules");
    }
    let constant = |name: &str| shared_u32_constant(&modules, name);
    assert_eq!(constant("SCENE_FOCUS_DEPTH_BINS"), SCENE_FOCUS_DEPTH_BINS);
    assert_eq!(constant("SCENE_FOCUS_BLUR_BINS"), SCENE_FOCUS_BLUR_BINS);
    assert_eq!(constant("SCENE_LAYER_LEVELS"), SCENE_LAYER_LEVELS);
    assert_eq!(
        constant("SCENE_SHADOW_LIGHTS") as usize,
        SCENE_SHADOW_LIGHTS
    );
    assert_eq!(
        constant("SCENE_VERTICES_PER_TRIANGLE"),
        super::scene::lights::VERTICES_PER_TRIANGLE
    );
    for (context, module) in &bake_modules {
        let mut layouter = Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        if let Some((handle, _)) = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref().map(undecorated) == Some("SceneFocus"))
        {
            assert_eq!(
                u64::from(layouter[handle].size),
                SCENE_FOCUS_BYTES,
                "{context}"
            );
        }
    }
}

/// The functions `entry` runs: its own and every one it calls, directly or
/// through others.
fn entry_functions<'a>(
    module: &'a naga::Module,
    entry: &'a naga::EntryPoint,
) -> Vec<&'a naga::Function> {
    fn calls(block: &naga::Block, found: &mut Vec<naga::Handle<naga::Function>>) {
        for statement in block.iter() {
            match statement {
                naga::Statement::Block(block) => calls(block, found),
                naga::Statement::If { accept, reject, .. } => {
                    calls(accept, found);
                    calls(reject, found);
                }
                naga::Statement::Switch { cases, .. } => {
                    for case in cases {
                        calls(&case.body, found);
                    }
                }
                naga::Statement::Loop {
                    body, continuing, ..
                } => {
                    calls(body, found);
                    calls(continuing, found);
                }
                naga::Statement::Call { function, .. } => found.push(*function),
                _ => {}
            }
        }
    }
    let mut functions = vec![&entry.function];
    let mut seen = BTreeSet::new();
    let mut pending = Vec::new();
    calls(&entry.function.body, &mut pending);
    while let Some(handle) = pending.pop() {
        if seen.insert(handle) {
            let function = &module.functions[handle];
            calls(&function.body, &mut pending);
            functions.push(function);
        }
    }
    functions
}

/// What `functions` read of the inputs `output_parts` sorts by part: uniform
/// members as `"SceneToneUniforms.grade_global"` (Rust's member names) and
/// the `MASK_ROW_*` constants of the mask slot rows they name.
fn output_inputs_read(module: &naga::Module, functions: &[&naga::Function]) -> BTreeSet<String> {
    let mut read = BTreeSet::new();
    for function in functions {
        for (_, expression) in function.expressions.iter() {
            match *expression {
                naga::Expression::Constant(constant) => {
                    let name = module.constants[constant].name.as_deref().map(undecorated);
                    if let Some(name) = name.filter(|name| name.starts_with("MASK_ROW_")) {
                        read.insert(name.to_owned());
                    }
                }
                naga::Expression::AccessIndex { base, index } => {
                    let naga::Expression::GlobalVariable(global) = function.expressions[base]
                    else {
                        continue;
                    };
                    let ty = &module.types[module.global_variables[global].ty];
                    let (Some(name), naga::TypeInner::Struct { members, .. }) =
                        (ty.name.as_deref(), &ty.inner)
                    else {
                        continue;
                    };
                    if let Some(member) = members.get(index as usize) {
                        read.insert(format!(
                            "{}.{}",
                            undecorated(name),
                            rust_member_name(member.name.as_deref().unwrap_or_default())
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    read
}

/// The members of `T`, the WGSL struct `wgsl_name` of `module`, that `strip`
/// clears entirely in a value whose bytes are all set, named as
/// [`output_inputs_read`] names them.
fn members_stripped<T: bytemuck::Pod>(
    module: &naga::Module,
    wgsl_name: &str,
    strip: impl Fn(&T) -> T,
) -> BTreeSet<String> {
    let mut full = T::zeroed();
    bytemuck::bytes_of_mut(&mut full).fill(0xff);
    let stripped = strip(&full);
    let bytes = bytemuck::bytes_of(&stripped);
    let mut layouter = Layouter::default();
    layouter.update(module.to_ctx()).expect("WGSL layout");
    let (_, ty) = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref().map(undecorated) == Some(wgsl_name))
        .unwrap_or_else(|| panic!("{wgsl_name} is declared"));
    let naga::TypeInner::Struct { members, .. } = &ty.inner else {
        panic!("{wgsl_name} is not a struct");
    };
    members
        .iter()
        .filter(|member| {
            let start = member.offset as usize;
            let end = start + layouter[member.ty].size as usize;
            bytes[start..end].iter().all(|byte| *byte == 0)
        })
        .map(|member| {
            format!(
                "{wgsl_name}.{}",
                rust_member_name(member.name.as_deref().unwrap_or_default())
            )
        })
        .collect()
}

/// The `MASK_ROW_*` constants whose rows `strip` clears entirely in a slot
/// whose bytes are all set.
fn mask_rows_stripped(strip: impl Fn(&MaskData) -> MaskData) -> BTreeSet<String> {
    let mut full = <MaskData as bytemuck::Zeroable>::zeroed();
    bytemuck::bytes_of_mut(&mut full).fill(0xff);
    let stripped = strip(&full);
    let bytes = bytemuck::bytes_of(&stripped);
    let (_, fields) = mask_data_layout();
    fields
        .into_iter()
        .filter(|(_, offset, size)| bytes[*offset..offset + size].iter().all(|byte| *byte == 0))
        .map(|(field, _, _)| format!("MASK_ROW_{}", field.to_uppercase()))
        .collect()
}

/// The part of the output stage an entry point runs in, for
/// [`inputs_left_out_of_a_part_are_not_read_by_it`]: the view node's, the
/// Creative part's (`encode_creative_part`, and the scene emission a render
/// from it starts with), or the Scene part for every other entry point, the
/// strictest, which also covers the RAW and tone stages.
fn output_part_of_entry(entry: &str) -> OutputPart {
    const CREATIVE: [&str; 6] = [
        "build_scene_emission",
        "prepare_glow_source",
        "diffuse_glow_",
        "styled_glow",
        "prepare_pixelate_blocks",
        "apply_creative_effects",
    ];
    if entry == "apply_view_node" {
        OutputPart::View
    } else if CREATIVE.iter().any(|name| entry.contains(name)) {
        OutputPart::Creative
    } else {
        OutputPart::Scene
    }
}

/// `output_parts` compares a part's inputs without what only later parts
/// read (`scene_tone_before_view` and the functions beside it), so a part
/// that read one of those would keep a stale result. No entry point may read
/// an input left out of its own part: a View-only one is read by the view
/// node alone, one the Creative part also reads by no Scene entry point.
/// Lanes cleared inside a member (Blacks in `basic_tone` and an adjustment's
/// `adjust_1`) are not covered here.
#[test]
fn inputs_left_out_of_a_part_are_not_read_by_it() {
    let modules = production_modules();
    let (_, declaring) = modules
        .iter()
        .find(|(label, _)| label.starts_with(shaders::VIEW_TRANSFORM_ENTRY.label))
        .expect("the view transform module is composed");
    let view_only: BTreeSet<String> = [
        members_stripped(declaring, "SceneToneUniforms", scene_tone_before_view),
        members_stripped(declaring, "EffectsUniforms", effects_before_view),
        mask_rows_stripped(slot_before_view),
    ]
    .into_iter()
    .flatten()
    .collect();
    let after_scene: BTreeSet<String> = [
        members_stripped(declaring, "SceneToneUniforms", scene_tone_before_creative),
        members_stripped(declaring, "EffectsUniforms", effects_before_creative),
        view_only.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert!(
        after_scene.len() > view_only.len()
            && view_only.iter().any(|row| row.starts_with("MASK_ROW_")),
        "inputs left out: {after_scene:?}"
    );
    let no_inputs = BTreeSet::new();
    for (label, module) in &modules {
        for entry in &module.entry_points {
            let part = output_part_of_entry(&entry.name);
            let left_out = match part {
                OutputPart::Scene => &after_scene,
                OutputPart::Creative => &view_only,
                OutputPart::View => &no_inputs,
            };
            let read = output_inputs_read(module, &entry_functions(module, entry));
            let wrong: Vec<_> = read.intersection(left_out).collect();
            assert!(
                wrong.is_empty(),
                "{label}: {} ({part:?} part) reads {wrong:?}, which output_parts.rs counts as read by later parts only",
                entry.name
            );
        }
    }
}

/// A pass dispatched in bands (work.rs) adds the band's first row to its
/// invocation through `dispatch_id` and to its workgroup through
/// `dispatch_group` (common.wgsl); a raw `global_invocation_id` or
/// `workgroup_id`, or `num_workgroups`, would address the first band's rows in
/// every band, and only on GPUs slow enough to band. Checks every entry point
/// `entry_reads_dispatch_band` bands.
#[test]
fn banded_entry_points_offset_their_invocations() {
    // These passes were in flight during reported device losses. Changing a declaration
    // can silently remove it from band discovery and skip the checks below.
    for name in [
        "xtrans_markesteijn_recalculate",
        "xtrans_markesteijn_homogeneity",
        "xtrans_markesteijn_accumulate",
        "xtrans_markesteijn_smooth",
        "xtrans_demosaic_finish",
        "build_styled_glow_map_level",
    ] {
        assert!(
            shaders::entry_reads_dispatch_band(name),
            "{name} must retain banded dispatch"
        );
    }
    let mut banded = 0;
    for shader in shaders::ALL_ENTRY_SHADERS {
        let text = shader.source.module_text();
        // Without comments, which may name the parameters.
        let code: String = text
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        for (start, _) in code.match_indices("fn ") {
            if start > 0 && !code[..start].ends_with('\n') {
                continue;
            }
            let Some((name, rest)) = code[start + 3..].split_once('(') else {
                continue;
            };
            if !shaders::entry_reads_dispatch_band(name) {
                continue;
            }
            let (parameters, body) = rest
                .split_once('{')
                .unwrap_or_else(|| panic!("{name} has a body"));
            let mut depth = 1;
            let end = body
                .char_indices()
                .find_map(|(index, character)| {
                    match character {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(index)
                })
                .unwrap_or_else(|| panic!("{name}'s body ends"));
            let body = &body[..end];
            assert!(
                !parameters.contains("num_workgroups"),
                "{name} is banded and reads num_workgroups"
            );
            assert!(
                !parameters.contains("workgroup_id")
                    || parameters.contains("@builtin(workgroup_id) dispatch_group:"),
                "{name} is banded and names its workgroup_id other than dispatch_group"
            );
            assert_eq!(
                body.matches("dispatch_gid").count(),
                body.matches("dispatch_id(dispatch_gid)").count(),
                "{name} is banded and reads dispatch_gid other than through dispatch_id"
            );
            assert_eq!(
                body.matches("dispatch_group").count(),
                2 * body.matches("dispatch_group(dispatch_group,").count(),
                "{name} is banded and reads dispatch_group other than through Common::dispatch_group"
            );
            banded += 1;
        }
    }
    assert!(banded > 0, "no banded entry point found");
}

/// A program compiled for an effect set defines `EFFECT_<NAME>` only for the
/// effects its entry point dispatches (`effect_programs::dispatched_effects`).
/// An `#ifdef` of an effect no program dispatches is never compiled into such
/// a program, so the effect would disappear once that program is ready.
#[test]
fn every_effect_def_is_dispatched() {
    let dispatched = production_modules()
        .iter()
        .flat_map(|(_, module)| module.entry_points.iter())
        .filter_map(|entry| dispatched_effects(&entry.name))
        .fold(0, |bits, set| bits | set.bits());
    let mut sources = vec![std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/shaders"
    ))];
    let mut tested = BTreeSet::new();
    while let Some(path) = sources.pop() {
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).expect("read the shader directory") {
                sources.push(entry.expect("shader directory entry").path());
            }
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "wgsl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read a shader");
        for line in text.lines() {
            let line = line.trim_start();
            let Some(def) = ["#ifdef ", "#ifndef ", "#if "]
                .iter()
                .find_map(|directive| line.strip_prefix(directive))
                .map(|rest| rest.split_whitespace().next().unwrap_or_default())
                .filter(|def| def.starts_with("EFFECT_"))
            else {
                continue;
            };
            tested.insert((def.to_owned(), path.display().to_string()));
        }
    }
    assert!(!tested.is_empty(), "no EFFECT_ def found");
    for (def, path) in tested {
        let effect = MaskEffect::ALL
            .into_iter()
            .find(|effect| effect_def(*effect) == def)
            .unwrap_or_else(|| panic!("{path} tests {def}, which names no effect"));
        assert!(
            dispatched & EffectSet::from_effects([effect]).bits() != 0,
            "{path} tests {def}, but no program compiled per effect set dispatches {effect:?}"
        );
    }
}
