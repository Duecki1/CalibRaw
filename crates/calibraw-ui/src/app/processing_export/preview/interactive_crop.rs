//! The reduced preview of the zoomed crop, shown while a control is dragged
//! (`interactive_preview`).
//!
//! It is the sharp crop's part of the photo rendered from a reduction of the
//! crop's raw, so it takes the sharp crop's place on the canvas with the same
//! coordinates, only with fewer pixels.

use super::*;
use anyhow::Context as _;

impl CalibRawApp {
    /// The reduced crop of `sharp`, made from `reduced`, a reduction of
    /// `sharp`'s raw. It is not rendered yet. Its masks use half the atlas of
    /// the sharp crop's. `register` makes the graph's texture available to the
    /// canvas.
    pub(in crate::app) fn build_interactive_crop(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        register: impl FnOnce(RawGpuPipeline) -> PreviewPipeline,
        sharp: &PreviewDetail,
        reduced: &Arc<LoadedRaw>,
    ) -> anyhow::Result<PreviewDetail> {
        let full_raw = self
            .develop
            .loaded_raw
            .as_ref()
            .context("no photo is open")?;
        let template = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template)
            .context("the fitted preview is gone")?;
        let masks = self.preview_mask_stack();
        let full_size = [full_raw.width, full_raw.height];
        let (virtual_origin, virtual_full_size) = detail_virtual_frame(
            [reduced.width, reduced.height],
            full_size,
            sharp.source_origin,
            sharp.source_size,
        );
        let mask_atlas_edge = interactive_detail_mask_edge();
        let placement = DetailPlacement {
            virtual_origin,
            virtual_full_size,
            full_size,
            mask_region: sharp.mask_source_region,
            mask_extent: detail_mask_texture_extent(
                sharp.mask_source_region,
                mask_atlas_edge,
                false,
            ),
        };
        let params = placement.params(
            &self.develop.target_exposure,
            &masks,
            self.develop.geometry,
            reduced,
        );
        let pipeline = RawGpuPipeline::new(
            device,
            queue,
            reduced,
            &params,
            detail_pipeline_options(masks.masks.len())
                .mask_atlas_edge(mask_atlas_edge)
                .programs(&template),
        )?;
        Ok(PreviewDetail {
            pipeline: register(pipeline),
            uv_rect: sharp.uv_rect,
            texture_uv_rect: sharp.texture_uv_rect,
            revision: sharp.revision,
            raw: Arc::clone(reduced),
            source_origin: sharp.source_origin,
            source_size: sharp.source_size,
            mask_source_region: placement.mask_region,
            mask_texture_extent: placement.mask_extent,
            virtual_origin,
            virtual_full_size,
            full_source_size: full_size,
            processing_halo: sharp.processing_halo,
        })
    }

    /// Renders `stage` of the reduced crop. While the masks are pending
    /// (`InteractiveState::masks_pending`) they go up first, all of them, as
    /// the crop was built without. Like the sharp crop, it takes the tone
    /// statistics of a full-frame graph.
    pub(in crate::app) fn render_interactive_crop_stage(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        reduced: &PreviewDetail,
        stage: ProcessingStage,
        masks: &MaskStack,
        full_raw: &LoadedRaw,
    ) -> anyhow::Result<()> {
        let placement = reduced.placement();
        if self.preview.interactive.masks_pending() {
            Self::upload_detail_masks(
                reduced.pipeline.gpu(),
                queue,
                masks,
                full_raw,
                placement.mask_region,
                placement.mask_extent,
                None,
            )
            .map_err(anyhow::Error::msg)?;
        }
        let params = placement.params(
            &self.develop.target_exposure,
            masks,
            self.develop.geometry,
            &reduced.raw,
        );
        let full_frame = full_frame_tone_pipeline(
            self.preview.gpu_pipeline.as_ref(),
            self.preview.navigation.as_ref(),
            self.preview.pending_stage,
        );
        dispatch_detail_stage(
            reduced.pipeline.gpu(),
            queue,
            device,
            &params,
            stage,
            full_frame,
            reduced.remove_scene(&self.inpaint.edits, full_raw, &self.develop.target_exposure),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What replaces the reduced crop on release must not jump: the reduced
    /// crop has the sharp crop's place and tone statistics, so it matches the
    /// sharp picture averaged down to its size.
    #[test]
    fn the_reduced_crop_shows_the_sharp_crops_picture_at_half_the_size() {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "reduced crop test",
            adapter: calibraw_gpu::HeadlessAdapter::Software,
            texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
        });
        let (device, queue) = match gpu {
            Ok(gpu) => (gpu.device, gpu.queue),
            Err(calibraw_gpu::HeadlessDeviceError::NoAdapter(_)) => {
                eprintln!("reduced crop test skipped: no headless GPU adapter");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let (width, height) = (320u32, 240u32);
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
                pixels.extend([0.02 + 0.6 * u, 0.02 + 0.4 * v, 0.05 + 0.3 * u * v]);
            }
        }
        let full = Arc::new(LoadedRaw::from_scene_linear_rec2020(width, height, pixels).unwrap());
        let exposure = ExposureParams {
            exposure: 0.7,
            ..ExposureParams::default()
        };
        let masks = MaskStack::default();
        let context = egui::Context::default();
        let mut app = CalibRawApp::empty(&context);
        app.develop.loaded_raw = Some(Arc::clone(&full));
        app.develop.target_exposure = exposure;
        let mut renderer = eframe::egui_wgpu::Renderer::new(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            eframe::egui_wgpu::RendererOptions::default(),
        );

        // The fitted graph supplies the programs and the tone statistics.
        let fitted_raw = Arc::new(build_proxy(&full, ProxySpec { max_edge: 160 }));
        let fitted_params = GpuParams::new(&exposure, &masks, &fitted_raw);
        let fitted = RawGpuPipeline::new(
            &device,
            &queue,
            &fitted_raw,
            &fitted_params,
            fitted_pipeline_options(0),
        )
        .unwrap();
        fitted.recompute(&queue, &device, &fitted_params);
        app.develop.preview_raw = Some(fitted_raw);
        app.preview.gpu_pipeline = Some(PreviewPipeline::register(
            fitted,
            &device,
            &mut renderer,
            &app.preview.retired_textures,
        ));

        // The sharp crop: the middle of the photo at its own density.
        let (origin, size) = ([80, 60], [160, 120]);
        let sharp_raw = Arc::new(crate::pipeline::crop_raw(
            &full, origin[0], origin[1], size[0], size[1],
        ));
        let region = detail_mask_source_region(&masks, origin, size, width, height);
        let (virtual_origin, virtual_full_size) =
            detail_virtual_frame(size, [width, height], origin, size);
        let placement = DetailPlacement {
            virtual_origin,
            virtual_full_size,
            full_size: [width, height],
            mask_region: region,
            mask_extent: detail_mask_texture_extent(region, detail_mask_edge(), false),
        };
        let sharp_params = placement.params(&exposure, &masks, app.develop.geometry, &sharp_raw);
        let sharp_pipeline = RawGpuPipeline::new(
            &device,
            &queue,
            &sharp_raw,
            &sharp_params,
            detail_pipeline_options(0)
                .programs(&app.preview.pipeline().unwrap().program_template()),
        )
        .unwrap();
        let view = PreviewUvRect {
            min: [0.25, 0.25],
            max: [0.75, 0.75],
        };
        let sharp = PreviewDetail {
            pipeline: PreviewPipeline::register(
                sharp_pipeline,
                &device,
                &mut renderer,
                &app.preview.retired_textures,
            ),
            uv_rect: view,
            texture_uv_rect: PreviewUvRect::FULL,
            revision: app.preview.revision,
            raw: sharp_raw,
            source_origin: origin,
            source_size: size,
            mask_source_region: region,
            mask_texture_extent: placement.mask_extent,
            virtual_origin,
            virtual_full_size,
            full_source_size: [width, height],
            processing_halo: 0,
        };
        let render = |app: &CalibRawApp, detail: &PreviewDetail| {
            for stage in [
                ProcessingStage::Raw,
                ProcessingStage::Tone,
                ProcessingStage::Output,
            ] {
                app.render_interactive_crop_stage(&queue, &device, detail, stage, &masks, &full)
                    .unwrap();
            }
            detail
                .pipeline
                .gpu()
                .read_output_region_blocking(
                    &device,
                    &queue,
                    0,
                    0,
                    detail.raw.width,
                    detail.raw.height,
                )
                .unwrap()
        };
        let sharp_picture = render(&app, &sharp);

        // The reduced crop of it.
        let reduced_raw = Arc::new(build_proxy(&sharp.raw, ProxySpec { max_edge: 80 }));
        assert_eq!((reduced_raw.width, reduced_raw.height), (80, 60));
        let reduced = app
            .build_interactive_crop(
                &device,
                &queue,
                |pipeline| {
                    PreviewPipeline::register(
                        pipeline,
                        &device,
                        &mut renderer,
                        &app.preview.retired_textures,
                    )
                },
                &sharp,
                &reduced_raw,
            )
            .unwrap();
        // It covers the same part of the photo and is drawn the same way.
        assert_eq!(reduced.uv_rect, sharp.uv_rect);
        assert_eq!(reduced.texture_uv_rect, sharp.texture_uv_rect);
        assert_eq!(reduced.revision, sharp.revision);
        assert_eq!(
            (reduced.source_origin, reduced.source_size),
            (sharp.source_origin, sharp.source_size)
        );
        // At half the density its frame is half the sharp crop's.
        assert_eq!(reduced.virtual_full_size, [160, 120]);
        assert_eq!(reduced.virtual_origin, [40, 30]);

        let reduced_picture = render(&app, &reduced);
        let sharp_down = crate::pipeline::downscale_display_rgba8(
            &sharp_picture,
            [sharp.raw.width, sharp.raw.height],
            [reduced.raw.width, reduced.raw.height],
        );
        let (mut total, mut count) = (0u64, 0u64);
        for (a, b) in sharp_down
            .chunks_exact(4)
            .zip(reduced_picture.chunks_exact(4))
        {
            for channel in 0..3 {
                total += u64::from(a[channel].abs_diff(b[channel]));
                count += 1;
            }
        }
        let mean = total as f64 / count as f64;
        eprintln!("reduced crop against sharp crop: mean error {mean:.2} of 255");
        assert!(mean < 1.5, "mean error {mean:.2} of 255");

        // While the drag is followed the canvas takes the crop's texture from
        // it, and the fitted one's from the fitted preview.
        let texture = reduced.pipeline.texture();
        let state = InteractiveState::showing_crop(reduced, reduced_raw);
        assert_eq!(state.crop_texture(), Some(texture));
        assert_eq!(state.texture(), None);
        assert_eq!(
            shown_pipeline(app.preview.gpu_pipeline.as_ref(), &state).map(PreviewPipeline::texture),
            app.preview
                .gpu_pipeline
                .as_ref()
                .map(PreviewPipeline::texture)
        );
    }
}
