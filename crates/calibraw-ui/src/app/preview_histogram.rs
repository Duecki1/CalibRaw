use super::*;
use crate::pipeline::{PreviewHistogram, PreviewHistogramGpu};

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq)]
struct HistogramKey {
    texture: egui::TextureId,
    output_revision: u64,
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
        let pipeline = self.preview.gpu_pipeline.as_ref()?;
        Some(HistogramKey {
            texture: pipeline.egui_texture_id?,
            output_revision: pipeline.output_revision(),
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
        let source = self
            .preview
            .gpu_pipeline
            .as_ref()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_readback_rejects_other_images_and_views_but_allows_live_edits() {
        let key = HistogramKey {
            texture: egui::TextureId::User(1),
            output_revision: 1,
            geometry: GeometryTransform::default(),
            original: false,
        };
        assert!(key.same_view(HistogramKey {
            output_revision: 2,
            ..key
        }));
        assert!(!key.same_view(HistogramKey {
            texture: egui::TextureId::User(2),
            ..key
        }));
        assert!(!key.same_view(HistogramKey {
            original: true,
            ..key
        }));
        assert!(!key.same_view(HistogramKey {
            geometry: GeometryTransform {
                crop: [0.1, 0.1, 0.8, 0.8],
                ..key.geometry
            },
            ..key
        }));
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn histogram_toggle_is_available_in_every_sidebar_header() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        let frame = eframe::Frame::_new_kittest();
        for tab in [
            SidebarTab::Adjustments,
            SidebarTab::Crop,
            SidebarTab::Masks,
            SidebarTab::Inpainting,
            SidebarTab::Export,
            SidebarTab::Info,
        ] {
            app.ui.sidebar_tab = tab;
            for width in [320.0, 480.0] {
                for open in [true, false] {
                    ctx.data_mut(|data| {
                        data.insert_persisted(egui::Id::new("develop-histogram-open"), open)
                    });
                    app.preview.histogram.visible_this_frame = false;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 800.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            Sidebar::show(ui, &mut app, ScreenLayout::Horizontal, &frame);
                            assert!(
                                ui.min_rect().width() <= width + 1.0,
                                "sidebar overflow for {tab:?}"
                            );
                        },
                    );
                    let text_shapes: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| {
                            if let egui::Shape::Text(text) = &shape.shape {
                                Some(text)
                            } else {
                                None
                            }
                        })
                        .collect();
                    assert!(text_shapes
                        .iter()
                        .any(|text| text.galley.text() == egui_phosphor::regular::CHART_BAR));
                    assert_eq!(
                        text_shapes
                            .iter()
                            .any(|text| text.galley.text() == "Histogram"),
                        open
                    );
                    assert_eq!(app.preview.histogram.visible_this_frame, open);
                }
            }
        }
    }
}
