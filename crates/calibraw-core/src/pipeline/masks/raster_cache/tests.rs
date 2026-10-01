use super::super::{
    blur_probability_mask, crop_mask_image, mask_feather_radius, rasterize_mask_image,
    shape_distance_mask, shape_probability_mask_from_source, smoothstep, source_mask_core_radius,
};
use super::*;

fn assert_same_pixels(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.to_bits(), expected.to_bits(), "pixel {index}");
    }
}

// Independent small-image oracle: measure both selected runs at every pixel,
// rather than reducing segments or retaining a frontier.
fn uncached_source_radius(source: &MaskImage, width: u32, height: u32) -> Option<f32> {
    if source.width == 0 || source.height == 0 {
        return None;
    }
    let sx = width as f32
        / source.width as f32
        / (source.sampling_rect[2] - source.sampling_rect[0])
            .abs()
            .max(1e-6);
    let sy = height as f32
        / source.height as f32
        / (source.sampling_rect[3] - source.sampling_rect[1])
            .abs()
            .max(1e-6);
    let selected = |x: u32, y: u32| source.pixels[(y * source.width + x) as usize] >= 128;
    let mut thickest = 0.0f32;
    for y in 0..source.height {
        for x in 0..source.width {
            if !selected(x, y) {
                continue;
            }
            let mut left = x;
            let mut right = x + 1;
            let mut top = y;
            let mut bottom = y + 1;
            while left > 0 && selected(left - 1, y) {
                left -= 1;
            }
            while right < source.width && selected(right, y) {
                right += 1;
            }
            while top > 0 && selected(x, top - 1) {
                top -= 1;
            }
            while bottom < source.height && selected(x, bottom) {
                bottom += 1;
            }
            thickest = thickest.max(((right - left) as f32 * sx).min((bottom - top) as f32 * sy));
        }
    }
    (thickest > 0.0).then_some(thickest * 0.45)
}

// The original grow/feather calculation, including its floating-point order.
fn uncached_shape_distance(
    mask: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
    feather_inside: bool,
    core_radius: Option<f32>,
) {
    if width == 0 || height == 0 || mask.is_empty() {
        return;
    }
    let binary = mask
        .iter()
        .map(|value| u8::from(*value >= 0.5))
        .collect::<Vec<_>>();
    if binary.iter().all(|value| *value == binary[0]) {
        return;
    }
    let inside = chamfer_distance(&binary, width as usize, height as usize, 1);
    let outside = chamfer_distance(&binary, width as usize, height as usize, 0);
    let edge = width.min(height) as f32;
    let grow_radius = grow * edge * 0.05;
    let mut feather_radius = mask_feather_radius(edge, feather);
    if feather_radius > 0.0 {
        let deepest = outside
            .iter()
            .zip(&binary)
            .filter(|(_, inside)| **inside == 1)
            .map(|(distance, _)| *distance)
            .fold(0.0f32, f32::max);
        feather_radius = feather_radius.min(core_radius.unwrap_or(deepest * 0.8));
    }
    for (index, value) in mask.iter_mut().enumerate() {
        let confidence = (*value - 0.5) * 0.5;
        let distance = outside[index] - inside[index] + confidence + grow_radius;
        *value = if feather_radius <= 1e-5 {
            smoothstep(-0.75, 0.75, distance)
        } else if feather_inside {
            smoothstep(0.0, feather_radius, distance)
        } else {
            smoothstep(-feather_radius, feather_radius, distance)
        };
    }
}

fn uncached_probability_from_source(
    mask: &mut [f32],
    width: u32,
    height: u32,
    grow: f32,
    feather: f32,
    source: &MaskImage,
) {
    let needs_core = feather > 1e-5
        && (grow.abs() > 1e-5 || mask_feather_radius(width.min(height) as f32, feather) > 1.0);
    let core = needs_core
        .then(|| uncached_source_radius(source, width, height))
        .flatten()
        .map(|radius| (radius + grow.min(0.0) * width.min(height) as f32 * 0.05).max(0.5));
    let grow = grow.clamp(-32.0, 32.0);
    let feather = feather.clamp(0.0, 32.0);
    if grow.abs() <= 1e-5 && feather <= 1e-5 {
        for value in mask {
            *value = value.clamp(0.0, 1.0);
        }
        return;
    }
    let radius = mask_feather_radius(width.min(height) as f32, feather);
    if grow.abs() <= 1e-5 && radius < 3.0 {
        let original = (radius > 1.0).then(|| mask.to_vec());
        blur_probability_mask(mask, width as usize, height as usize, radius);
        if let Some(mut contour) = original {
            uncached_shape_distance(&mut contour, width, height, grow, feather, false, core);
            let blend = smoothstep(1.0, 3.0, radius);
            for (blurred, contour) in mask.iter_mut().zip(contour) {
                *blurred += (contour - *blurred) * blend;
            }
        }
    } else {
        uncached_shape_distance(mask, width, height, grow, feather, false, core);
    }
}

#[test]
fn source_frontier_matches_every_small_contour_at_different_scales_and_crops() {
    let cache = SourceCache(BoundedCache::new(SOURCE_BYTES, MAX_ENTRIES));
    for bits in 0..512 {
        let pixels = (0..9)
            .map(|index| {
                if bits & (1 << index) != 0 {
                    128 + (index * 13) as u8
                } else {
                    127
                }
            })
            .collect();
        let source = MaskImage::new(3, 3, pixels).unwrap();
        let frontier = cache.get(&source);
        for rect in [
            [0.0, 0.0, 1.0, 1.0],
            [0.13, 0.27, 0.68, 0.84],
            [0.91, 0.87, 0.16, 0.29],
            [0.3, 0.7, 0.3, 0.7],
        ] {
            let mut cropped = source.clone();
            cropped.sampling_rect = rect;
            assert!(Arc::ptr_eq(&frontier, &cache.get(&cropped)));
            for (width, height) in [(1, 1), (31, 7), (7, 31), (2048, 1537), (0, 9)] {
                assert_eq!(
                    source_mask_core_radius(&cropped, width, height).map(f32::to_bits),
                    uncached_source_radius(&cropped, width, height).map(f32::to_bits),
                    "contour {bits}, target {width}x{height}, rect {rect:?}",
                );
            }
        }
    }
    assert_eq!(
        source_mask_core_radius(&MaskImage::new(0, 3, vec![]).unwrap(), 8, 8),
        None
    );
}

#[test]
fn source_frontier_keeps_only_nondominated_pairs() {
    let mut pixels = vec![0; 22 * 10];
    for (left, width, height) in [(0, 3, 7), (4, 5, 5), (10, 9, 3), (20, 2, 2)] {
        for y in 0..height {
            pixels[y * 22 + left..y * 22 + left + width].fill(255);
        }
    }
    let source = MaskImage::new(22, 10, pixels).unwrap();
    let frontier = SourceFrontier::prepare(&source);
    assert_eq!(frontier.pairs, [(3, 7), (5, 5), (9, 3)]);
    for (width, height) in [(13, 201), (201, 13), (79, 43)] {
        assert_eq!(
            source_mask_core_radius(&source, width, height).map(f32::to_bits),
            uncached_source_radius(&source, width, height).map(f32::to_bits)
        );
    }
}

#[test]
fn source_cache_uses_weak_identity_dimensions_and_updated_pixels() {
    let cache = SourceCache(BoundedCache::new(SOURCE_BYTES, MAX_ENTRIES));
    let mut source =
        MaskImage::new(4, 3, vec![0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0]).unwrap();
    let first = cache.get(&source);
    assert_eq!(Arc::strong_count(&source.pixels), 1);
    let crop = crop_mask_image(&source, 0.2, 0.1, 0.6, 0.8);
    assert!(Arc::ptr_eq(&first, &cache.get(&crop)));
    let mut reshaped = source.clone();
    reshaped.width = 3;
    reshaped.height = 4;
    assert!(!Arc::ptr_eq(&first, &cache.get(&reshaped)));
    let replaced = MaskImage::new(4, 3, source.pixels.to_vec()).unwrap();
    assert!(!Arc::ptr_eq(&first, &cache.get(&replaced)));
    drop(crop);
    drop(reshaped);
    Arc::make_mut(&mut source.pixels).fill(255);
    let updated = cache.get(&source);
    assert!(!Arc::ptr_eq(&first, &updated));
    assert_eq!(first.pixels.strong_count(), 0);
    assert_eq!(updated.core_radius(1.0, 1.0), Some(3.0 * 0.45));
    assert!(cache
        .0
        .state
        .lock()
        .unwrap()
        .entries
        .iter()
        .all(|entry| entry.data.pixels.strong_count() > 0));
    drop(source);
    drop(replaced);
    let blank = MaskImage::new(1, 1, vec![0]).unwrap();
    assert_eq!(cache.get(&blank).core_radius(1.0, 1.0), None);
    assert_eq!(cache.0.state.lock().unwrap().entries.len(), 1);
}

fn sample_binary(width: usize, height: usize) -> Vec<u8> {
    (0..width * height)
        .map(|index| {
            let (x, y) = (index % width, index / width);
            u8::from(x > width / 5 && x < width * 4 / 5 && y > height / 6 && y < height * 5 / 6)
        })
        .collect()
}

#[test]
fn contour_cache_checks_binary_equality_even_when_fingerprints_collide() {
    let cache = ContourCache(BoundedCache::new(CONTOUR_BYTES, MAX_ENTRIES));
    let binary = sample_binary(7, 5);
    let first = cache.get_with_fingerprint(binary.clone(), 7, 5, 42);
    assert!(Arc::ptr_eq(
        &first,
        &cache.get_with_fingerprint(binary.clone(), 7, 5, 42)
    ));
    let mut changed = binary.clone();
    changed[0] = 1;
    let second = cache.get_with_fingerprint(changed.clone(), 7, 5, 42);
    assert!(!Arc::ptr_eq(&first, &second));
    let reshaped = cache.get_with_fingerprint(binary, 5, 7, 42);
    assert!(!Arc::ptr_eq(&first, &reshaped));
    for prepared in [&first, &second, &reshaped] {
        let inside = chamfer_distance(&prepared.binary, prepared.width, prepared.height, 1);
        let outside = chamfer_distance(&prepared.binary, prepared.width, prepared.height, 0);
        let signed = outside
            .iter()
            .zip(&inside)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>();
        assert_same_pixels(&prepared.signed_distance, &signed);
        let deepest = outside
            .iter()
            .zip(&prepared.binary)
            .filter(|(_, value)| **value == 1)
            .map(|(value, _)| *value)
            .fold(0.0f32, f32::max);
        assert_eq!(prepared.deepest_inside.to_bits(), deepest.to_bits());
    }
    assert!(cache.get(vec![], 0, 0).is_none());
    assert!(cache.get(vec![0; 35], 7, 5).is_none());
    assert!(cache.get(vec![1; 35], 7, 5).is_none());
}

#[test]
fn cached_grow_and_feather_match_uncached_bits_with_changed_confidence_and_contours() {
    for (width, height) in [(31, 23), (23, 31)] {
        let mut binary = sample_binary(width as usize, height as usize);
        for changed in [false, true] {
            if changed {
                binary[(width * height / 2) as usize] ^= 1;
            }
            for confidence in [0, 1] {
                let original = binary
                    .iter()
                    .enumerate()
                    .map(|(index, inside)| {
                        if *inside == 1 {
                            [0.5, 0.61, 0.87, 1.2][(index + confidence) % 4]
                        } else {
                            [-0.2, 0.0, 0.23, 0.49][(index + confidence) % 4]
                        }
                    })
                    .collect::<Vec<_>>();
                for grow in [-1.0, -0.015, 0.0, 0.6] {
                    for feather in [0.0, 0.1, 0.6, 3.0] {
                        for feather_inside in [false, true] {
                            for core in [None, Some(0.5), Some(2.125)] {
                                let mut actual = original.clone();
                                let mut expected = original.clone();
                                shape_distance_mask(
                                    &mut actual,
                                    width,
                                    height,
                                    grow,
                                    feather,
                                    feather_inside,
                                    core,
                                );
                                uncached_shape_distance(
                                    &mut expected,
                                    width,
                                    height,
                                    grow,
                                    feather,
                                    feather_inside,
                                    core,
                                );
                                assert_same_pixels(&actual, &expected);
                            }
                        }
                    }
                }
            }
        }
    }
    for original in [vec![0.25; 12], vec![0.75; 12], vec![]] {
        let mut actual = original.clone();
        let mut expected = original;
        shape_distance_mask(&mut actual, 4, 3, 0.4, 0.8, false, None);
        uncached_shape_distance(&mut expected, 4, 3, 0.4, 0.8, false, None);
        assert_same_pixels(&actual, &expected);
    }
}

#[test]
fn cached_generated_raster_matches_uncached_across_crop_scale_and_blur_blending() {
    let pixels = sample_binary(31, 19)
        .into_iter()
        .enumerate()
        .map(|(index, inside)| {
            if inside == 1 {
                128 + (index % 128) as u8
            } else {
                (index % 128) as u8
            }
        })
        .collect();
    let source = MaskImage::new(31, 19, pixels).unwrap();
    for source in [
        source.clone(),
        crop_mask_image(&source, 0.17, 0.12, 0.53, 0.74),
    ] {
        for (width, height) in [(63, 47), (47, 63), (16, 16)] {
            let blended_feather = (2.0 / (width.min(height) as f32 * 0.045)).powf(1.0 / 1.30);
            for grow in [-0.3, 0.0, 0.4] {
                for feather in [0.0, 0.01, blended_feather, 1.0, 2.0] {
                    let mut actual = rasterize_mask_image(width, height, &source);
                    let mut expected = actual.clone();
                    shape_probability_mask_from_source(
                        &mut actual,
                        width,
                        height,
                        grow,
                        feather,
                        &source,
                    );
                    uncached_probability_from_source(
                        &mut expected,
                        width,
                        height,
                        grow,
                        feather,
                        &source,
                    );
                    assert_same_pixels(&actual, &expected);
                }
            }
        }
    }
}

#[test]
fn contour_cache_evicts_by_bytes_and_lru_and_keeps_active_data_usable() {
    let binary = |index| {
        let mut pixels = vec![0; 64];
        pixels[index] = 1;
        pixels
    };
    let entry_bytes = PreparedContour::prepare(binary(0), 8, 8, 0).byte_len();
    let cache = ContourCache(BoundedCache::new(entry_bytes * 2, MAX_ENTRIES));
    let first = cache.get(binary(0), 8, 8).unwrap();
    let second = cache.get(binary(1), 8, 8).unwrap();
    assert!(Arc::ptr_eq(&first, &cache.get(binary(0), 8, 8).unwrap()));
    cache.get(binary(2), 8, 8).unwrap();
    assert!(Arc::ptr_eq(&first, &cache.get(binary(0), 8, 8).unwrap()));
    assert_same_pixels(
        &second.signed_distance,
        &PreparedContour::prepare(binary(1), 8, 8, 0).signed_distance,
    );
    assert!(!Arc::ptr_eq(&second, &cache.get(binary(1), 8, 8).unwrap()));
    for index in 3..64 {
        cache.get(binary(index), 8, 8).unwrap();
        let state = cache.0.state.lock().unwrap();
        assert!(state.bytes <= entry_bytes * 2);
        assert!(state.entries.len() <= 2);
        assert_eq!(
            state.bytes,
            state.entries.iter().map(|entry| entry.bytes).sum::<usize>()
        );
    }
    let retained = cache.get(binary(63), 8, 8).unwrap();
    let large = sample_binary(32, 16);
    let oversized = cache.get(large.clone(), 32, 16).unwrap();
    assert!(!Arc::ptr_eq(&oversized, &cache.get(large, 32, 16).unwrap()));
    assert!(Arc::ptr_eq(
        &retained,
        &cache.get(binary(63), 8, 8).unwrap()
    ));
    assert_eq!(cache.0.state.lock().unwrap().entries.len(), 2);
}

#[test]
fn both_caches_obey_entry_caps_and_source_byte_caps() {
    let contour = ContourCache(BoundedCache::new(CONTOUR_BYTES, 2));
    let source_cache = SourceCache(BoundedCache::new(SOURCE_BYTES, 2));
    let sources = (0..8)
        .map(|index| {
            let mut binary = vec![0; 16];
            binary[index] = 255;
            MaskImage::new(4, 4, binary).unwrap()
        })
        .collect::<Vec<_>>();
    for source in &sources {
        source_cache.get(source);
        contour.get(
            source
                .pixels
                .iter()
                .map(|value| u8::from(*value >= 128))
                .collect(),
            4,
            4,
        );
        assert!(source_cache.0.state.lock().unwrap().entries.len() <= 2);
        assert!(contour.0.state.lock().unwrap().entries.len() <= 2);
        assert_eq!(Arc::strong_count(&source.pixels), 1);
    }
    let entry_bytes = SourceFrontier::prepare(&sources[0]).byte_len();
    let small = SourceCache(BoundedCache::new(entry_bytes, MAX_ENTRIES));
    for source in &sources {
        small.get(source);
        let state = small.0.state.lock().unwrap();
        assert!(state.bytes <= entry_bytes);
        assert_eq!(state.entries.len(), 1);
    }
    let disabled = SourceCache(BoundedCache::new(0, MAX_ENTRIES));
    let uncached = disabled.get(&sources[0]);
    assert!(!Arc::ptr_eq(&uncached, &disabled.get(&sources[0])));
    assert!(disabled.0.state.lock().unwrap().entries.is_empty());
}

#[test]
fn source_cache_budget_accounts_for_the_allocation_kept_by_weak_pixels() {
    let source = MaskImage::new(64, 64, vec![255; 64 * 64]).unwrap();
    let prepared = SourceFrontier::prepare(&source);
    let metadata_bytes =
        size_of::<SourceFrontier>() + prepared.pairs.capacity() * size_of::<(u32, u32)>();
    assert_eq!(prepared.byte_len(), metadata_bytes + source.pixels.len());

    let cache = SourceCache(BoundedCache::new(metadata_bytes, MAX_ENTRIES));
    let first = cache.get(&source);
    assert!(!Arc::ptr_eq(&first, &cache.get(&source)));
    assert!(cache.0.state.lock().unwrap().entries.is_empty());
}

#[test]
fn concurrent_preparations_reuse_one_entry_per_source_and_contour() {
    let contour = ContourCache(BoundedCache::new(CONTOUR_BYTES, MAX_ENTRIES));
    let sources = SourceCache(BoundedCache::new(SOURCE_BYTES, MAX_ENTRIES));
    let binary = sample_binary(32, 24);
    let source = MaskImage::new(32, 24, binary.iter().map(|value| value * 255).collect()).unwrap();
    let barrier = std::sync::Barrier::new(8);
    let prepared = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    (
                        contour.get(binary.clone(), 32, 24).unwrap(),
                        sources.get(&source),
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    for (contour, source) in &prepared[1..] {
        assert!(Arc::ptr_eq(contour, &prepared[0].0));
        assert!(Arc::ptr_eq(source, &prepared[0].1));
    }
    assert_eq!(contour.0.state.lock().unwrap().entries.len(), 1);
    assert_eq!(sources.0.state.lock().unwrap().entries.len(), 1);
}
