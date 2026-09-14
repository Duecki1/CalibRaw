use super::*;
use crate::pipeline::PreviewClippingGpu;

#[derive(Clone, Copy, PartialEq, Eq)]
struct ClippingKey {
    source: egui::TextureId,
    revision: u64,
    shadows: bool,
    highlights: bool,
}

struct ClippingComposite {
    gpu: PreviewClippingGpu,
    texture: egui::TextureId,
    key: Option<ClippingKey>,
}

#[derive(Default)]
pub(crate) struct ClippingState {
    pub(crate) shadows: bool,
    pub(crate) highlights: bool,
    base: Option<ClippingComposite>,
    detail: Option<ClippingComposite>,
}

fn composite_texture(
    slot: &mut Option<ClippingComposite>,
    render: &eframe::egui_wgpu::RenderState,
    source: Option<&RawGpuPipeline>,
    shadows: bool,
    highlights: bool,
) -> Option<egui::TextureId> {
    let source = source.filter(|_| shadows || highlights);
    let Some(source) = source else {
        if let Some(old) = slot.take() {
            render.renderer.write().free_texture(&old.texture);
        }
        return None;
    };
    let key = ClippingKey {
        source: source.egui_texture_id?,
        revision: source.output_revision(),
        shadows,
        highlights,
    };
    if slot
        .as_ref()
        .is_some_and(|cached| cached.gpu.size() != [source.width, source.height])
    {
        let old = slot.take().expect("cached composite exists");
        render.renderer.write().free_texture(&old.texture);
    }
    let cached = slot.get_or_insert_with(|| {
        let gpu = PreviewClippingGpu::new(&render.device, source.width, source.height);
        let texture = render.renderer.write().register_native_texture(
            &render.device,
            gpu.view(),
            eframe::wgpu::FilterMode::Linear,
        );
        ClippingComposite {
            gpu,
            texture,
            key: None,
        }
    });
    if cached.key != Some(key) {
        cached
            .gpu
            .update(&render.device, &render.queue, source, shadows, highlights);
        cached.key = Some(key);
    }
    Some(cached.texture)
}

impl CalibRawApp {
    /// Processing runs after canvas layout; refresh textures already used by this frame.
    pub(super) fn refresh_preview_clipping(&mut self, frame: &eframe::Frame) {
        let Some(render) = frame.wgpu_render_state() else {
            return;
        };
        let state = &mut self.preview.clipping;
        for (cached, source) in [
            (&mut state.base, self.preview.gpu_pipeline.as_ref()),
            (
                &mut state.detail,
                self.preview.detail.as_ref().map(|detail| &detail.pipeline),
            ),
        ] {
            let (Some(cached), Some(source)) = (cached.as_mut(), source) else {
                continue;
            };
            let Some(previous) = cached.key else { continue };
            // A replaced pipeline is picked up during the next canvas layout.
            if Some(previous.source) != source.egui_texture_id {
                continue;
            }
            let key = ClippingKey {
                revision: source.output_revision(),
                shadows: state.shadows,
                highlights: state.highlights,
                ..previous
            };
            if key != previous {
                cached.gpu.update(
                    &render.device,
                    &render.queue,
                    source,
                    key.shadows,
                    key.highlights,
                );
                cached.key = Some(key);
            }
        }
    }

    /// Substitute only the canvas textures. Histogram, picker and export keep the source RGB.
    pub(crate) fn preview_clipping_textures(
        &mut self,
        frame: &eframe::Frame,
    ) -> [Option<egui::TextureId>; 2] {
        let Some(render) = frame.wgpu_render_state() else {
            return [None, None];
        };
        let state = &mut self.preview.clipping;
        let base = composite_texture(
            &mut state.base,
            render,
            self.preview.gpu_pipeline.as_ref(),
            state.shadows,
            state.highlights,
        );
        let detail = composite_texture(
            &mut state.detail,
            render,
            self.preview
                .detail
                .as_ref()
                .filter(|detail| detail.revision == self.preview.revision)
                .map(|detail| &detail.pipeline),
            state.shadows,
            state.highlights,
        );
        [base, detail]
    }
}
