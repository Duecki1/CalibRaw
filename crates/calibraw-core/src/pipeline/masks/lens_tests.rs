use super::*;

const FULL: [u32; 2] = [241, 161];
const FRAME: [u32; 4] = [0, 0, FULL[0], FULL[1]];

fn nonlinear_lens() -> LensGeometryMap {
    // The map resolution and original image dimensions deliberately differ from
    // the raster. Both axes bend, and horizontal displacement varies with row.
    let [width, height] = [481, 321];
    let [gw, gh] = [33, 25];
    let coordinates = (0..gh)
        .flat_map(|y| {
            (0..gw).map(move |x| {
                let u = x as f32 / (gw - 1) as f32;
                let v = y as f32 / (gh - 1) as f32;
                [
                    (u + 0.8 * u * (1.0 - u) * (2.0 * v - 1.0).powi(2)) * (width - 1) as f32,
                    (v + 0.3 * v * (1.0 - v) * (2.0 * u - 1.0)) * (height - 1) as f32,
                ]
            })
        })
        .collect();
    LensGeometryMap::new(width, height, gw, gh, coordinates).unwrap()
}

fn linear() -> MaskComponent {
    let mut component = MaskComponent::new(MaskKind::Linear, MaskCombineMode::Add);
    component.geometry = MaskGeometry::Linear {
        start: [0.2, 0.5],
        end: [0.8, 0.5],
        feather: 1.0,
        initialized: true,
    };
    component
}

fn radial() -> MaskComponent {
    let mut component = MaskComponent::new(MaskKind::Radial, MaskCombineMode::Add);
    component.geometry = MaskGeometry::Radial {
        center: [0.46, 0.52],
        radius: [0.24, 0.17],
        rotation: 0.7,
        feather: 0.4,
        initialized: true,
    };
    component
}

fn stack(components: Vec<MaskComponent>) -> MaskStack {
    let mut stack = MaskStack::default();
    stack.add_mask(MaskKind::Fullscreen).unwrap();
    stack.masks[0].components = components;
    stack
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Simulate lens rendering with the same normalized UV convention as the UI.
fn rendered_sample(pixels: &[u8], lens: &LensGeometryMap, uv: [f32; 2]) -> f32 {
    let last = FULL.map(|value| (value - 1) as f32);
    let native =
        lens.source_position_for_raster(uv[0] * last[0], uv[1] * last[1], FULL[0], FULL[1]);
    let p = std::array::from_fn::<_, 2, _>(|axis| {
        (native[axis] / last[axis] * FULL[axis] as f32 - 0.5).clamp(0.0, last[axis])
    });
    let [x, y] = p.map(|value| value.floor() as usize);
    let x1 = (x + 1).min(FULL[0] as usize - 1);
    let y1 = (y + 1).min(FULL[1] as usize - 1);
    let [tx, ty] = [p[0] - x as f32, p[1] - y as f32];
    let load = |x, y| pixels[y * FULL[0] as usize + x] as f32;
    let top = load(x, y) * (1.0 - tx) + load(x1, y) * tx;
    let bottom = load(x, y1) * (1.0 - tx) + load(x1, y1) * tx;
    top * (1.0 - ty) + bottom * ty
}

#[test]
fn linear_gradient_is_straight_after_nonlinear_lens_rendering() {
    let lens = nonlinear_lens();
    let stack = stack(vec![linear()]);
    let pixels = stack.rasterize_layer_region(0, FULL, FRAME, FULL, Some(&lens));
    let half = stack.rasterize_layer_region_f16(0, FULL, FRAME, FULL, Some(&lens));
    let native = stack.rasterize_layer_region(0, FULL, FRAME, FULL, None);
    // Check individual native samples more precisely than byte quantization to
    // catch extent-vs-last-pixel and half-pixel convention regressions.
    for [x, y] in [[51, 17], [97, 43], [151, 113], [179, 141]] {
        let uv = [
            (x as f32 + 0.5) / FULL[0] as f32,
            (y as f32 + 0.5) / FULL[1] as f32,
        ];
        let corrected = lens.corrected_position_for_raster(
            uv[0] * (FULL[0] - 1) as f32,
            uv[1] * (FULL[1] - 1) as f32,
            FULL[0],
            FULL[1],
        );
        let expected = 1.0 - smooth((corrected[0] / (FULL[0] - 1) as f32 - 0.2) / 0.6);
        let actual = f16::from_bits(half[(y * FULL[0] + x) as usize]).to_f32();
        assert!((actual - expected).abs() < 0.0003);
    }
    let mut largest_native_error = 0.0_f32;
    for u in [0.3, 0.45, 0.6, 0.7] {
        let expected = (1.0 - smooth((u - 0.2) / 0.6)) * 255.0;
        for v in [0.15, 0.35, 0.55, 0.85] {
            let actual = rendered_sample(&pixels, &lens, [u, v]);
            assert!(
                (actual - expected).abs() < 1.5,
                "UV [{u}, {v}]: {actual} != {expected}"
            );
            largest_native_error = largest_native_error
                .max((rendered_sample(&native, &lens, [u, v]) - expected).abs());
        }
    }
    assert!(
        largest_native_error > 30.0,
        "fixture must expose a bent native gradient"
    );
}

#[test]
fn rotated_radial_remains_an_ellipse_in_corrected_pixel_metric() {
    let lens = nonlinear_lens();
    let stack = stack(vec![radial()]);
    let pixels = stack.rasterize_layer_region(0, FULL, FRAME, FULL, Some(&lens));
    let native = stack.rasterize_layer_region(0, FULL, FRAME, FULL, None);
    let mut largest_native_error = 0.0_f32;
    let (sin, cos) = 0.7_f32.sin_cos();
    for distance in [0.4, 0.7, 0.85, 1.08] {
        let expected = (1.0 - smooth((distance - 0.608) / 0.392)) * 255.0;
        for angle in (0..16).map(|i| i as f32 * TAU / 16.0) {
            let x = distance * angle.cos() * 0.24 * FULL[0] as f32;
            let y = distance * angle.sin() * 0.17 * FULL[1] as f32;
            let uv = [
                0.46 + (cos * x - sin * y) / FULL[0] as f32,
                0.52 + (sin * x + cos * y) / FULL[1] as f32,
            ];
            let actual = rendered_sample(&pixels, &lens, uv);
            assert!(
                (actual - expected).abs() < 2.5,
                "UV {uv:?}: {actual} != {expected}"
            );
            largest_native_error =
                largest_native_error.max((rendered_sample(&native, &lens, uv) - expected).abs());
        }
    }
    assert!(
        largest_native_error > 30.0,
        "fixture must expose a deformed native ellipse"
    );
}

#[test]
fn lens_crop_and_full_rasters_agree_at_native_and_reduced_extents() {
    let lens = nonlinear_lens();
    let mut gradient = linear();
    gradient.geometry = MaskGeometry::Linear {
        start: [0.15, 0.25],
        end: [0.8, 0.7],
        feather: 0.7,
        initialized: true,
    };
    let mut stack = stack(vec![radial(), gradient]);
    stack.masks[0].components[1].combine = MaskCombineMode::Intersect;
    stack.masks[0].components[1].invert = true;
    stack.masks[0].invert = true;
    stack.masks[0].opacity = 0.63;
    // Crops have a different aspect from the full image and include samples
    // whose corrected UVs lie outside the native crop.
    for (full_size, full_extent, region, extent, offset) in [
        (FULL, FULL, [53, 17, 91, 107], [91, 107], [53, 17]),
        ([240, 160], [120, 80], [80, 32, 80, 80], [40, 40], [40, 16]),
    ] {
        let frame = [0, 0, full_size[0], full_size[1]];
        let full = stack.rasterize_layer_region(0, full_extent, frame, full_size, Some(&lens));
        let crop = stack.rasterize_layer_region(0, extent, region, full_size, Some(&lens));
        let full_half =
            stack.rasterize_layer_region_f16(0, full_extent, frame, full_size, Some(&lens));
        let crop_half = stack.rasterize_layer_region_f16(0, extent, region, full_size, Some(&lens));
        let full_component =
            stack.rasterize_component_region(0, 0, full_extent, frame, full_size, Some(&lens));
        let crop_component =
            stack.rasterize_component_region(0, 0, extent, region, full_size, Some(&lens));
        for y in 0..extent[1] as usize {
            for x in 0..extent[0] as usize {
                let i = y * extent[0] as usize + x;
                let j = (y + offset[1] as usize) * full_extent[0] as usize + x + offset[0] as usize;
                assert!(crop[i].abs_diff(full[j]) <= 1);
                assert!(crop_component[i].abs_diff(full_component[j]) <= 1);
                assert!(
                    (f16::from_bits(crop_half[i]).to_f32() - f16::from_bits(full_half[j]).to_f32())
                        .abs()
                        <= 0.001
                );
            }
        }
    }
}

#[test]
fn region_without_lens_matches_existing_apis_exactly() {
    let stack = stack(vec![radial(), linear()]);
    for (extent, region) in [([53, 47], FRAME), ([37, 19], [23, 41, 113, 59])] {
        let cropped =
            stack.cropped_for_region(region[0], region[1], region[2], region[3], FULL[0], FULL[1]);
        assert_eq!(
            stack.rasterize_layer_region(0, extent, region, FULL, None),
            cropped.rasterize_layer(0, extent[0], extent[1], region[2], region[3])
        );
        assert_eq!(
            stack.rasterize_layer_region_f16(0, extent, region, FULL, None),
            cropped.rasterize_layer_f16(0, extent[0], extent[1], region[2], region[3])
        );
        for component in 0..2 {
            assert_eq!(
                stack.rasterize_component_region(0, component, extent, region, FULL, None),
                cropped.rasterize_component_layer(
                    0, component, extent[0], extent[1], region[2], region[3]
                )
            );
        }
    }
    assert_eq!(
        stack.rasterize_layer_region(0, [53, 47], FRAME, FULL, None),
        stack.rasterize_layer(0, 53, 47, FULL[0], FULL[1])
    );
}

fn native_components() -> Vec<MaskComponent> {
    let mut brush = MaskComponent::new(MaskKind::Brush, MaskCombineMode::Add);
    if let MaskGeometry::Brush { dabs, .. } = &mut brush.geometry {
        dabs.push(BrushDab {
            center: [0.6, 0.4],
            opacity: 0.7,
            size: 0.3,
            feather: 0.5,
        });
    }
    let mut ai = MaskComponent::new(MaskKind::Subject, MaskCombineMode::Add);
    ai.geometry = MaskGeometry::Ai {
        mask: MaskImage::new(3, 2, vec![0, 100, 255, 200, 30, 170]),
        grow: 0.0,
        feather: 0.0,
    };
    let mut path = MaskComponent::new(MaskKind::Path, MaskCombineMode::Add);
    path.geometry = MaskGeometry::Path {
        points: [[0.1, 0.2], [0.8, 0.3], [0.6, 0.9]]
            .map(PathPoint::corner)
            .to_vec(),
        grow: 0.0,
        feather: 0.0,
    };
    vec![brush, ai, path]
}

#[test]
fn native_geometries_are_unchanged_even_in_a_mixed_lens_layer() {
    let lens = nonlinear_lens();
    let mut components = vec![radial(), linear()];
    components.extend(native_components());
    let mixed = stack(components);
    for region in [FRAME, [31, 27, 137, 93]] {
        for component in 2..5 {
            assert_eq!(
                mixed.rasterize_component_region(0, component, [71, 53], region, FULL, Some(&lens)),
                mixed.rasterize_component_region(0, component, [71, 53], region, FULL, None)
            );
        }
        let native = stack(native_components());
        assert_eq!(
            native.rasterize_layer_region(0, [71, 53], region, FULL, Some(&lens)),
            native.rasterize_layer_region(0, [71, 53], region, FULL, None)
        );
    }
    assert_ne!(
        mixed.rasterize_component_region(0, 0, [71, 53], FRAME, FULL, Some(&lens)),
        mixed.rasterize_component_region(0, 0, [71, 53], FRAME, FULL, None)
    );
}

#[test]
fn lens_layer_preserves_composition_inversion_and_opacity() {
    let lens = nonlinear_lens();
    let mut components = vec![radial(), native_components().remove(0), linear(), radial()];
    components[2].combine = MaskCombineMode::Subtract;
    components[2].invert = true;
    components[3].combine = MaskCombineMode::Intersect;
    components[3].invert = true;
    let mut stack = stack(components);
    stack.masks[0].invert = true;
    stack.masks[0].opacity = 0.61;
    let extent = [67, 43];
    let region = [33, 19, 151, 123];
    let components: Vec<_> = (0..4)
        .map(|i| stack.rasterize_component_region(0, i, extent, region, FULL, Some(&lens)))
        .collect();
    let bytes = stack.rasterize_layer_region(0, extent, region, FULL, Some(&lens));
    let half = stack.rasterize_layer_region_f16(0, extent, region, FULL, Some(&lens));
    for i in 0..bytes.len() {
        let c = |j: usize| components[j][i] as f32 / 255.0;
        let expected = (1.0 - c(0).max(c(1)) * (1.0 - c(2)) * c(3)) * 0.61;
        assert!((bytes[i] as f32 / 255.0 - expected).abs() < 0.006);
        assert!((f16::from_bits(half[i]).to_f32() - expected).abs() < 0.004);
    }
    stack.masks[0]
        .components
        .iter_mut()
        .for_each(|c| c.enabled = false);
    assert!(stack
        .rasterize_layer_region(0, extent, region, FULL, Some(&lens))
        .iter()
        .all(|&v| v == 0));
    stack.masks[0].components[0].enabled = true;
    stack.masks[0].components[0].combine = MaskCombineMode::Subtract;
    stack.masks[0].invert = false;
    assert!(stack
        .rasterize_layer_region(0, extent, region, FULL, Some(&lens))
        .iter()
        .all(|&v| v == 0));
}

#[test]
fn identity_and_degenerate_lens_regions_are_well_defined() {
    let lens = LensGeometryMap::new(
        19,
        11,
        2,
        2,
        vec![[0.0, 0.0], [18.0, 0.0], [0.0, 10.0], [18.0, 10.0]],
    )
    .unwrap();
    let stack = stack(vec![radial(), linear()]);
    for full in [FULL, [1, 161], [241, 1], [1, 1]] {
        let region = [0, 0, full[0], full[1]];
        let with = stack.rasterize_layer_region(0, [37, 29], region, full, Some(&lens));
        let without = stack.rasterize_layer_region(0, [37, 29], region, full, None);
        assert!(with.iter().zip(&without).all(|(&a, &b)| a.abs_diff(b) <= 1));
    }
    assert!(stack
        .rasterize_layer_region(0, [0, 29], FRAME, FULL, Some(&lens))
        .is_empty());
    assert!(stack
        .rasterize_layer_region_f16(0, [37, 0], FRAME, FULL, Some(&lens))
        .is_empty());
    assert!(stack
        .rasterize_component_region(0, 0, [0, 0], FRAME, FULL, Some(&lens))
        .is_empty());
    assert_eq!(
        stack.rasterize_layer_region(9, [3, 2], FRAME, FULL, Some(&lens)),
        vec![0; 6]
    );
    assert_eq!(
        stack.rasterize_component_region(0, 9, [3, 2], FRAME, FULL, Some(&lens)),
        vec![0; 6]
    );
}
