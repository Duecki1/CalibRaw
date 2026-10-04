//! Post-processing a selected mask: connectivity, hole filling, edge-aware refinement, resizing.

use super::*;

pub(super) fn keep_prompt_connected_component(
    probabilities: Vec<f32>,
    width: u32,
    height: u32,
    prompts: &[ObjectPrompt],
    source_width: u32,
    source_height: u32,
    crop: ObjectCropRect,
) -> Vec<f32> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    let mut visited = vec![false; probabilities.len()];
    let mut keep = vec![false; probabilities.len()];
    let mut stack = Vec::new();

    for prompt in prompts.iter().filter(|prompt| prompt.kind.is_foreground()) {
        let sx = (prompt.point[0].clamp(0.0, 1.0) * source_width as f32 - crop.x as f32)
            / crop.width.max(1) as f32
            * width as f32;
        let sy = (prompt.point[1].clamp(0.0, 1.0) * source_height as f32 - crop.y as f32)
            / crop.height.max(1) as f32
            * height as f32;
        let mut x = sx.round().clamp(0.0, width.saturating_sub(1) as f32) as usize;
        let mut y = sy.round().clamp(0.0, height.saturating_sub(1) as f32) as usize;
        if probabilities[y * width_usize + x] < 0.5 {
            if let Some((nx, ny)) =
                nearest_foreground(&probabilities, width_usize, height_usize, x, y, 16)
            {
                x = nx;
                y = ny;
            } else {
                continue;
            }
        }
        let start = y * width_usize + x;
        if visited[start] {
            continue;
        }
        visited[start] = true;
        stack.push(start);
        while let Some(index) = stack.pop() {
            keep[index] = true;
            let px = index % width_usize;
            let py = index / width_usize;
            for (nx, ny) in neighbors4(px, py, width_usize, height_usize) {
                let next = ny * width_usize + nx;
                if !visited[next] && probabilities[next] >= 0.5 {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
    }

    if !keep.iter().any(|value| *value) {
        let mut largest = Vec::new();
        visited.fill(false);
        for index in 0..probabilities.len() {
            if visited[index] || probabilities[index] < 0.5 {
                continue;
            }
            let mut component = Vec::new();
            visited[index] = true;
            stack.push(index);
            while let Some(current) = stack.pop() {
                component.push(current);
                let px = current % width_usize;
                let py = current / width_usize;
                for (nx, ny) in neighbors4(px, py, width_usize, height_usize) {
                    let next = ny * width_usize + nx;
                    if !visited[next] && probabilities[next] >= 0.5 {
                        visited[next] = true;
                        stack.push(next);
                    }
                }
            }
            if component.len() > largest.len() {
                largest = component;
            }
        }
        for index in largest {
            keep[index] = true;
        }
    }

    fill_enclosed_component_holes(&mut keep, width_usize, height_usize);
    let background = keep.iter().map(|selected| !*selected).collect::<Vec<_>>();
    let near_background = dilate_component_band(&background, width_usize, height_usize, 3);
    let soft_band = dilate_component_band(&keep, width_usize, height_usize, 10);
    probabilities
        .into_iter()
        .enumerate()
        .map(|(index, probability)| {
            if keep[index] {
                if near_background[index] {
                    probability.max(0.82)
                } else {
                    1.0
                }
            } else if soft_band[index] {
                probability.min(0.49)
            } else {
                0.0
            }
        })
        .collect()
}

fn fill_enclosed_component_holes(selected: &mut [bool], width: usize, height: usize) {
    use std::collections::VecDeque;

    if width == 0 || height == 0 || selected.len() != width.saturating_mul(height) {
        return;
    }
    let mut exterior = vec![false; selected.len()];
    let mut queue = VecDeque::new();
    let seed = |x: usize, y: usize, exterior: &mut [bool], queue: &mut VecDeque<usize>| {
        let index = y * width + x;
        if !selected[index] && !exterior[index] {
            exterior[index] = true;
            queue.push_back(index);
        }
    };
    for x in 0..width {
        seed(x, 0, &mut exterior, &mut queue);
        if height > 1 {
            seed(x, height - 1, &mut exterior, &mut queue);
        }
    }
    for y in 0..height {
        seed(0, y, &mut exterior, &mut queue);
        if width > 1 {
            seed(width - 1, y, &mut exterior, &mut queue);
        }
    }
    while let Some(index) = queue.pop_front() {
        let x = index % width;
        let y = index / width;
        for (nx, ny) in neighbors4(x, y, width, height) {
            let next = ny * width + nx;
            if !selected[next] && !exterior[next] {
                exterior[next] = true;
                queue.push_back(next);
            }
        }
    }
    let max_hole_area = (selected.len() / 512).clamp(32, 2048);
    let mut visited_holes = vec![false; selected.len()];
    for start in 0..selected.len() {
        if selected[start] || exterior[start] || visited_holes[start] {
            continue;
        }
        let mut component = Vec::new();
        visited_holes[start] = true;
        queue.push_back(start);
        while let Some(index) = queue.pop_front() {
            component.push(index);
            let x = index % width;
            let y = index / width;
            for (nx, ny) in neighbors4(x, y, width, height) {
                let next = ny * width + nx;
                if !selected[next] && !exterior[next] && !visited_holes[next] {
                    visited_holes[next] = true;
                    queue.push_back(next);
                }
            }
        }
        if component.len() <= max_hole_area {
            for index in component {
                selected[index] = true;
            }
        }
    }
}

fn dilate_component_band(selected: &[bool], width: usize, height: usize, radius: u16) -> Vec<bool> {
    use std::collections::VecDeque;

    let mut distance = vec![u16::MAX; selected.len()];
    let mut queue = VecDeque::new();
    for (index, is_selected) in selected.iter().copied().enumerate() {
        if is_selected {
            distance[index] = 0;
            queue.push_back(index);
        }
    }
    while let Some(index) = queue.pop_front() {
        let next_distance = distance[index].saturating_add(1);
        if next_distance > radius {
            continue;
        }
        let x = index % width;
        let y = index / width;
        for (nx, ny) in neighbors4(x, y, width, height) {
            let next = ny * width + nx;
            if next_distance < distance[next] {
                distance[next] = next_distance;
                queue.push_back(next);
            }
        }
    }
    distance.into_iter().map(|value| value <= radius).collect()
}

fn nearest_foreground(
    probabilities: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: usize,
) -> Option<(usize, usize)> {
    let mut best = None;
    let mut best_distance = usize::MAX;
    let min_x = x.saturating_sub(radius);
    let max_x = (x + radius).min(width.saturating_sub(1));
    let min_y = y.saturating_sub(radius);
    let max_y = (y + radius).min(height.saturating_sub(1));
    for ny in min_y..=max_y {
        for nx in min_x..=max_x {
            if probabilities[ny * width + nx] >= 0.5 {
                let distance = nx.abs_diff(x).pow(2) + ny.abs_diff(y).pow(2);
                if distance < best_distance {
                    best_distance = distance;
                    best = Some((nx, ny));
                }
            }
        }
    }
    best
}

fn neighbors4(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let mut values = [(usize::MAX, usize::MAX); 4];
    let mut count = 0;
    if x > 0 {
        values[count] = (x - 1, y);
        count += 1;
    }
    if x + 1 < width {
        values[count] = (x + 1, y);
        count += 1;
    }
    if y > 0 {
        values[count] = (x, y - 1);
        count += 1;
    }
    if y + 1 < height {
        values[count] = (x, y + 1);
        count += 1;
    }
    values.into_iter().take(count)
}

pub(super) fn edge_aware_refine(
    mut mask: Vec<f32>,
    width: u32,
    height: u32,
    rgba: &[u8],
    strength: f32,
) -> Vec<f32> {
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.001 || rgba.len() != width as usize * height as usize * 4 {
        return mask;
    }
    let radius = (2.0 + strength * 4.0).round() as i32;
    let iterations = 2;
    let sigma_space = radius.max(1) as f32 * 0.75;
    let sigma_color = 0.04 + (1.0 - strength) * 0.10;
    let width_usize = width as usize;
    let height_usize = height as usize;

    for _ in 0..iterations {
        let source = mask.clone();
        mask.par_chunks_mut(width_usize)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, output) in row.iter_mut().enumerate() {
                    let index = y * width_usize + x;
                    let value = source[index];
                    if !(0.02..=0.98).contains(&value) {
                        continue;
                    }
                    let base = index * 4;
                    let base_rgb = [
                        rgba[base] as f32 / 255.0,
                        rgba[base + 1] as f32 / 255.0,
                        rgba[base + 2] as f32 / 255.0,
                    ];
                    let mut weighted = 0.0;
                    let mut total = 0.0;
                    for dy in -radius..=radius {
                        let ny = (y as i32 + dy).clamp(0, height_usize as i32 - 1) as usize;
                        for dx in -radius..=radius {
                            let nx = (x as i32 + dx).clamp(0, width_usize as i32 - 1) as usize;
                            let neighbor = ny * width_usize + nx;
                            let rgb_index = neighbor * 4;
                            let dr = rgba[rgb_index] as f32 / 255.0 - base_rgb[0];
                            let dg = rgba[rgb_index + 1] as f32 / 255.0 - base_rgb[1];
                            let db = rgba[rgb_index + 2] as f32 / 255.0 - base_rgb[2];
                            let spatial = (dx * dx + dy * dy) as f32;
                            let color = dr * dr + dg * dg + db * db;
                            let weight = (-spatial / (2.0 * sigma_space * sigma_space)).exp()
                                * (-color / (2.0 * sigma_color * sigma_color)).exp();
                            weighted += source[neighbor] * weight;
                            total += weight;
                        }
                    }
                    if total > 0.0 {
                        let filtered = weighted / total;
                        *output = value + (filtered - value) * (0.45 + strength * 0.45);
                    }
                }
            });
    }
    mask
}

pub(in crate::ai_masks) fn resize_probability_u8(
    values: &[f32],
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<u8> {
    resize_f32(values, width, height, target_width, target_height)
        .into_iter()
        .map(|value| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect()
}

pub(super) fn resize_f32(
    values: &[f32],
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<f32> {
    if width == 0 || height == 0 || target_width == 0 || target_height == 0 {
        return Vec::new();
    }
    let mut output = vec![0.0; target_width as usize * target_height as usize];
    for y in 0..target_height {
        let source_y = ((y as f32 + 0.5) * height as f32 / target_height as f32 - 0.5)
            .clamp(0.0, height.saturating_sub(1) as f32);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(height as usize - 1);
        let fy = source_y - y0 as f32;
        for x in 0..target_width {
            let source_x = ((x as f32 + 0.5) * width as f32 / target_width as f32 - 0.5)
                .clamp(0.0, width.saturating_sub(1) as f32);
            let x0 = source_x.floor() as usize;
            let x1 = (x0 + 1).min(width as usize - 1);
            let fx = source_x - x0 as f32;
            let top = values[y0 * width as usize + x0]
                + (values[y0 * width as usize + x1] - values[y0 * width as usize + x0]) * fx;
            let bottom = values[y1 * width as usize + x0]
                + (values[y1 * width as usize + x1] - values[y1 * width as usize + x0]) * fx;
            output[y as usize * target_width as usize + x as usize] = top + (bottom - top) * fy;
        }
    }
    output
}
