//! Rust/WGSL buffer-layout and shared-constant contracts.
//!
//! The checks inspect the modules `ShaderManager` composes in production, so
//! imports and source specialization match what the GPU compiles. A
//! `#[repr(C)]`/`bytemuck::Pod` Rust type does not prove WGSL compatibility:
//! Rust aligns `[f32; 4]` to 4 bytes, while a WGSL `vec4<f32>` aligns to 16 and
//! a uniform `mat3x3<f32>` has a 16-byte column stride. Each member's byte
//! offset and size are therefore compared explicitly.

use super::{
    effect_lanes, processing_work_format, shader_manager::ShaderManager, shaders,
    work_shader_source, CameraUniforms, CfaKind, EffectsUniforms, MaskData, PackedPointColor,
    ProcessingQuality, RemoveCompositeParams, SceneToneUniforms, ViewTransformMode,
    FILM_MODE_MONOCHROME, FILM_MODE_POSITIVE, HAZE_AIRLIGHT_BYTES, HAZE_CELL_BYTES, HAZE_GRID_LONG,
    IMAGE_LIGHT_BANDS, IMAGE_LIGHT_GRID_LONG, MASK_EFFECT_ID_SHIFT, MAX_RENDER_MASK_SLOTS,
    RELIGHT_SHADOW_MAP_CHANNELS, TONE_HISTOGRAM_BIN_COUNT, TONE_STATS_SIZE_BYTES,
};
use super::{DETAIL_PYRAMID_LEVELS, DETAIL_PYRAMID_TEXTURE_LEVELS};
use crate::pipeline::MaskEffect;
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
                    .compose_naga_module(text.as_ref(), shader.source.file_name)
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
    let contracts: [(&str, RustLayout); 5] = [
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
            }),
        ),
        (
            "SceneToneUniforms",
            rust_layout!(SceneToneUniforms {
                exposure,
                saturation,
                vibrance,
                scene_depth_present,
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
            "MaskData",
            rust_layout!(MaskData {
                metadata,
                adjust_0,
                adjust_1,
                adjust_2,
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

#[test]
fn shared_buffer_bindings_match_the_rust_bind_group_layouts() {
    // (global, group, binding, address space) as bound by `create_bind_group_layouts`.
    let expected = [
        ("camera_uniforms", 0, 0, naga::AddressSpace::Uniform),
        ("scene_tone_uniforms", 1, 0, naga::AddressSpace::Uniform),
        ("effects_uniforms", 2, 0, naga::AddressSpace::Uniform),
        (
            "mask_data",
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
    ] {
        assert_eq!(constant(name), mode.shader_value(), "{name}");
    }
    assert_eq!(
        constant("TONE_HISTOGRAM_BIN_COUNT"),
        TONE_HISTOGRAM_BIN_COUNT
    );
    assert_eq!(constant("FILM_MODE_POSITIVE"), FILM_MODE_POSITIVE);
    assert_eq!(constant("FILM_MODE_MONOCHROME"), FILM_MODE_MONOCHROME);
    // The image-light grid buffer and textures are sized from these.
    assert_eq!(constant("IMAGE_LIGHT_GRID_LONG"), IMAGE_LIGHT_GRID_LONG);
    assert_eq!(constant("IMAGE_LIGHT_BANDS"), IMAGE_LIGHT_BANDS);
    // So are the haze grid's.
    assert_eq!(constant("HAZE_GRID_LONG"), HAZE_GRID_LONG);
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

#[test]
fn effect_parameter_lanes_match_their_wgsl_names() {
    let modules = production_modules();
    let (_, creative) = modules
        .iter()
        .find(|(name, _)| name.starts_with(shaders::CREATIVE_EFFECTS_ENTRY.label))
        .expect("creative effects module");
    let constant = |name: &str| {
        u32_constant(creative, name).unwrap_or_else(|| panic!("WGSL declares {name}")) as usize
    };
    let mut expected = BTreeSet::new();
    let mut occupied = BTreeSet::new();
    for (effect, name, lane) in effect_lanes::named_lanes() {
        let wgsl_name = format!("{}_{name}_LANE", effect.to_uppercase());
        assert_eq!(constant(&wgsl_name), lane, "{wgsl_name}");
        for lane in lane..lane + effect_lanes::lane_width(name) {
            assert!(lane < 12, "{wgsl_name} reaches past adjust_2");
            assert!(
                occupied.insert((effect, lane)),
                "{wgsl_name} overlaps lane {lane}"
            );
        }
        expected.insert(wgsl_name);
    }
    for (name, lane) in effect_lanes::options::named_options() {
        let wgsl_name = format!("{name}_OPTION");
        assert_eq!(constant(&wgsl_name), lane, "{wgsl_name}");
        // Lane 0 is local Halation in every slot.
        assert!((1..4).contains(&lane), "{wgsl_name}");
        expected.insert(wgsl_name);
    }
    assert_eq!(
        constant("RELIGHT_SHADOW_MAP_CHANNELS"),
        RELIGHT_SHADOW_MAP_CHANNELS
    );
    // Every lane the shaders name is packed by name in Rust too.
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
