use super::*;

// Measure the displayed contour in source pixels, independently of the mask's
// distance transform and of the preview raster's aspect ratio.
struct CoverageView {
    pixels: Vec<f32>,
    raster: [u32; 2],
    region: [f32; 4],
    background: bool,
}

impl CoverageView {
    fn sample(&self, point: [f32; 2]) -> f32 {
        let [width, height] = self.raster;
        let x = ((point[0] - self.region[0]) / self.region[2] * width as f32 - 0.5)
            .clamp(0.0, (width - 1) as f32);
        let y = ((point[1] - self.region[1]) / self.region[3] * height as f32 - 0.5)
            .clamp(0.0, (height - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
        let at = |x, y| self.pixels[(y * width + x) as usize];
        let top = at(x0, y0) * (1.0 - x.fract()) + at(x1, y0) * x.fract();
        let bottom = at(x0, y1) * (1.0 - x.fract()) + at(x1, y1) * x.fract();
        let value = top * (1.0 - y.fract()) + bottom * y.fract();
        if self.background {
            1.0 - value
        } else {
            value
        }
    }

    fn crossings(&self, center: [f32; 2], normal: [f32; 2]) -> [f32; 3] {
        let sample = |distance: f32| {
            self.sample([
                center[0] + normal[0] * distance,
                center[1] + normal[1] * distance,
            ])
        };
        assert!(
            sample(-64.0) < 0.05 && sample(64.0) > 0.95,
            "profile must bracket an isolated edge with enough crop margin"
        );
        [0.1, 0.5, 0.9].map(|level| {
            let mut previous = sample(-64.0);
            for step in 1..=512 {
                let distance = -64.0 + step as f32 * 0.25;
                let value = sample(distance);
                if previous <= level && value >= level && value > previous {
                    return distance - 0.25 + 0.25 * (level - previous) / (value - previous);
                }
                previous = value;
            }
            panic!("missing coverage crossing at {level}");
        })
    }
}

fn fixture(kind: MaskKind, image: [u32; 2], grow: f32, feather: f32, diagonal: bool) -> MaskStack {
    let [width, height] = image;
    let points = if diagonal {
        vec![[0.275, 0.1], [0.475, 0.9], [0.85, 0.9], [0.85, 0.1]]
    } else {
        vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]]
    };
    let mut matte = Vec::with_capacity((width * height) as usize);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            let inside = if diagonal {
                (0.1..0.9).contains(&v) && u >= 0.25 + 0.25 * v && u < 0.85
            } else {
                (0.25..0.75).contains(&u) && (0.25..0.75).contains(&v)
            };
            let value = if inside { 255 } else { 0 };
            matte.push(value);
            // Keep the selected luminance away from either range endpoint,
            // including when its value-domain feather is zero.
            let rgb = if inside { 230 } else { 0 };
            rgba.extend_from_slice(&[rgb, rgb, rgb, 255]);
        }
    }
    let mut stack = MaskStack::default();
    stack.add_mask(kind);
    let component = stack.selected_component_mut().unwrap();
    component.geometry = match kind {
        MaskKind::Subject | MaskKind::Background | MaskKind::Sky => MaskGeometry::Ai {
            mask: MaskImage::new(width, height, matte),
            grow,
            feather,
        },
        MaskKind::Object => MaskGeometry::Object {
            mask: MaskImage::new(width, height, matte),
            grow,
            feather,
            brush_size: 0.1,
            edge_refine: 0.0,
            strokes: vec![],
        },
        MaskKind::Path => MaskGeometry::Path {
            points: points.into_iter().map(PathPoint::corner).collect(),
            grow,
            feather,
        },
        MaskKind::LuminanceRange => MaskGeometry::LuminanceRange {
            source: MaskRgbImage::new(width, height, rgba),
            low: 0.6,
            high: 0.95,
            grow,
            feather,
        },
        MaskKind::ColorRange => MaskGeometry::ColorRange {
            source: MaskRgbImage::new(width, height, rgba),
            sample: [230.0 / 255.0; 3],
            tolerance: 0.1,
            grow,
            feather,
            sampled: true,
        },
        _ => unreachable!(),
    };
    stack
}

fn views(stack: &MaskStack, kind: MaskKind, image: [u32; 2]) -> [CoverageView; 3] {
    let [width, height] = image;
    let edge = width.max(height) / 2;
    // The viewport changes both the aspect and the source pixels per texel.
    let crop = [width / 8, height / 8, width * 3 / 4, height * 7 / 8];
    let crop_extent = crate::pipeline::mask_region_texture_extent(crop, edge);
    let cropped = stack.cropped_for_region(crop[0], crop[1], crop[2], crop[3], width, height);
    let full_region = [0.0, 0.0, width as f32, height as f32];
    let render = |stack: &MaskStack, raster: [u32; 2], region: [f32; 4]| CoverageView {
        pixels: stack.rasterize_layer_coverage(
            0,
            raster[0],
            raster[1],
            region[2] as u32,
            region[3] as u32,
        ),
        raster,
        region,
        background: kind == MaskKind::Background,
    };
    [
        render(stack, [edge, edge], full_region),
        render(stack, [width / 2, height / 2], full_region),
        render(&cropped, crop_extent, crop.map(|v| v as f32)),
    ]
}

fn assert_profile_consistency(
    views: &[CoverageView; 3],
    center: [f32; 2],
    normal: [f32; 2],
    context: &str,
) {
    let reference = views[1].crossings(center, normal);
    for (label, view) in [("square fit atlas", &views[0]), ("zoom crop", &views[2])] {
        let actual = view.crossings(center, normal);
        for (index, level) in [0.1, 0.5, 0.9].into_iter().enumerate() {
            assert!(
                (actual[index] - reference[index]).abs() <= 3.0,
                "{context}, {label}, coverage {level}: source crossing {} vs {}",
                actual[index],
                reference[index]
            );
        }
        let actual_width = actual[2] - actual[0];
        let reference_width = reference[2] - reference[0];
        assert!((actual_width - reference_width).abs() <= 4.0,
            "{context}, {label}: 10–90% feather width {actual_width} vs {reference_width} source pixels");
    }
}

#[test]
fn straight_edge_grow_and_feather_survive_fit_to_zoom() {
    let kinds = [
        MaskKind::Subject,
        MaskKind::Background,
        MaskKind::Sky,
        MaskKind::Object,
        MaskKind::Path,
        MaskKind::LuminanceRange,
        MaskKind::ColorRange,
    ];
    // Include unchanged edges, both grow signs, feather alone, and their interaction.
    // At two source pixels per aspect-raster pixel, the tolerances above allow
    // interpolation/contour quantization but reject the old aspect-dependent widths.
    for image in [[768, 512], [512, 768], [512, 512]] {
        for kind in kinds {
            for (grow, feather) in [
                (0.0, 0.0),
                (1.0, 0.0),
                (-1.0, 0.0),
                (0.0, 0.8),
                (1.0, 0.8),
                (-1.0, 0.8),
            ] {
                let stack = fixture(kind, image, grow, feather, false);
                let views = views(&stack, kind, image);
                let context = format!("{kind:?}, {image:?}, grow={grow}, feather={feather}");
                let [w, h] = image.map(|v| v as f32);
                assert_profile_consistency(&views, [w * 0.25, h * 0.5], [1.0, 0.0], &context);
                assert_profile_consistency(&views, [w * 0.5, h * 0.25], [0.0, 1.0], &context);
            }
        }
    }
}

#[test]
fn diagonal_contour_grow_and_feather_survive_fit_to_zoom() {
    for image in [[768, 512], [512, 768]] {
        let [w, h] = image.map(|v| v as f32);
        let slope = 0.25 * w / h;
        let length = (1.0 + slope * slope).sqrt();
        let normal = [1.0 / length, -slope / length];
        for kind in [MaskKind::Subject, MaskKind::Object, MaskKind::Path] {
            for grow in [-1.0, 0.0, 1.0] {
                let stack = fixture(kind, image, grow, 0.8, true);
                let context = format!("diagonal {kind:?}, {image:?}, grow={grow}");
                assert_profile_consistency(
                    &views(&stack, kind, image),
                    [w * 0.375, h * 0.5],
                    normal,
                    &context,
                );
            }
        }
    }
}

#[test]
fn range_masks_keep_their_sampling_when_grow_leaves_zero() {
    for [width, height] in [[128, 64], [64, 128]] {
        let mut rgba = vec![0; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                // A thin band exposes sampling changes, while the rectangle
                // provides a stable selected core for the distance transform.
                let selected = if width > height { y == 15 } else { x == 15 }
                    || (x >= width / 3
                        && x < width * 2 / 3
                        && y >= height / 3
                        && y < height * 2 / 3);
                let value = if selected { 220 } else { 0 };
                rgba[((y * width + x) * 4) as usize..][..4]
                    .copy_from_slice(&[value, value, value, 255]);
            }
        }
        let source = MaskRgbImage::new(width, height, rgba).unwrap();
        for kind in [MaskKind::LuminanceRange, MaskKind::ColorRange] {
            let mut stack = MaskStack::default();
            stack.add_mask(kind);
            stack.selected_component_mut().unwrap().geometry = match kind {
                MaskKind::LuminanceRange => MaskGeometry::LuminanceRange {
                    source: Some(source.clone()),
                    low: 0.3,
                    high: 0.9,
                    grow: 0.0,
                    feather: 0.0,
                },
                MaskKind::ColorRange => MaskGeometry::ColorRange {
                    source: Some(source.clone()),
                    sample: [220.0 / 255.0; 3],
                    tolerance: 0.05,
                    grow: 0.0,
                    feather: 0.0,
                    sampled: true,
                },
                _ => unreachable!(),
            };
            let original = stack.rasterize_layer_coverage(0, 64, 64, width, height);
            assert!(original.iter().any(|value| *value > 0.99));
            for grow_value in [-0.0001, 0.0001] {
                match &mut stack.selected_component_mut().unwrap().geometry {
                    MaskGeometry::LuminanceRange { grow, .. }
                    | MaskGeometry::ColorRange { grow, .. } => *grow = grow_value,
                    _ => unreachable!(),
                }
                let grown = stack.rasterize_layer_coverage(0, 64, 64, width, height);
                let max_change = original
                    .iter()
                    .zip(&grown)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f32::max);
                assert!(
                    max_change < 0.001,
                    "{kind:?}, {width}x{height}, grow={grow_value}: coverage jumps by {max_change}"
                );
            }
        }
    }
}
