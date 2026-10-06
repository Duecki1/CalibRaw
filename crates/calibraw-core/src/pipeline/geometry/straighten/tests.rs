use super::segments::{detect_line_segments, detect_line_segments_multiscale};
use super::*;
use crate::pipeline::raw_loader::{CompactPixelMap, LoadedRaw};

/// Axis-aligned rectangles (centre x, centre y, half width, half height) in a frame centred on
/// the origin, drawn dark on a light gradient: a horizon band, window rows and building edges.
const SCENE: &[[f32; 4]] = &[
    [0.0, 0.10, 0.48, 0.012],
    [-0.30, 0.0, 0.010, 0.30],
    [0.32, 0.02, 0.010, 0.28],
    [-0.05, -0.18, 0.22, 0.008],
    [0.05, 0.25, 0.30, 0.006],
];

/// Renders `SCENE` rotated by `tilt_degrees` (positive turns content clockwise on screen, the
/// y-down convention of `rotation_degrees`), anti-aliased by signed distance.
fn render_scene(width: usize, height: usize, tilt_degrees: f32) -> Vec<f32> {
    let (sin, cos) = tilt_degrees.to_radians().sin_cos();
    let scale = width.max(height) as f32;
    let mut values = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let px = (x as f32 + 0.5 - width as f32 * 0.5) / scale;
            let py = (y as f32 + 0.5 - height as f32 * 0.5) / scale;
            // Undo the tilt to find the point in the scene frame.
            let sx = cos * px + sin * py;
            let sy = -sin * px + cos * py;
            let mut coverage = 0.0f32;
            for &[cx, cy, hw, hh] in SCENE {
                let distance = ((sx - cx).abs() - hw).max((sy - cy).abs() - hh) * scale;
                coverage = coverage.max((0.5 - distance).clamp(0.0, 1.0));
            }
            let background = 0.55 + 0.25 * (y as f32 / height as f32);
            values.push(background * (1.0 - 0.85 * coverage));
        }
    }
    values
}

fn raster_raw(width: usize, height: usize, luminance: &[f32]) -> LoadedRaw {
    let rgb = luminance.iter().flat_map(|&v| [v, v, v]).collect();
    LoadedRaw::from_scene_linear_rec2020(width as u32, height as u32, rgb).unwrap()
}

/// An RGGB mosaic with unequal channel gains (no white balance) and a black offset.
fn bayer_raw(width: usize, height: usize, luminance: &[f32]) -> LoadedRaw {
    const BLACK: f32 = 256.0;
    const GAINS: [f32; 4] = [0.45, 1.0, 1.0, 0.6];
    let pattern = [0u8, 1, 1, 2];
    let pixels = (0..width * height)
        .map(|index| {
            let (x, y) = (index % width, index / width);
            let channel = pattern[(y % 2) * 2 + x % 2] as usize;
            (BLACK + luminance[index] * GAINS[channel] * 12_000.0) as u16
        })
        .collect();
    let base = raster_raw(1, 1, &[0.0]);
    let (width, height) = (width as u32, height as u32);
    LoadedRaw {
        white_levels: [16_000.0; 4],
        ..base.derive_with(
            width,
            height,
            pixels,
            CompactPixelMap::repeating(width, height, 2, 2, pattern.to_vec()),
            CompactPixelMap::repeating(width, height, 1, 1, vec![BLACK]),
        )
    }
}

fn estimate(raw: &LoadedRaw, geometry: GeometryTransform) -> Option<StraightenEstimate> {
    let image = LineAnalysisImage::from_raw(raw).unwrap();
    estimate_straighten_rotation(&image, geometry)
}

#[test]
fn rotation_cancels_scene_tilt() {
    for tilt in [-7.5f32, -0.6, 0.0, 2.3, 11.0] {
        let raw = raster_raw(1200, 800, &render_scene(1200, 800, tilt));
        let estimate = estimate(&raw, GeometryTransform::default()).unwrap();
        assert!(
            (estimate.rotation_degrees + tilt).abs() < 0.1,
            "tilt {tilt}: estimated {}",
            estimate.rotation_degrees
        );
        assert!(estimate.supporting_lines >= 4);
    }
}

#[test]
fn bayer_mosaic_bins_whole_cfa_periods() {
    let tilt = 3.2;
    let raw = bayer_raw(2400, 1600, &render_scene(2400, 1600, tilt));
    let image = LineAnalysisImage::from_raw(&raw).unwrap();
    assert_eq!((image.width(), image.height()), (1200, 800));
    let estimate = estimate_straighten_rotation(&image, GeometryTransform::default()).unwrap();
    assert!(
        (estimate.rotation_degrees + tilt).abs() < 0.1,
        "estimated {}",
        estimate.rotation_degrees
    );
}

#[test]
fn flips_mirror_the_rotation() {
    let raw = raster_raw(1200, 800, &render_scene(1200, 800, 4.0));
    let plain = estimate(&raw, GeometryTransform::default()).unwrap();
    for (flip_horizontal, flip_vertical) in [(true, false), (false, true)] {
        let geometry = GeometryTransform {
            flip_horizontal,
            flip_vertical,
            ..Default::default()
        };
        let flipped = estimate(&raw, geometry).unwrap();
        assert!((flipped.rotation_degrees + plain.rotation_degrees).abs() < 0.05);
    }
}

#[test]
fn estimate_ignores_the_current_rotation() {
    let raw = raster_raw(1200, 800, &render_scene(1200, 800, -2.0));
    let plain = estimate(&raw, GeometryTransform::default()).unwrap();
    let rotated = estimate(
        &raw,
        GeometryTransform {
            rotation_degrees: 9.0,
            quarter_turns: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plain, rotated);
}

#[test]
fn perspective_shear_is_applied_before_rotation() {
    // Horizontal edges only: vertical perspective tilts them by the shear angle.
    let width = 1200;
    let height = 800;
    let scale = width as f32;
    let values: Vec<f32> = (0..width * height)
        .map(|index| {
            let y = (index / width) as f32 + 0.5 - height as f32 * 0.5;
            let band = ((y / scale * 12.0).rem_euclid(1.0) - 0.5).abs();
            if band < 0.15 {
                0.15
            } else {
                0.7
            }
        })
        .collect();
    let raw = raster_raw(width, height, &values);
    let geometry = GeometryTransform {
        vertical_transform: 4.0,
        ..Default::default()
    };
    let estimate = estimate(&raw, geometry).unwrap();
    assert!(
        (estimate.rotation_degrees + 4.0).abs() < 0.1,
        "estimated {}",
        estimate.rotation_degrees
    );
}

#[test]
fn texture_without_lines_has_no_estimate() {
    let mut state = 0x2545_f491_u32;
    let values: Vec<f32> = (0..640 * 480)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            0.2 + 0.6 * (state as f32 / u32::MAX as f32)
        })
        .collect();
    let raw = raster_raw(640, 480, &values);
    assert_eq!(estimate(&raw, GeometryTransform::default()), None);

    let flat = raster_raw(640, 480, &vec![0.4; 640 * 480]);
    assert_eq!(estimate(&flat, GeometryTransform::default()), None);
}

#[test]
fn detector_finds_a_single_straight_edge() {
    let (width, height) = (400usize, 300usize);
    let angle = 10.0f32.to_radians();
    let (sin, cos) = angle.sin_cos();
    let values: Vec<f32> = (0..width * height)
        .map(|index| {
            let x = (index % width) as f32 - 200.0;
            let y = (index / width) as f32 - 150.0;
            // Signed distance from a line through the centre at `angle`.
            let distance = -x * sin + y * cos;
            40.0 + 160.0 * (0.5 + distance).clamp(0.0, 1.0)
        })
        .collect();
    let segments = detect_line_segments(width, height, &values, 40.0);
    assert_eq!(segments.len(), 1, "{segments:?}");
    let [dx, dy] = segments[0].delta();
    let measured = dy.atan2(dx).to_degrees();
    let measured = measured - (measured / 180.0).round() * 180.0;
    assert!((measured - 10.0).abs() < 0.1, "measured {measured}");
    assert!(segments[0].length() > 300.0);
}

#[test]
fn tiny_images_are_rejected() {
    let raw = raster_raw(12, 12, &[0.5; 144]);
    assert!(LineAnalysisImage::from_raw(&raw).is_err());
}

#[test]
fn verticals_outweigh_a_long_receding_edge() {
    // A roofline receding in depth at 14° across the frame, and short level-scene verticals
    // (posts, window frames) tilted by the camera's 1.5° roll.
    let (width, height) = (1200usize, 800usize);
    let roll = 1.5f32;
    let (roll_sin, roll_cos) = roll.to_radians().sin_cos();
    let (roof_sin, roof_cos) = 14.0f32.to_radians().sin_cos();
    let posts = [-420.0f32, -260.0, -90.0, 70.0, 230.0, 400.0];
    let values: Vec<f32> = (0..width * height)
        .map(|index| {
            let x = (index % width) as f32 + 0.5 - width as f32 * 0.5;
            let y = (index / width) as f32 + 0.5 - height as f32 * 0.5;
            let roof = (-x * roof_sin + (y + 150.0) * roof_cos).abs() - 3.0;
            let scene_x = roll_cos * x + roll_sin * y;
            let scene_y = -roll_sin * x + roll_cos * y;
            let post = posts
                .iter()
                .map(|&post_x| ((scene_x - post_x).abs() - 4.0).max((scene_y - 120.0).abs() - 90.0))
                .fold(f32::MAX, f32::min);
            let coverage = (0.5 - roof.min(post)).clamp(0.0, 1.0);
            0.7 - 0.55 * coverage
        })
        .collect();
    let raw = raster_raw(width, height, &values);
    let estimate = estimate(&raw, GeometryTransform::default()).unwrap();
    assert!(
        (estimate.rotation_degrees + roll).abs() < 0.15,
        "estimated {}",
        estimate.rotation_degrees
    );
}

/// Parallel bars at `angle_degrees` (y-down), `spacing` apart, filling the given rectangle of a
/// plain background, like rails on a bridge or a grating.
fn parallel_bars(
    (width, height): (usize, usize),
    [left, top, right, bottom]: [f32; 4],
    angle_degrees: f32,
    spacing: f32,
) -> Vec<f32> {
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    (0..width * height)
        .map(|index| {
            let x = (index % width) as f32 + 0.5;
            let y = (index / width) as f32 + 0.5;
            if x < left || x > right || y < top || y > bottom {
                return 0.6;
            }
            let across = -x * sin + y * cos;
            let distance = ((across / spacing).rem_euclid(1.0) - 0.5).abs() * spacing - 1.5;
            0.6 - 0.45 * (0.5 - distance).clamp(0.0, 1.0)
        })
        .collect()
}

#[test]
fn receding_rails_alone_give_no_estimate() {
    // The lower part of a landscape whose only straight lines are a railway bridge crossing
    // the frame at 13° in depth.
    let size = (800, 1200);
    let values = parallel_bars(size, [300.0, 850.0, 800.0, 1050.0], 13.0, 14.0);
    let raw = raster_raw(size.0, size.1, &values);
    assert_eq!(estimate(&raw, GeometryTransform::default()), None);
}

#[test]
fn a_grating_votes_as_one_structure() {
    // A dense grating of near-vertical bars in one corner must not outvote a frame-wide level
    // horizon.
    let (width, height) = (1200usize, 800usize);
    let mut values = parallel_bars((width, height), [950.0, 40.0, 1150.0, 260.0], 99.0, 8.0);
    for (index, value) in values.iter_mut().enumerate() {
        let y = (index / width) as f32 + 0.5;
        if y > 520.0 {
            *value *= 0.4;
        }
    }
    let raw = raster_raw(width, height, &values);
    let estimate = estimate(&raw, GeometryTransform::default()).unwrap();
    assert!(
        estimate.rotation_degrees.abs() < 0.1,
        "estimated {}",
        estimate.rotation_degrees
    );
}

#[test]
fn soft_edges_are_found_at_coarser_scales() {
    // A defocused edge: an 8-pixel ramp spreads its gradient too far for the straightness test
    // at full size, but is as tight as a sharp edge after downsampling.
    let (width, height) = (1200usize, 900usize);
    let angle = 4.0f32.to_radians();
    let (sin, cos) = angle.sin_cos();
    let values: Vec<f32> = (0..width * height)
        .map(|index| {
            let x = (index % width) as f32 - 600.0;
            let y = (index / width) as f32 - 450.0;
            let across = x * cos + y * sin;
            40.0 + 160.0 * (0.5 + across / 8.0).clamp(0.0, 1.0)
        })
        .collect();
    assert!(detect_line_segments(width, height, &values, 90.0).is_empty());
    let segments = detect_line_segments_multiscale(width, height, &values, 90.0);
    assert!(!segments.is_empty());
    let longest = segments
        .iter()
        .max_by(|a, b| a.length().total_cmp(&b.length()))
        .unwrap();
    let [dx, dy] = longest.delta();
    let measured = dx.atan2(-dy).to_degrees();
    let measured = measured - (measured / 180.0).round() * 180.0;
    assert!((measured - 4.0).abs() < 0.3, "measured {measured}");
    assert!(longest.length() > 600.0);
}

#[test]
fn heavy_roll_beats_receding_courses() {
    // A frontal wall rolled by 6°: a 600-pixel impost line and one soft-edged pillar, among
    // stone courses receding in depth at 14° that are numerous. The evidence is modest, as in
    // a real photo, so a tilt penalty on each vote would lose it.
    let (width, height) = (1200usize, 800usize);
    let roll = 6.0f32;
    let (roll_sin, roll_cos) = roll.to_radians().sin_cos();
    let (course_sin, course_cos) = 14.0f32.to_radians().sin_cos();
    let pillars = [130.0f32];
    let courses = [
        (-380.0f32, 300.0f32),
        (-100.0, 330.0),
        (160.0, 270.0),
        (400.0, 310.0),
        (-250.0, 200.0),
        (250.0, 160.0),
    ];
    let values: Vec<f32> = (0..width * height)
        .map(|index| {
            let x = (index % width) as f32 + 0.5 - width as f32 * 0.5;
            let y = (index / width) as f32 + 0.5 - height as f32 * 0.5;
            let scene_x = roll_cos * x + roll_sin * y;
            let scene_y = -roll_sin * x + roll_cos * y;
            let impost = ((scene_y + 90.0).abs() - 2.0).max(scene_x.abs() - 300.0);
            let pillar = pillars
                .iter()
                .map(|&pillar_x| {
                    ((scene_x - pillar_x).abs() - 14.0).max((scene_y - 120.0).abs() - 90.0)
                })
                .fold(f32::MAX, f32::min);
            let course = courses
                .iter()
                .map(|&(cx, cy)| {
                    let (dx, dy) = (x - cx, y - cy);
                    let along = dx * course_cos + dy * course_sin;
                    let across = -dx * course_sin + dy * course_cos;
                    (across.abs() - 2.0).max(along.abs() - 90.0)
                })
                .fold(f32::MAX, f32::min);
            let sharp = (0.5 - impost.min(course)).clamp(0.0, 1.0);
            let soft = (0.5 - pillar / 6.0).clamp(0.0, 1.0);
            0.7 - 0.5 * sharp.max(soft)
        })
        .collect();
    let raw = raster_raw(width, height, &values);
    let estimate = estimate(&raw, GeometryTransform::default()).unwrap();
    assert!(
        (estimate.rotation_degrees + roll).abs() < 0.3,
        "estimated {}",
        estimate.rotation_degrees
    );
}

/// TEMPORARY debug harness.
#[test]
#[ignore]
fn debug_dump() {
    let path = std::env::var("STRAIGHTEN_RAW").unwrap();
    let out = std::env::var("STRAIGHTEN_OUT").unwrap();
    let raw = crate::pipeline::load_raw_file(std::path::Path::new(&path)).unwrap();
    let image = LineAnalysisImage::from_raw(&raw).unwrap();
    let longest = image.width.max(image.height) as f32;
    let segments = detect_line_segments_multiscale(image.width, image.height, &image.values, longest * MIN_SEGMENT_FRACTION);
    let kept = distinct_structures(segments, longest * STRUCTURE_DISTANCE_FRACTION);
    let mut rgb = image::RgbImage::new(image.width as u32, image.height as u32);
    for (i, v) in image.values.iter().enumerate() {
        let g = (*v * 0.6) as u8;
        rgb.put_pixel((i % image.width) as u32, (i / image.width) as u32, image::Rgb([g, g, g]));
    }
    for s in &kept {
        let [dx, dy] = s.delta();
        let a = dy.atan2(dx).to_degrees();
        let q = (a / 90.0).round();
        let tilt = a - q * 90.0;
        let vertical = (q as i32).rem_euclid(2) == 1;
        if tilt.abs() > 25.0 { continue; }
        if vertical {
            let m = s.midpoint();
            println!("V tilt {tilt:6.1} len {:5.0} mid ({:4.0},{:4.0})", s.length(), m[0], m[1]);
        }
        let c = if vertical { [255, 60, 60] } else { [60, 160, 255] };
        let n = s.length().ceil() as usize + 1;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let x = s.start[0] + dx * t;
            let y = s.start[1] + dy * t;
            if x >= 0.0 && y >= 0.0 && (x as u32) < rgb.width() && (y as u32) < rgb.height() {
                for (ox, oy) in [(0, 0), (1, 0), (0, 1)] {
                    if ((x as u32 + ox) < rgb.width()) && ((y as u32 + oy) < rgb.height()) {
                        rgb.put_pixel(x as u32 + ox, y as u32 + oy, image::Rgb(c));
                    }
                }
            }
        }
    }
    rgb.save(out).unwrap();
}
