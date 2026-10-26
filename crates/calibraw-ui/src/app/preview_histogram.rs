use super::*;
use crate::pipeline::{PreviewHistogram, PreviewHistogramGpu};

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq)]
struct HistogramKey {
    /// The fitted preview's texture, which names the image even while the
    /// reduced preview is the one read.
    texture: egui::TextureId,
    output_revision: u64,
    /// The reduced preview is the one read (`shown_pipeline`).
    reduced: bool,
    geometry: GeometryTransform,
    original: bool,
}

impl HistogramKey {
    fn same_view(self, other: Self) -> bool {
        self.texture == other.texture
            && self.geometry == other.geometry
            && self.original == other.original
    }
}

#[derive(Default)]
pub(crate) struct HistogramState {
    pub(crate) visible_this_frame: bool,
    pub(crate) data: Option<PreviewHistogram>,
    pub(crate) error: bool,
    gpu: Option<PreviewHistogramGpu>,
    requested: Option<HistogramKey>,
    displayed: Option<HistogramKey>,
    pending: bool,
    last_request: Option<Instant>,
}

impl CalibRawApp {
    fn histogram_key(&self) -> Option<HistogramKey> {
        let presented = self.preview.gpu_pipeline.as_ref()?;
        let shown = shown_pipeline(Some(presented), &self.preview.interactive)?;
        Some(HistogramKey {
            texture: presented.texture(),
            output_revision: shown.gpu().output_revision(),
            reduced: !std::ptr::eq(shown, presented),
            geometry: self.develop.geometry,
            original: self.preview.original_requested,
        })
    }

    pub(crate) fn current_preview_histogram(&self) -> Option<&PreviewHistogram> {
        let key = self.histogram_key()?;
        self.preview
            .histogram
            .displayed
            .filter(|displayed| displayed.same_view(key))
            .and(self.preview.histogram.data.as_ref())
    }

    pub(super) fn update_preview_histogram(&mut self, frame: &eframe::Frame) {
        let visible = std::mem::take(&mut self.preview.histogram.visible_this_frame);
        let key = self.histogram_key();
        let state = &mut self.preview.histogram;
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };

        // Drain an already-submitted readback even if the panel has just closed.
        if let Some(result) = state
            .gpu
            .as_mut()
            .and_then(|gpu| gpu.poll(&render_state.device))
        {
            state.pending = false;
            if state
                .requested
                .zip(key)
                .is_some_and(|(request, current)| request.same_view(current))
            {
                match result {
                    Ok(data) => {
                        state.data = Some(data);
                        state.displayed = state.requested;
                        state.error = false;
                    }
                    Err(error) => {
                        log::warn!("Preview histogram readback failed: {error:#}");
                        state.error = true;
                    }
                }
                self.egui_ctx.request_repaint();
            }
        }
        let Some(key) = key else {
            state.data = None;
            state.displayed = None;
            state.requested = None;
            state.error = false;
            return;
        };
        if state.pending {
            self.egui_ctx
                .request_repaint_after(Duration::from_millis(16));
            return;
        }
        if !visible || state.requested == Some(key) {
            return;
        }
        if let Some(last) = state.last_request {
            let remaining = UPDATE_INTERVAL.saturating_sub(last.elapsed());
            if !remaining.is_zero() {
                self.egui_ctx.request_repaint_after(remaining);
                return;
            }
        }
        let gpu = state
            .gpu
            .get_or_insert_with(|| PreviewHistogramGpu::new(&render_state.device));
        let source = shown_pipeline(
            self.preview.gpu_pipeline.as_ref(),
            &self.preview.interactive,
        )
        .map(PreviewPipeline::gpu)
        .expect("histogram source exists");
        if gpu.request(
            &render_state.device,
            &render_state.queue,
            source,
            key.geometry,
        ) {
            state.requested = Some(key);
            state.pending = true;
            state.last_request = Some(Instant::now());
            state.error = false;
            self.egui_ctx
                .request_repaint_after(Duration::from_millis(16));
        }
    }
}
