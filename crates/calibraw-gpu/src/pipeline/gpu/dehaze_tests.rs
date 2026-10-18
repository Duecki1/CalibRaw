//! Dehaze (haze.wgsl, haze_analysis.wgsl) on synthetic scenes built from the
//! scattering model I = J t + A (1 - t) with a known airlight A and
//! transmission t.

use super::{
    tests::request_test_device, FullImageMap, GpuParams, PipelineOptions, ProcessingQuality,
    RawGpuPipeline, RemoveSceneContext,
};
use crate::pipeline::{
    extract_padded_tile, required_export_tile_halo, ExportTile, ExposureParams, LoadedRaw,
    LocalMask, MaskKind, MaskStack, ProcessingStage, RemoveEditState, TilePlan, TileSpec,
};

// Large enough for haze grid cells of several boxes, so cells see detail.
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const MASK_EDGE: u32 = 64;
const EPSILON: f32 = 2.0e-4;
/// Rows above this are sky, which is the airlight itself.
const HORIZON: u32 = 144;
const AIRLIGHT: [f32; 3] = [0.70, 0.75, 0.82];
/// Saturated haze-free colours, each with a channel of 0.01.
const GROUND: [[f32; 3]; 6] = [
    [0.45, 0.06, 0.01],
    [0.01, 0.30, 0.06],
    [0.04, 0.08, 0.40],
    [0.30, 0.25, 0.01],
    [0.01, 0.01, 0.01],
    [0.25, 0.01, 0.20],
];

/// The scene without haze below the horizon: fine detail, 2-pixel blocks of
/// `GROUND` in a scrambled order.
fn ground(x: u32, y: u32) -> [f32; 3] {
    let hash = (x / 2).wrapping_mul(73_856_093) ^ (y / 2).wrapping_mul(19_349_663);
    GROUND[(hash >> 7) as usize % GROUND.len()]
}

/// Transmission of the hazy scene: none in the sky, 0.3 at the horizon and
/// 0.9 at the bottom.
fn transmission(y: u32) -> f32 {
    if y < HORIZON {
        return 0.0;
    }
    0.3 + 0.6 * (y - HORIZON) as f32 / (HEIGHT - 1 - HORIZON) as f32
}

fn clear_scene(x: u32, y: u32) -> [f32; 3] {
    if y < HORIZON {
        AIRLIGHT
    } else {
        ground(x, y)
    }
}

fn hazy_scene(x: u32, y: u32) -> [f32; 3] {
    let t = transmission(y);
    let clear = ground(x, y);
    std::array::from_fn(|c| clear[c] * t + AIRLIGHT[c] * (1.0 - t))
}

/// Rec.2020 luminance weights (`Common::LUMA`).
const LUMA: [f32; 3] = [0.262_700_2, 0.677_998_1, 0.059_301_7];
/// Brightnesses of `airlight_ground`'s blocks, relative to the airlight.
const AIRLIGHT_GROUND: [f32; 2] = [0.4, 0.7];
/// Relative amplitude of the pixel noise in `noisy`.
const NOISE: f32 = 0.02;
/// Columns of the colour fringes in `fringed`: one in every `FRINGE_PERIOD`,
/// further apart than the guide scene reaches.
const FRINGE_PERIOD: u32 = 8;
const FRINGE_COLUMN: u32 = 3;
/// Amplitude of the fringes in `fringed`.
const FRINGE: f32 = 0.03;

/// Hazy ground of the airlight's own colour, in 2-pixel blocks of two
/// brightnesses. Every colour lies on the line through the airlight, so
/// Dehaze can change nothing but the distance from it.
fn airlight_ground(x: u32, y: u32) -> [f32; 3] {
    let t = transmission(y);
    let hash = (x / 2).wrapping_mul(73_856_093) ^ (y / 2).wrapping_mul(19_349_663);
    let brightness = AIRLIGHT_GROUND[(hash >> 7) as usize % AIRLIGHT_GROUND.len()];
    AIRLIGHT.map(|a| a * (brightness * t + 1.0 - t))
}

/// `airlight_ground` with luminance noise alternating from pixel to pixel,
/// the finest a sensor makes.
fn noisy(x: u32, y: u32) -> [f32; 3] {
    let sign = if (x + y).is_multiple_of(2) { 1.0 } else { -1.0 };
    airlight_ground(x, y).map(|c| c * (1.0 + sign * NOISE))
}

/// `airlight_ground` with one-pixel colour fringes of no luminance below the
/// horizon, as demosaicing and chromatic aberration leave along fine detail.
fn fringed(x: u32, y: u32) -> [f32; 3] {
    let mut rgb = airlight_ground(x, y);
    if y >= HORIZON && x % FRINGE_PERIOD == FRINGE_COLUMN {
        rgb[0] += FRINGE * LUMA[1];
        rgb[1] -= FRINGE * LUMA[0];
    }
    rgb
}

struct Fixture {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    exposure: ExposureParams,
    pipeline: RawGpuPipeline,
}

impl Fixture {
    fn new(pixel: impl Fn(u32, u32) -> [f32; 3]) -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            eprintln!("Dehaze GPU regression skipped: no headless wgpu adapter");
            return Ok(None);
        };
        let pixels = (0..WIDTH * HEIGHT)
            .flat_map(|i| pixel(i % WIDTH, i / WIDTH))
            .collect();
        let source = LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, pixels)?;
        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        let pipeline = RawGpuPipeline::new(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &MaskStack::default(), &source),
            PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(MASK_EDGE),
        )?;
        Ok(Some(Self {
            device,
            queue,
            source,
            exposure,
            pipeline,
        }))
    }

    fn render(&mut self, dehaze: f32) -> anyhow::Result<Vec<f32>> {
        self.exposure.dehaze = dehaze;
        self.render_masks(&MaskStack::default())
    }

    fn render_masks(&self, masks: &MaskStack) -> anyhow::Result<Vec<f32>> {
        self.pipeline.recompute(
            &self.queue,
            &self.device,
            &GpuParams::new(&self.exposure, masks, &self.source),
        );
        let result = self.pipeline.read_display_linear_region_blocking(
            &self.device,
            &self.queue,
            0,
            0,
            WIDTH,
            HEIGHT,
        )?;
        assert!(
            result.iter().all(|v| v.is_finite()),
            "non-finite rendered value"
        );
        Ok(result)
    }

    /// The resolved airlight: colour and luminance.
    fn airlight(&self) -> anyhow::Result<[f32; 4]> {
        let resources = self
            .pipeline
            .effect_inputs
            .full_image_map(FullImageMap::Haze)
            .ok_or_else(|| anyhow::anyhow!("the haze map is not allocated"))?;
        let airlight = &resources.buffers[0];
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("calibraw test airlight readback"),
            size: airlight.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("calibraw test airlight copy"),
            });
        encoder.copy_buffer_to_buffer(airlight, 0, &readback, 0, airlight.size());
        let submission = self.queue.submit(Some(encoder.finish()));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })?;
        receiver.recv()??;
        let values = bytemuck::pod_read_unaligned(&readback.get_mapped_range(..)[..16]);
        readback.unmap();
        Ok(values)
    }
}

fn pixel(rgb: &[f32], x: u32, y: u32) -> &[f32] {
    let i = ((y * WIDTH + x) * 3) as usize;
    &rgb[i..i + 3]
}

/// Largest channel difference over rows `rows`.
fn difference_in_rows(a: &[f32], b: &[f32], rows: std::ops::Range<u32>) -> f32 {
    rows.flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(a, x, y).iter().zip(pixel(b, x, y)))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

/// Mean channel difference over rows `rows`.
fn mean_difference_in_rows(a: &[f32], b: &[f32], rows: std::ops::Range<u32>) -> f32 {
    let count = (rows.len() as u32 * WIDTH * 3) as f32;
    rows.flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(a, x, y).iter().zip(pixel(b, x, y)))
        .map(|(a, b)| (a - b).abs())
        .sum::<f32>()
        / count
}

fn luminance(rgb: &[f32]) -> f32 {
    rgb.iter().zip(LUMA).map(|(c, w)| c * w).sum()
}

fn difference(a: &[f32], b: &[f32]) -> [f32; 3] {
    std::array::from_fn(|c| a[c] - b[c])
}

/// Renders `scene` and the same scene with `pixel` instead, without Dehaze
/// and at full Dehaze.
struct Pair {
    before: Vec<f32>,
    after: Vec<f32>,
    altered_before: Vec<f32>,
    altered_after: Vec<f32>,
}

impl Pair {
    fn render(
        scene: impl Fn(u32, u32) -> [f32; 3],
        altered: impl Fn(u32, u32) -> [f32; 3],
    ) -> anyhow::Result<Option<Self>> {
        let (Some(mut scene), Some(mut altered)) = (Fixture::new(scene)?, Fixture::new(altered)?)
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            before: scene.render(0.0)?,
            after: scene.render(100.0)?,
            altered_before: altered.render(0.0)?,
            altered_after: altered.render(100.0)?,
        }))
    }

    /// The gain Dehaze applied to the unaltered scene at a pixel: how much
    /// further from the airlight, the unchanged sky, it moved the luminance.
    fn gain(&self, x: u32, y: u32) -> f32 {
        let sky = luminance(pixel(&self.before, x, HORIZON / 2));
        (luminance(pixel(&self.after, x, y)) - sky) / (luminance(pixel(&self.before, x, y)) - sky)
    }

    /// The alteration at a pixel without Dehaze and at full Dehaze.
    fn alteration(&self, x: u32, y: u32) -> ([f32; 3], [f32; 3]) {
        (
            difference(pixel(&self.altered_before, x, y), pixel(&self.before, x, y)),
            difference(pixel(&self.altered_after, x, y), pixel(&self.after, x, y)),
        )
    }

    /// The pixels of `columns` in dense haze, where full removal at least
    /// doubles the departure from the airlight, with that gain. Fails unless
    /// there are many.
    fn dense_haze(&self, columns: impl Iterator<Item = u32> + Clone) -> Vec<(u32, u32, f32)> {
        let dense: Vec<_> = (HORIZON + 8..HORIZON + 72)
            .flat_map(|y| columns.clone().map(move |x| (x, y)))
            .map(|(x, y)| (x, y, self.gain(x, y)))
            .filter(|&(_, _, gain)| gain >= 2.0)
            .collect();
        assert!(dense.len() > 500, "too little dense haze: {}", dense.len());
        dense
    }
}

fn full_mask(dehaze: f32) -> MaskStack {
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.adjustments.dehaze = dehaze;
    MaskStack {
        masks: vec![mask],
        ..Default::default()
    }
}

fn fill_mask_layer(scene: &Fixture, weight: half::f16) -> anyhow::Result<()> {
    scene.pipeline.update_mask_layer(
        &scene.queue,
        0,
        &vec![weight.to_bits(); (MASK_EDGE * MASK_EDGE) as usize],
    )
}

#[test]
fn haze_map_is_allocated_only_for_dehaze() -> anyhow::Result<()> {
    let Some(mut scene) = Fixture::new(hazy_scene)? else {
        return Ok(());
    };
    scene.render(0.0)?;
    assert!(scene
        .pipeline
        .effect_inputs
        .full_image_map(FullImageMap::Haze)
        .is_none());
    scene.render(50.0)?;
    assert!(scene
        .pipeline
        .effect_inputs
        .full_image_map(FullImageMap::Haze)
        .is_some());
    Ok(())
}

#[test]
fn dehaze_leaves_haze_free_detail_unchanged() -> anyhow::Result<()> {
    let Some(mut scene) = Fixture::new(clear_scene)? else {
        return Ok(());
    };
    let untouched = scene.render(0.0)?;
    let dehazed = scene.render(100.0)?;
    // The sky is found as the airlight, so the comparison is not vacuous.
    let airlight = scene.airlight()?;
    assert!(airlight[3] > 0.5, "no airlight found: {airlight:?}");
    // Every ground colour holds a channel far darker than the airlight, so
    // the map finds no haze there, and the guide scene stops at the horizon
    // rather than lending the ground the sky's veil.
    let ground_change = difference_in_rows(&dehazed, &untouched, HORIZON..HEIGHT);
    assert!(
        ground_change < EPSILON,
        "Dehaze changed haze-free ground by {ground_change}"
    );
    Ok(())
}

#[test]
fn dehaze_recovers_a_synthetic_hazy_scene() -> anyhow::Result<()> {
    let Some(mut clear) = Fixture::new(clear_scene)? else {
        return Ok(());
    };
    let Some(mut hazy) = Fixture::new(hazy_scene)? else {
        return Ok(());
    };
    let target = clear.render(0.0)?;
    let before = hazy.render(0.0)?;
    let after = hazy.render(100.0)?;

    let airlight = hazy.airlight()?;
    for (channel, (found, expected)) in airlight.iter().zip(AIRLIGHT).enumerate() {
        assert!(
            (found - expected).abs() < 0.01 * expected,
            "airlight channel {channel}: {found} instead of {expected}"
        );
    }

    // From the densest haze below the horizon to light haze near the bottom.
    // Full removal stops short of the clear scene where the haze is densest,
    // as the gain limits demand, but removes most of the haze overall.
    let band = HORIZON + 24..HEIGHT - 24;
    let hazy_error = mean_difference_in_rows(&before, &target, band.clone());
    let dehazed_error = mean_difference_in_rows(&after, &target, band);
    assert!(
        dehazed_error < 0.45 * hazy_error,
        "Dehaze left {dehazed_error} of the haze's {hazy_error} mean error"
    );
    // The sky is the airlight and stays as it is.
    let sky_change = difference_in_rows(&after, &before, 0..HORIZON - 8);
    assert!(sky_change < 1e-3, "Dehaze changed the sky by {sky_change}");
    Ok(())
}

#[test]
fn dehaze_amplifies_pixel_noise_by_the_gain_of_the_haze_around_it() -> anyhow::Result<()> {
    let Some(pair) = Pair::render(airlight_ground, noisy)? else {
        return Ok(());
    };
    // The gain comes from the guide scene, which the alternating noise barely
    // reaches, and the pixel enters the model linearly: noise is amplified as
    // much as the haze it sits in, not by a gain of its own. Only across the
    // ground's blocks does the pixel's noise shift the guide scene's range
    // weights, by up to about a tenth. A gain that followed the pixel
    // amplified this noise over twice as much as the haze.
    let largest_mismatch = pair
        .dense_haze(0..WIDTH)
        .into_iter()
        .map(|(x, y, gain)| {
            let (before, after) = pair.alteration(x, y);
            (luminance(&after) / luminance(&before) / gain - 1.0).abs()
        })
        .fold(0.0, f32::max);
    assert!(
        largest_mismatch < 0.15,
        "noise gain departs from the haze's gain by {largest_mismatch}"
    );
    Ok(())
}

#[test]
fn dehaze_amplifies_color_fringes_less_than_the_haze() -> anyhow::Result<()> {
    let Some(pair) = Pair::render(airlight_ground, fringed)? else {
        return Ok(());
    };
    // Colour detail finer than the guide scene takes a reduced gain, yet the
    // veil over it is still thinned.
    let norm = |rgb: [f32; 3]| rgb.iter().map(|c| c * c).sum::<f32>().sqrt();
    let amplifications: Vec<_> = pair
        .dense_haze((FRINGE_COLUMN..WIDTH).step_by(FRINGE_PERIOD as usize))
        .into_iter()
        .map(|(x, y, gain)| {
            let (before, after) = pair.alteration(x, y);
            (norm(after) / norm(before), gain)
        })
        .collect();
    let largest_share = amplifications
        .iter()
        .map(|(amplification, gain)| amplification / gain)
        .fold(0.0, f32::max);
    assert!(
        largest_share < 0.85,
        "fringes grew by up to {largest_share} of the haze's gain"
    );
    let smallest = amplifications
        .iter()
        .map(|(amplification, _)| *amplification)
        .fold(f32::INFINITY, f32::min);
    assert!(smallest > 1.0, "fringes faded under Dehaze: {smallest}");
    Ok(())
}

#[test]
fn negative_dehaze_thickens_the_veil_more_where_haze_is_dense() -> anyhow::Result<()> {
    let Some(mut scene) = Fixture::new(hazy_scene)? else {
        return Ok(());
    };
    let untouched = scene.render(0.0)?;
    let sky = pixel(&untouched, WIDTH / 2, HORIZON / 2).to_vec();
    let distance_to_sky = |rgb: &[f32], rows: std::ops::Range<u32>| {
        let count = (rows.len() as u32 * WIDTH * 3) as f32;
        rows.flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
            .flat_map(|(x, y)| pixel(rgb, x, y).iter().zip(&sky))
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / count
    };
    let dense = HORIZON + 8..HORIZON + 40;
    let light = HEIGHT - 40..HEIGHT - 8;
    let half = scene.render(-50.0)?;
    let full = scene.render(-100.0)?;
    for rows in [dense.clone(), light.clone()] {
        let (none, half, full) = (
            distance_to_sky(&untouched, rows.clone()),
            distance_to_sky(&half, rows.clone()),
            distance_to_sky(&full, rows.clone()),
        );
        assert!(
            full < half && half < none,
            "rows {rows:?}: the veil does not thicken with the amount: {none} {half} {full}"
        );
    }
    // Light haze still gains a veil; dense haze gains relatively more.
    let kept = |rows: std::ops::Range<u32>| {
        distance_to_sky(&full, rows.clone()) / distance_to_sky(&untouched, rows)
    };
    assert!(kept(light.clone()) < 0.9, "light haze gained no veil");
    assert!(kept(dense) < kept(light), "dense haze gained less veil");
    Ok(())
}

#[test]
fn mask_dehaze_matches_global_dehaze_and_honours_empty_masks() -> anyhow::Result<()> {
    let Some(mut scene) = Fixture::new(hazy_scene)? else {
        return Ok(());
    };
    let untouched = scene.render(0.0)?;
    let global = scene.render(60.0)?;
    scene.exposure.dehaze = 0.0;

    fill_mask_layer(&scene, half::f16::ZERO)?;
    let empty = scene.render_masks(&full_mask(60.0))?;
    assert!(
        difference_in_rows(&empty, &untouched, 0..HEIGHT) < EPSILON,
        "Dehaze in an empty mask changed the photo"
    );
    fill_mask_layer(&scene, half::f16::ONE)?;
    let masked = scene.render_masks(&full_mask(60.0))?;
    assert!(
        difference_in_rows(&masked, &global, 0..HEIGHT) < EPSILON,
        "Dehaze in a full mask differs from global Dehaze"
    );
    Ok(())
}

#[test]
fn dehaze_keeps_extreme_inputs_finite_and_non_negative() -> anyhow::Result<()> {
    let Some(mut scene) = Fixture::new(|x, y| {
        let sun = (x as f32 - 60.0).hypot(y as f32 - 30.0) < 10.0;
        match () {
            _ if sun => [50.0, 48.0, 40.0],
            _ if y < HORIZON => AIRLIGHT,
            _ if x < 40 => [0.0; 3],
            _ if x < 80 => [8.0, 6.0, 4.0],
            _ if x < 120 => [-0.05, 0.3, 0.2],
            _ => hazy_scene(x, y),
        }
    })?
    else {
        return Ok(());
    };
    for dehaze in [100.0, -100.0] {
        let rendered = scene.render(dehaze)?;
        let lowest = rendered.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(lowest > -1e-4, "Dehaze {dehaze} rendered {lowest}");
    }
    Ok(())
}

#[test]
fn dehaze_export_tiles_and_crops_match_the_full_frame() -> anyhow::Result<()> {
    let Some(mut scene) = Fixture::new(hazy_scene)? else {
        return Ok(());
    };
    scene.exposure.dehaze = 80.0;
    // A half-covering mask that thins the haze removal.
    let masks = full_mask(-40.0);
    fill_mask_layer(&scene, half::f16::from_f32(0.5))?;
    let full = scene.render_masks(&masks)?;
    let expected_core = |tile: ExportTile| -> Vec<f32> {
        (tile.core_y..tile.core_y + tile.core_height)
            .flat_map(|row| {
                let start = ((row * WIDTH + tile.core_x) * 3) as usize;
                full[start..start + (tile.core_width * 3) as usize]
                    .iter()
                    .copied()
            })
            .collect()
    };
    let assert_core = |actual: &[f32], tile: ExportTile, what: &str| {
        let largest = actual
            .iter()
            .zip(expected_core(tile))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            largest < 1e-3,
            "{what} at {},{} differs from the full frame by {largest}",
            tile.core_x,
            tile.core_y
        );
    };
    let tile_params = |raw: &LoadedRaw, tile: ExportTile| {
        GpuParams::new_for_tile(
            &scene.exposure,
            &masks,
            raw,
            tile.global_origin_x,
            tile.global_origin_y,
            WIDTH,
            HEIGHT,
        )
    };

    // The export prepass: every tile adds its core to the shared haze grid.
    let halo = required_export_tile_halo(&scene.exposure, &masks);
    let plan = TilePlan::new(
        WIDTH,
        HEIGHT,
        TileSpec {
            core_edge: 200,
            halo,
        },
    );
    let first = extract_padded_tile(&scene.source, plan.tiles[0]);
    let pipeline = RawGpuPipeline::new(
        &scene.device,
        &scene.queue,
        &first,
        &tile_params(&first, plan.tiles[0]),
        PipelineOptions::new(ProcessingQuality::High)
            .mask_atlas_edge(MASK_EDGE)
            .programs(&scene.pipeline.program_template()),
    )?;
    pipeline.update_mask_layer(
        &scene.queue,
        0,
        &vec![half::f16::from_f32(0.5).to_bits(); (MASK_EDGE * MASK_EDGE) as usize],
    )?;
    let remove = RemoveEditState::default();
    let context = |tile: ExportTile| {
        RemoveSceneContext::new(
            &remove,
            &scene.source,
            &scene.exposure,
            [tile.global_origin_x as f32, tile.global_origin_y as f32],
            [tile.padded_width as f32, tile.padded_height as f32],
        )
    };
    pipeline.begin_export_tone_analysis(&scene.queue, &scene.device);
    for &tile in &plan.tiles {
        let raw = extract_padded_tile(&scene.source, tile);
        pipeline.upload_raw_tile(&scene.queue, &raw)?;
        let params = tile_params(&raw, tile).with_global_tone_histogram_bounds(
            tile.core_x,
            tile.core_y,
            tile.core_width,
            tile.core_height,
        );
        pipeline.accumulate_export_tone_tile_with_remove(
            &scene.queue,
            &scene.device,
            &params,
            context(tile),
        )?;
    }
    pipeline.finish_export_tone_analysis(&scene.queue, &scene.device);
    for &tile in &plan.tiles {
        let raw = extract_padded_tile(&scene.source, tile);
        pipeline.upload_raw_tile(&scene.queue, &raw)?;
        pipeline.dispatch_export_tile_with_remove(
            &scene.queue,
            &scene.device,
            &tile_params(&raw, tile),
            context(tile),
        )?;
        let actual = pipeline.read_display_linear_region_blocking(
            &scene.device,
            &scene.queue,
            tile.local_core_x,
            tile.local_core_y,
            tile.core_width,
            tile.core_height,
        )?;
        assert_core(&actual, tile, "export tile");
    }

    // A cropped view takes the full frame's airlight and map.
    let crop = ExportTile {
        core_x: 300,
        core_y: 120,
        core_width: 160,
        core_height: 120,
        local_core_x: halo,
        local_core_y: halo,
        padded_width: 160 + 2 * halo,
        padded_height: 120 + 2 * halo,
        global_origin_x: 300 - halo as i32,
        global_origin_y: 120 - halo as i32,
    };
    let raw = extract_padded_tile(&scene.source, crop);
    let params = tile_params(&raw, crop);
    let cropped = RawGpuPipeline::new(
        &scene.device,
        &scene.queue,
        &raw,
        &params,
        PipelineOptions::new(ProcessingQuality::High)
            .mask_atlas_edge(MASK_EDGE)
            .programs(&scene.pipeline.program_template()),
    )?;
    cropped.update_mask_layer(
        &scene.queue,
        0,
        &vec![half::f16::from_f32(0.5).to_bits(); (MASK_EDGE * MASK_EDGE) as usize],
    )?;
    cropped.dispatch_stage(&scene.queue, &scene.device, &params, ProcessingStage::Raw);
    cropped.dispatch_tone_guide_with_inherited_statistics(
        &scene.queue,
        &scene.device,
        &params,
        &scene.pipeline,
    );
    cropped.dispatch_stage(
        &scene.queue,
        &scene.device,
        &params,
        ProcessingStage::Output,
    );
    let actual = cropped.read_display_linear_region_blocking(
        &scene.device,
        &scene.queue,
        crop.local_core_x,
        crop.local_core_y,
        crop.core_width,
        crop.core_height,
    )?;
    assert_core(&actual, crop, "cropped view");
    Ok(())
}
