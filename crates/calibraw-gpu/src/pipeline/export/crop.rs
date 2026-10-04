//! Developed crops rendered outside a full export: remove-scene sources and linear crops.

use super::*;

/// Renders a complete native Remove crop through one bounded GPU pipeline.
pub fn render_remove_scene_crop_resized(
    job: DevelopedCropJob,
    maximum_edge: u32,
) -> Result<ResizedRemoveSceneCrop> {
    anyhow::ensure!(maximum_edge > 0, "Remove working edge is zero");
    prepare_crop_source(&job)?;
    let working_raw = build_region_proxy(
        &job.raw,
        job.crop.x,
        job.crop.y,
        job.crop.width,
        job.crop.height,
        ProxySpec {
            max_edge: maximum_edge,
        },
    );
    anyhow::ensure!(
        working_raw.width <= maximum_edge && working_raw.height <= maximum_edge,
        "Remove working scene {}x{} exceeds the {}px edge limit",
        working_raw.width,
        working_raw.height,
        maximum_edge
    );

    // Express image-global shader coordinates in the same reduced coordinate
    // system as the working RAW. Existing Remove patches are uploaded below
    // using their native crop mapping, so their placement remains exact.
    let scale_x = f64::from(working_raw.width) / f64::from(job.crop.width);
    let scale_y = f64::from(working_raw.height) / f64::from(job.crop.height);
    let full_width = (f64::from(job.raw.width) * scale_x)
        .round()
        .clamp(f64::from(working_raw.width), f64::from(u32::MAX)) as u32;
    let full_height = (f64::from(job.raw.height) * scale_y)
        .round()
        .clamp(f64::from(working_raw.height), f64::from(u32::MAX)) as u32;
    let origin_x = (f64::from(job.crop.x) * scale_x)
        .round()
        .clamp(0.0, f64::from(i32::MAX)) as i32;
    let origin_y = (f64::from(job.crop.y) * scale_y)
        .round()
        .clamp(0.0, f64::from(i32::MAX)) as i32;

    let empty_masks = MaskStack::default();
    let mask_edge = mask_atlas_edge();
    let params = GpuParams::new_for_tile(
        &job.exposure,
        &empty_masks,
        &working_raw,
        origin_x,
        origin_y,
        full_width,
        full_height,
    );
    let pipeline = create_crop_pipeline(&job, &working_raw, &params, mask_edge)?;

    pipeline.dispatch_stage(&job.queue, &job.device, &params, ProcessingStage::Raw);
    pipeline.upload_remove_scene_patches(
        &job.queue,
        &job.device,
        RemoveSceneContext::new(
            &job.remove,
            &job.raw,
            &job.exposure,
            [job.crop.x as f32, job.crop.y as f32],
            [job.crop.width as f32, job.crop.height as f32],
        ),
    )?;
    let pixels = pipeline.read_scene_texture_blocking(&job.device, &job.queue)?;
    Ok(ResizedRemoveSceneCrop {
        width: working_raw.width,
        height: working_raw.height,
        pixels,
    })
}

pub fn render_remove_scene_crop(job: DevelopedCropJob) -> Result<Vec<f32>> {
    prepare_crop_source(&job)?;
    let empty_masks = MaskStack::default();
    let halo = required_export_tile_halo(&job.exposure, &empty_masks);
    let tile = crate::pipeline::ExportTile {
        core_x: job.crop.x,
        core_y: job.crop.y,
        core_width: job.crop.width,
        core_height: job.crop.height,
        local_core_x: halo,
        local_core_y: halo,
        padded_width: job.crop.width.saturating_add(halo.saturating_mul(2)),
        padded_height: job.crop.height.saturating_add(halo.saturating_mul(2)),
        global_origin_x: job.crop.x as i32 - halo as i32,
        global_origin_y: job.crop.y as i32 - halo as i32,
    };
    let tile_raw = extract_padded_tile(&job.raw, tile);
    let mask_edge = mask_atlas_edge();
    let params = GpuParams::new_for_tile(
        &job.exposure,
        &empty_masks,
        &tile_raw,
        tile.global_origin_x,
        tile.global_origin_y,
        job.raw.width,
        job.raw.height,
    );
    let pipeline = create_crop_pipeline(&job, &tile_raw, &params, mask_edge)?;

    pipeline.dispatch_stage(&job.queue, &job.device, &params, ProcessingStage::Raw);
    pipeline.upload_remove_scene_patches(
        &job.queue,
        &job.device,
        RemoveSceneContext::new(
            &job.remove,
            &job.raw,
            &job.exposure,
            [tile.global_origin_x as f32, tile.global_origin_y as f32],
            [tile.padded_width as f32, tile.padded_height as f32],
        ),
    )?;
    let scene = pipeline.read_scene_texture_blocking(&job.device, &job.queue)?;
    let mut crop = vec![0.0f32; job.crop.width as usize * job.crop.height as usize * 3];
    for y in 0..job.crop.height as usize {
        let source_y = tile.local_core_y as usize + y;
        let source_start = (source_y * tile.padded_width as usize + tile.local_core_x as usize) * 3;
        let source_end = source_start + job.crop.width as usize * 3;
        let destination_start = y * job.crop.width as usize * 3;
        crop[destination_start..destination_start + job.crop.width as usize * 3]
            .copy_from_slice(&scene[source_start..source_end]);
    }
    Ok(crop)
}

/// Rejects an empty crop or one outside the RAW, then prepares the RAW's
/// opposed-chroma highlight data for this exposure when the crop needs it.
fn prepare_crop_source(job: &DevelopedCropJob) -> Result<()> {
    anyhow::ensure!(
        job.crop.width > 0 && job.crop.height > 0,
        "Remove crop is empty"
    );
    anyhow::ensure!(
        job.crop.right() <= job.raw.width && job.crop.bottom() <= job.raw.height,
        "Remove crop lies outside the native source image"
    );
    if job.raw.uses_opposed_chroma(&job.exposure) {
        job.raw.inpaint_opposed_chroma_for_exposure(&job.exposure);
    }
    Ok(())
}

pub struct DevelopedCropJob {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub raw: Arc<LoadedRaw>,
    pub geometry: GeometryTransform,
    pub exposure: ExposureParams,
    pub masks: MaskStack,
    pub remove: RemoveEditState,
    pub crop: NativeRect,
    pub program_prewarm: Option<Arc<GpuProgramPrewarm>>,
}

fn create_crop_pipeline(
    job: &DevelopedCropJob,
    raw: &LoadedRaw,
    params: &GpuParams,
    mask_edge: u32,
) -> Result<RawGpuPipeline> {
    let build = || {
        RawGpuPipeline::new(
            &job.device,
            &job.queue,
            raw,
            params,
            PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(mask_edge),
        )
    };
    let template = job
        .program_prewarm
        .as_deref()
        .and_then(|prewarm| prewarm.wait().ok());
    match template.as_deref() {
        Some(template) => RawGpuPipeline::new(
            &job.device,
            &job.queue,
            raw,
            params,
            PipelineOptions::new(ProcessingQuality::High)
                .mask_atlas_edge(mask_edge)
                .programs(template),
        )
        .or_else(|_| build()),
        None => build(),
    }
}

pub fn render_developed_linear_crop(job: DevelopedCropJob) -> Result<Vec<f32>> {
    prepare_crop_source(&job)?;
    let halo = required_export_tile_halo(&job.exposure, &job.masks);
    let tile = tone_grid_aligned_crop_tile(job.crop, halo)?;
    let tile_raw = extract_padded_tile(&job.raw, tile);
    let mask_region = tile_mask_source_region(
        &job.masks,
        tile.global_origin_x,
        tile.global_origin_y,
        tile.padded_width,
        tile.padded_height,
        job.raw.width,
        job.raw.height,
    );
    let mask_edge = if job.masks.masks.is_empty() {
        mask_atlas_edge()
    } else {
        export_mask_atlas_edge(tile.padded_width, tile.padded_height)
    };
    let mask_extent = mask_region_texture_extent(mask_region, mask_edge);
    let params = GpuParams::new_for_tile(
        &job.exposure,
        &job.masks,
        &tile_raw,
        tile.global_origin_x,
        tile.global_origin_y,
        job.raw.width,
        job.raw.height,
    )
    .with_vignette_geometry(job.geometry)
    .with_mask_uv_rect_and_extent(
        mask_source_region_uv(mask_region, job.raw.width, job.raw.height),
        mask_extent,
    );
    let pipeline = create_crop_pipeline(&job, &tile_raw, &params, mask_edge)?;
    upload_mask_atlas(
        &pipeline,
        &job.queue,
        &job.masks,
        job.raw.width,
        job.raw.height,
        mask_region,
        job.raw.lens_geometry.as_deref(),
    )?;
    pipeline.update_light_rays_mask_layers(
        &job.queue,
        &job.masks,
        job.raw.width,
        job.raw.height,
        job.raw.lens_geometry.as_deref(),
    )?;
    pipeline.dispatch_stage(&job.queue, &job.device, &params, ProcessingStage::Raw);
    pipeline.dispatch_stage(&job.queue, &job.device, &params, ProcessingStage::Tone);
    pipeline.dispatch_stage(&job.queue, &job.device, &params, ProcessingStage::Output);
    let mut rgb = pipeline.read_display_linear_region_blocking(
        &job.device,
        &job.queue,
        tile.local_core_x,
        tile.local_core_y,
        tile.core_width,
        tile.core_height,
    )?;
    crate::pipeline::composite_remove_edits_into_linear_region(&job.remove, job.crop, &mut rgb);
    Ok(rgb)
}
