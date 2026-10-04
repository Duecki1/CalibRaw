//! Choosing among SAM's candidate masks using the prompts.

use super::*;

pub(super) fn select_sam_candidate(
    masks: SamTensorData,
    scores: SamTensorData,
    prompt_set: &ObjectPromptSet,
    cache: &ObjectInferenceCache,
) -> Result<DecodedSamMask> {
    let prompts = &prompt_set.prompts;
    anyhow::ensure!(
        masks.shape.len() == 4 && masks.shape[0] == 1,
        "unexpected SAM mask shape {:?}",
        masks.shape
    );
    let candidates = masks.shape[1];
    let height = masks.shape[2];
    let width = masks.shape[3];
    anyhow::ensure!(
        candidates > 0 && width > 0 && height > 0,
        "empty SAM decoder output"
    );
    let plane = width
        .checked_mul(height)
        .context("SAM mask size overflow")?;
    anyhow::ensure!(
        masks.values.len() == candidates * plane,
        "SAM mask tensor length mismatch"
    );
    anyhow::ensure!(
        scores.values.len() >= candidates,
        "SAM score tensor is too short"
    );

    let mut best_index = 0usize;
    let mut best_score = f32::NEG_INFINITY;
    for candidate in 0..candidates {
        let logits = &masks.values[candidate * plane..(candidate + 1) * plane];
        let mut score = scores.values[candidate];
        for prompt in prompts {
            if !prompt.kind.is_foreground() && !prompt.kind.is_background() {
                continue;
            }
            let source_x = prompt.point[0].clamp(0.0, 1.0) * cache.source_width as f32;
            let source_y = prompt.point[1].clamp(0.0, 1.0) * cache.source_height as f32;
            let px = (((source_x - cache.crop.x as f32) / cache.crop.width.max(1) as f32)
                * width as f32)
                .round()
                .clamp(0.0, width.saturating_sub(1) as f32) as usize;
            let py = (((source_y - cache.crop.y as f32) / cache.crop.height.max(1) as f32)
                * height as f32)
                .round()
                .clamp(0.0, height.saturating_sub(1) as f32) as usize;
            let probability = sigmoid_probability(logits[py * width + px]);
            score += if prompt.kind.is_foreground() {
                probability * 0.14
            } else {
                (1.0 - probability) * 0.16
            };
        }
        let (outside_focus, focus_fill, area_ratio) =
            candidate_focus_statistics(logits, width, height, prompt_set.focus, cache);
        score -= outside_focus * 0.95;
        score += focus_fill.min(0.75) * 0.22;
        score -= (area_ratio - 2.0).clamp(0.0, 5.0) * 0.10;
        let border = candidate_border_fraction(logits, width, height);
        score -= border * 0.20;
        if score > best_score {
            best_score = score;
            best_index = candidate;
        }
    }
    let selected_logits = masks.values[best_index * plane..(best_index + 1) * plane].to_vec();
    let probabilities = selected_logits
        .iter()
        .map(|value| sigmoid_probability(*value))
        .collect();
    Ok(DecodedSamMask {
        width: u32::try_from(width).context("SAM output width exceeds u32")?,
        height: u32::try_from(height).context("SAM output height exceeds u32")?,
        probabilities,
        selected_logits,
    })
}

fn candidate_focus_statistics(
    logits: &[f32],
    width: usize,
    height: usize,
    focus: ObjectPromptFocus,
    cache: &ObjectInferenceCache,
) -> (f32, f32, f32) {
    let to_output_x = |normalized: f32| {
        ((normalized.clamp(0.0, 1.0) * cache.source_width as f32 - cache.crop.x as f32)
            / cache.crop.width.max(1) as f32
            * width as f32)
            .clamp(0.0, width as f32)
    };
    let to_output_y = |normalized: f32| {
        ((normalized.clamp(0.0, 1.0) * cache.source_height as f32 - cache.crop.y as f32)
            / cache.crop.height.max(1) as f32
            * height as f32)
            .clamp(0.0, height as f32)
    };
    let min_x = to_output_x(focus.min[0]);
    let max_x = to_output_x(focus.max[0]);
    let min_y = to_output_y(focus.min[1]);
    let max_y = to_output_y(focus.max[1]);
    let focus_area = ((max_x - min_x).max(1.0) * (max_y - min_y).max(1.0)).max(1.0);

    let mut active = 0usize;
    let mut inside = 0usize;
    for y in 0..height {
        for x in 0..width {
            if logits[y * width + x] <= 0.0 {
                continue;
            }
            active += 1;
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if px >= min_x && px <= max_x && py >= min_y && py <= max_y {
                inside += 1;
            }
        }
    }
    if active == 0 {
        return (1.0, 0.0, 0.0);
    }
    let outside_fraction = (active - inside) as f32 / active as f32;
    let focus_fill = inside as f32 / focus_area;
    let area_ratio = active as f32 / focus_area;
    (outside_fraction, focus_fill, area_ratio)
}

fn candidate_border_fraction(logits: &[f32], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let band = (width.min(height) / 64).clamp(2, 12);
    let mut active = 0usize;
    let mut total = 0usize;
    for y in 0..height {
        for x in 0..width {
            if x < band || y < band || x + band >= width || y + band >= height {
                total += 1;
                active += usize::from(logits[y * width + x] > 0.0);
            }
        }
    }
    active as f32 / total.max(1) as f32
}

pub(super) fn mask_touches_crop_border(probabilities: &[f32], width: u32, height: u32) -> bool {
    let logits = probabilities
        .iter()
        .map(|value| if *value >= 0.5 { 1.0 } else { -1.0 })
        .collect::<Vec<_>>();
    candidate_border_fraction(&logits, width as usize, height as usize) > 0.025
}
