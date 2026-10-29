//! The reduced preview of the zoomed crop, shown while a control is dragged
//! (`interactive_preview`).
//!
//! It is the sharp crop's part of the photo rendered from a reduction of the
//! crop's raw, so it takes the sharp crop's place on the canvas with the same
//! coordinates, only with fewer pixels.

use super::*;
use crate::i18n::tr;
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
            .context(tr("no photo is open"))?;
        let template = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template)
            .context(tr("the fitted preview is gone"))?;
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
        let params = placement
            .params(
                &self.develop.target_exposure,
                &masks,
                self.develop.geometry,
                reduced,
                &reduced.profile_color,
            )
            .with_look_lut(self.develop.target_look.as_ref());
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
        let params = placement
            .params(
                &self.develop.target_exposure,
                masks,
                self.develop.geometry,
                &reduced.raw,
                &reduced.raw.profile_color,
            )
            .with_look_lut(self.develop.target_look.as_ref());
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
