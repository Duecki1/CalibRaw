//! Retouch (clone and heal) strokes: source offsets, patch building and Laplace healing.

use super::*;

pub(super) fn run_retouch(request: RetouchRequest) -> Result<RemoveStroke> {
    ensure_not_cancelled(&request.cancellation)?;
    anyhow::ensure!(!request.brush.points.is_empty(), "Retouch brush is empty");
    anyhow::ensure!(
        request.retouch.source.iter().all(|value| value.is_finite())
            && request
                .retouch
                .destination
                .iter()
                .all(|value| value.is_finite()),
        "Retouch source coordinates are invalid"
    );

    let render = |crop: NativeRect, remove: RemoveEditState| {
        render_remove_scene_crop(DevelopedCropJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: request.geometry,
            exposure: request.exposure,
            masks: request.masks.clone(),
            remove,
            crop,
            program_prewarm: request.program_prewarm.clone(),
        })
    };
    let chunks = split_retouch_brush(&request.brush);
    anyhow::ensure!(
        chunks.len() <= crate::pipeline::REMOVE_MAX_PATCHES_PER_STROKE,
        "Retouch stroke is too long"
    );
    let source_snapshot = request.existing.clone();
    let mut destination_working = request.existing.clone();
    let mut patches = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        ensure_not_cancelled(&request.cancellation)?;
        let destination_bounds =
            retouch_stroke_bounds(request.raw.width, request.raw.height, &chunk)?;
        let source_bounds = retouch_source_bounds(
            request.raw.width,
            request.raw.height,
            destination_bounds,
            &chunk,
            request.retouch,
        )?;
        let destination_scene = render(destination_bounds, destination_working.clone())
            .context("render retouch destination")?;
        ensure_not_cancelled(&request.cancellation)?;
        let source_scene =
            render(source_bounds, source_snapshot.clone()).context("render retouch source")?;
        ensure_not_cancelled(&request.cancellation)?;
        let patch = build_retouch_patch(
            &request.raw,
            &request.exposure,
            destination_bounds,
            &destination_scene,
            source_bounds,
            &source_scene,
            &chunk,
            request.retouch,
            &request.cancellation,
        )?;
        destination_working.strokes.push(RemoveStroke {
            brush: chunk,
            patches: vec![patch.clone()],
            retouch: Some(request.retouch),
            opacity: request.retouch.opacity,
        });
        patches.push(patch);
    }
    Ok(RemoveStroke {
        brush: request.brush,
        patches,
        retouch: Some(request.retouch),
        opacity: request.retouch.opacity,
    })
}

fn split_retouch_brush(brush: &RemoveBrushStroke) -> Vec<RemoveBrushStroke> {
    const MAX_CHUNK_EDGE: f32 = 2_048.0;
    const MAX_CHUNK_POINTS: usize = 512;
    let mut chunks = Vec::new();
    let mut points = Vec::new();
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for point in &brush.points {
        let radius = point.radius.max(0.5) + 2.0;
        let candidate = [
            bounds[0].min(point.x - radius),
            bounds[1].min(point.y - radius),
            bounds[2].max(point.x + radius),
            bounds[3].max(point.y + radius),
        ];
        let too_large = !points.is_empty()
            && (candidate[2] - candidate[0] > MAX_CHUNK_EDGE
                || candidate[3] - candidate[1] > MAX_CHUNK_EDGE
                || points.len() >= MAX_CHUNK_POINTS);
        if too_large {
            chunks.push(RemoveBrushStroke {
                points: std::mem::take(&mut points),
                dilation_radius: 0,
            });
            bounds = [
                point.x - radius,
                point.y - radius,
                point.x + radius,
                point.y + radius,
            ];
        } else {
            bounds = candidate;
        }
        points.push(*point);
    }
    if !points.is_empty() {
        chunks.push(RemoveBrushStroke {
            points,
            dilation_radius: 0,
        });
    }
    chunks
}

fn retouch_stroke_bounds(
    image_width: u32,
    image_height: u32,
    brush: &RemoveBrushStroke,
) -> Result<NativeRect> {
    let mut left = image_width as f32;
    let mut top = image_height as f32;
    let mut right = 0.0f32;
    let mut bottom = 0.0f32;
    for point in &brush.points {
        if !point.x.is_finite() || !point.y.is_finite() || !point.radius.is_finite() {
            continue;
        }
        let radius = point.radius.max(0.5) + 2.0;
        left = left.min(point.x - radius);
        top = top.min(point.y - radius);
        right = right.max(point.x + radius);
        bottom = bottom.max(point.y + radius);
    }
    clipped_native_rect(image_width, image_height, left, top, right, bottom)
        .context("Retouch stroke lies outside the image")
}

fn retouch_source_bounds(
    image_width: u32,
    image_height: u32,
    destination: NativeRect,
    brush: &RemoveBrushStroke,
    retouch: RetouchStroke,
) -> Result<NativeRect> {
    if retouch.alignment == RetouchAlignment::Fixed {
        let radius = brush
            .points
            .iter()
            .map(|point| point.radius.max(0.5))
            .fold(0.5f32, f32::max)
            + 3.0;
        return clipped_native_rect(
            image_width,
            image_height,
            retouch.source[0] - radius,
            retouch.source[1] - radius,
            retouch.source[0] + radius,
            retouch.source[1] + radius,
        )
        .context("Retouch source lies outside the image");
    }
    let offset = retouch_source_offset(retouch);
    clipped_native_rect(
        image_width,
        image_height,
        destination.x as f32 + offset[0] - 2.0,
        destination.y as f32 + offset[1] - 2.0,
        destination.right() as f32 + offset[0] + 2.0,
        destination.bottom() as f32 + offset[1] + 2.0,
    )
    .context("Retouch source lies outside the image")
}

fn clipped_native_rect(
    image_width: u32,
    image_height: u32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) -> Option<NativeRect> {
    let left = left.floor().clamp(0.0, image_width as f32) as u32;
    let top = top.floor().clamp(0.0, image_height as f32) as u32;
    let right = right.ceil().clamp(0.0, image_width as f32) as u32;
    let bottom = bottom.ceil().clamp(0.0, image_height as f32) as u32;
    (right > left && bottom > top).then_some(NativeRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn retouch_source_offset(retouch: RetouchStroke) -> [f32; 2] {
    if retouch.alignment == RetouchAlignment::Registered {
        [0.0, 0.0]
    } else {
        [
            retouch.source[0] - retouch.destination[0],
            retouch.source[1] - retouch.destination[1],
        ]
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_retouch_patch(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    destination_bounds: NativeRect,
    destination_scene: &[f32],
    source_bounds: NativeRect,
    source_scene: &[f32],
    brush: &RemoveBrushStroke,
    retouch: RetouchStroke,
    cancellation: &AtomicBool,
) -> Result<RemovePatch> {
    let pixels = destination_bounds.width as usize * destination_bounds.height as usize;
    anyhow::ensure!(
        destination_scene.len() == pixels * 3,
        "Retouch destination has an invalid RGB length"
    );
    anyhow::ensure!(
        source_scene.len() == source_bounds.width as usize * source_bounds.height as usize * 3,
        "Retouch source has an invalid RGB length"
    );

    let width = destination_bounds.width as usize;
    let height = destination_bounds.height as usize;
    let hardness = retouch.hardness.clamp(0.0, 1.0);
    let offset = retouch_source_offset(retouch);
    let mut stroke_coverage = vec![0.0f32; pixels];
    let output = match retouch.tool {
        RetouchTool::Clone => {
            let mut output_scene = destination_scene.to_vec();
            for (dab_index, point) in brush.points.iter().enumerate() {
                if dab_index % 16 == 0 {
                    ensure_not_cancelled(cancellation)?;
                }
                let Some([left, top, right, bottom]) =
                    retouch_dab_bounds(destination_bounds, *point)
                else {
                    continue;
                };
                for y in top..bottom {
                    let destination_y = destination_bounds.y as f32 + y as f32 + 0.5;
                    for x in left..right {
                        let destination_x = destination_bounds.x as f32 + x as f32 + 0.5;
                        let coverage = retouch_brush_coverage(
                            destination_x - point.x,
                            destination_y - point.y,
                            point.radius.max(0.5),
                            hardness,
                        );
                        if coverage <= 0.0 {
                            continue;
                        }
                        let source_position = retouch_dab_source_position(
                            [destination_x, destination_y],
                            *point,
                            retouch,
                            offset,
                        );
                        let Some(source) =
                            sample_scene_bilinear(source_scene, source_bounds, source_position)
                        else {
                            continue;
                        };
                        let index = y * width + x;
                        for channel in 0..3 {
                            let destination = output_scene[index * 3 + channel];
                            output_scene[index * 3 + channel] =
                                destination + (source[channel] - destination) * coverage;
                        }
                        stroke_coverage[index] = stroke_coverage[index].max(coverage);
                    }
                }
            }
            output_scene
        }
        RetouchTool::Heal => {
            // GIMP solves and composites every dab independently. Solving one
            // large Poisson field for the whole stroke smears texture along the
            // stroke and produces noticeably flatter results.
            let mut output_working = destination_scene
                .chunks_exact(3)
                .map(|pixel| pipeline_scene_to_working_rec2020(raw, [pixel[0], pixel[1], pixel[2]]))
                .collect::<Vec<_>>();
            for point in &brush.points {
                ensure_not_cancelled(cancellation)?;
                let Some([left, top, right, bottom]) =
                    retouch_dab_bounds(destination_bounds, *point)
                else {
                    continue;
                };
                let dab_width = right - left;
                let dab_height = bottom - top;
                let dab_pixels = dab_width * dab_height;
                let mut difference = vec![0.0f32; dab_pixels * 3 + 3];
                let mut source_perceptual = vec![[0.0f32; 3]; dab_pixels];
                let mut dab_coverage = vec![0.0f32; dab_pixels];
                let mut binary_mask = vec![false; dab_pixels];
                let mut source_rect_is_valid = true;

                for local_y in 0..dab_height {
                    let y = top + local_y;
                    let destination_y = destination_bounds.y as f32 + y as f32 + 0.5;
                    for local_x in 0..dab_width {
                        let x = left + local_x;
                        let destination_x = destination_bounds.x as f32 + x as f32 + 0.5;
                        let local_index = local_y * dab_width + local_x;
                        let source_position = retouch_dab_source_position(
                            [destination_x, destination_y],
                            *point,
                            retouch,
                            offset,
                        );
                        let Some(source_scene_pixel) =
                            sample_scene_bilinear(source_scene, source_bounds, source_position)
                        else {
                            source_rect_is_valid = false;
                            continue;
                        };
                        let source = pipeline_scene_to_working_rec2020(raw, source_scene_pixel)
                            .map(srgb_encode_signed);
                        source_perceptual[local_index] = source;
                        let destination = output_working[y * width + x].map(srgb_encode_signed);
                        for channel in 0..3 {
                            difference[local_index * 3 + channel] =
                                destination[channel] - source[channel];
                        }
                        let coverage = retouch_brush_coverage(
                            destination_x - point.x,
                            destination_y - point.y,
                            point.radius.max(0.5),
                            hardness,
                        );
                        dab_coverage[local_index] = coverage;
                        binary_mask[local_index] = coverage > 0.0;
                    }
                }
                // GIMP skips a dab when its source rectangle falls off-canvas,
                // instead of solving it against synthetic edge pixels.
                if !source_rect_is_valid || !binary_mask.iter().any(|value| *value) {
                    continue;
                }
                gimp_heal_laplace_loop(
                    &mut difference,
                    dab_width,
                    dab_height,
                    &binary_mask,
                    cancellation,
                )?;
                for local_y in 0..dab_height {
                    let y = top + local_y;
                    for local_x in 0..dab_width {
                        let local_index = local_y * dab_width + local_x;
                        let coverage = dab_coverage[local_index];
                        if coverage <= 0.0 {
                            continue;
                        }
                        let x = left + local_x;
                        let index = y * width + x;
                        let source = source_perceptual[local_index];
                        let healed: [f32; 3] = std::array::from_fn(|channel| {
                            srgb_decode_signed(
                                source[channel] + difference[local_index * 3 + channel],
                            )
                        });
                        for channel in 0..3 {
                            let destination = output_working[index][channel];
                            output_working[index][channel] =
                                destination + (healed[channel] - destination) * coverage;
                        }
                        stroke_coverage[index] = stroke_coverage[index].max(coverage);
                    }
                }
            }
            output_working
                .into_iter()
                .flat_map(|working| {
                    working_rec2020_to_canonical_remove_scene(raw, exposure, working)
                })
                .collect()
        }
    };

    anyhow::ensure!(
        stroke_coverage.iter().any(|value| *value > 0.0),
        "Retouch source does not overlap the brush"
    );

    let output_is_canonical = retouch.tool == RetouchTool::Heal;
    let mut left = width;
    let mut top = height;
    let mut right = 0usize;
    let mut bottom = 0usize;
    for y in 0..height {
        for x in 0..width {
            if stroke_coverage[y * width + x] > 0.0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    anyhow::ensure!(
        right > left && bottom > top,
        "Retouch brush has no coverage"
    );
    let patch_bounds = NativeRect {
        x: destination_bounds.x + left as u32,
        y: destination_bounds.y + top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    };
    let mut rgb16f = Vec::with_capacity((right - left) * (bottom - top) * 3);
    let mut alpha = Vec::with_capacity((right - left) * (bottom - top));
    for y in top..bottom {
        for x in left..right {
            let index = y * width + x;
            let canonical = if output_is_canonical {
                [
                    output[index * 3],
                    output[index * 3 + 1],
                    output[index * 3 + 2],
                ]
            } else {
                pipeline_scene_to_canonical_remove_scene(
                    raw,
                    exposure,
                    [
                        output[index * 3],
                        output[index * 3 + 1],
                        output[index * 3 + 2],
                    ],
                )
            };
            for value in canonical {
                let finite = if value.is_finite() { value } else { 0.0 };
                rgb16f.push(half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits());
            }
            alpha.push(if stroke_coverage[index] > 0.0 { 255 } else { 0 });
        }
    }
    RemovePatch::new_scene(patch_bounds, rgb16f, alpha).map_err(anyhow::Error::msg)
}

fn retouch_dab_bounds(
    bounds: NativeRect,
    point: crate::pipeline::RemoveBrushPoint,
) -> Option<[usize; 4]> {
    let radius = point.radius.max(0.5);
    let left = (point.x - radius - bounds.x as f32).floor().max(0.0) as usize;
    let top = (point.y - radius - bounds.y as f32).floor().max(0.0) as usize;
    let right = (point.x + radius - bounds.x as f32)
        .ceil()
        .clamp(0.0, bounds.width as f32) as usize;
    let bottom = (point.y + radius - bounds.y as f32)
        .ceil()
        .clamp(0.0, bounds.height as f32) as usize;
    (right > left && bottom > top).then_some([left, top, right, bottom])
}

fn retouch_dab_source_position(
    destination: [f32; 2],
    point: crate::pipeline::RemoveBrushPoint,
    retouch: RetouchStroke,
    offset: [f32; 2],
) -> [f32; 2] {
    if retouch.alignment == RetouchAlignment::Fixed {
        [
            retouch.source[0] + destination[0] - point.x,
            retouch.source[1] + destination[1] - point.y,
        ]
    } else {
        [destination[0] + offset[0], destination[1] + offset[1]]
    }
}

pub(super) fn retouch_brush_coverage(dx: f32, dy: f32, radius: f32, hardness: f32) -> f32 {
    let distance = dx.hypot(dy);
    if distance >= radius {
        return 0.0;
    }
    let inner = radius * hardness.clamp(0.0, 1.0);
    if distance <= inner || radius - inner <= f32::EPSILON {
        return 1.0;
    }
    let t = ((radius - distance) / (radius - inner)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sample_scene_bilinear(
    scene: &[f32],
    bounds: NativeRect,
    position: [f32; 2],
) -> Option<[f32; 3]> {
    let local_x = position[0] - bounds.x as f32 - 0.5;
    let local_y = position[1] - bounds.y as f32 - 0.5;
    if local_x < -0.5
        || local_y < -0.5
        || local_x > bounds.width as f32 - 0.5
        || local_y > bounds.height as f32 - 0.5
    {
        return None;
    }
    let width = bounds.width as usize;
    let height = bounds.height as usize;
    let x = local_x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = local_y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |x: usize, y: usize, channel: usize| scene[(y * width + x) * 3 + channel];
    Some(std::array::from_fn(|channel| {
        let top = sample(x0, y0, channel) * (1.0 - tx) + sample(x1, y0, channel) * tx;
        let bottom = sample(x0, y1, channel) * (1.0 - tx) + sample(x1, y1, channel) * tx;
        top * (1.0 - ty) + bottom * ty
    }))
}

/// Port of GIMP 3.0.4's `app/paint/gimpheal.c` solver.
pub(super) fn gimp_heal_laplace_loop(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    mask: &[bool],
    cancellation: &AtomicBool,
) -> Result<()> {
    const EPSILON: f32 = 0.1 / 255.0;
    const MAX_ITERATIONS: usize = 500;
    let nmask = mask.iter().filter(|value| **value).count();
    if nmask == 0 {
        return Ok(());
    }
    let relaxation = 2.0 - 1.0 / (0.1575 * (nmask as f32).sqrt() + 0.8);
    let w = relaxation * 0.25;
    for iteration in 0..MAX_ITERATIONS {
        if iteration % 8 == 0 {
            ensure_not_cancelled(cancellation)?;
        }
        let mut error = 0.0f32;
        for parity in 0..2 {
            for y in 0..height {
                let first_x = (y & 1) ^ parity;
                for x in (first_x..width).step_by(2) {
                    let index = y * width + x;
                    if !mask[index] {
                        continue;
                    }
                    let degree = 4
                        - usize::from(x == 0)
                        - usize::from(x + 1 == width)
                        - usize::from(y == 0)
                        - usize::from(y + 1 == height);
                    for channel in 0..3 {
                        let mut neighbors = 0.0;
                        if x > 0 {
                            neighbors += pixels[(index - 1) * 3 + channel];
                        }
                        if x + 1 < width {
                            neighbors += pixels[(index + 1) * 3 + channel];
                        }
                        if y > 0 {
                            neighbors += pixels[(index - width) * 3 + channel];
                        }
                        if y + 1 < height {
                            neighbors += pixels[(index + width) * 3 + channel];
                        }
                        let location = index * 3 + channel;
                        let residual = degree as f32 * w * pixels[location] - w * neighbors;
                        pixels[location] -= residual;
                        error += residual * residual;
                    }
                }
            }
        }
        if error < EPSILON * EPSILON * w * w {
            break;
        }
    }
    Ok(())
}
