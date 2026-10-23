//! The reduced preview shown while a control is dragged.
//!
//! On a GPU that needs more than a few frames for the preview, a slider drag
//! falls far behind the pointer. While a control is dragged, a second graph at
//! half the edge (a quarter of the pixels and of the GPU time) follows it, and
//! the sharp preview waits. When the drag ends, the sharp preview renders
//! everything it missed (`queue_preview_processing` keeps merging the stage it
//! owes while it waits) and replaces the reduced one once it is done.
//!
//! What is reduced depends on the view. The photo is shown whole by the fitted
//! preview, so the reduced graph is the whole photo at half the fitted edge.
//! Zoomed in, the sharp crop (`PreviewDetail`) covers the view and is the
//! larger graph, so the reduced graph is that same part of the photo at half
//! the crop's edge, drawn with the crop's coordinates
//! (`processing_export::preview::interactive_crop`).
//!
//! Which GPUs need it is measured, not assumed (`gpu_is_slow`): the dispatch
//! gate times the batches of the preview that is shown.
//!
//! The reduced graph is built from the sharp one's source on a worker once the
//! editor is quiet, so a drag finds it ready; one that starts earlier just
//! uses the sharp preview. There is one at a time: changing the zoom drops it
//! and builds the other, so it never adds more than a quarter of a graph.

use super::preview_dispatch_gate::Batch;
use super::*;

/// The smallest source edge (the fitted proxy, or the zoomed crop's raw) that
/// gets a reduced graph; half of it must still reach the smallest proxy
/// (`MIN_PROXY_EDGE`).
const MIN_SOURCE_EDGE: u32 = 1280;

/// A median fitted batch time above this makes the reduced preview follow
/// drags. At about 22 frames a second a drag starts to feel behind.
const ENGAGE_ABOVE_MS: f32 = 45.0;

/// A median below this lets the fitted preview follow drags again. The gap
/// keeps the choice from flipping on noise.
const RELEASE_BELOW_MS: f32 = 25.0;

/// How long the fitted preview has to be current before it replaces the
/// reduced one, so a final value that arrives late is not shown twice.
const SETTLE_DELAY: Duration = Duration::from_millis(80);

/// A drag whose value stays put this long lets the fitted preview catch up
/// while the pointer is still down.
const HOLD_STILL: Duration = Duration::from_millis(300);

/// How long after a drag ends or pauses the reduced preview goes on owning the
/// edits before the sharp preview starts to catch up. The GPU cannot take back
/// a batch it was given, so a control grabbed again within it finds the GPU
/// free instead of busy with a sharp render that the next edit outdates.
const CATCH_UP_DELAY: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InteractiveMode {
    /// Follow drags with the reduced preview where the GPU is slow.
    Auto,
    Always,
    Off,
}

/// `CALIBRAW_INTERACTIVE_PREVIEW=off|auto|always` overrides the choice on the
/// desktop, to compare the two paths on a fast GPU.
fn interactive_mode() -> InteractiveMode {
    static MODE: std::sync::OnceLock<InteractiveMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(
        || match std::env::var("CALIBRAW_INTERACTIVE_PREVIEW").as_deref() {
            Ok("always") => InteractiveMode::Always,
            Ok("off") => InteractiveMode::Off,
            _ => InteractiveMode::Auto,
        },
    )
}

/// The edge of the reduced graph for a source of `source_edge`, or `None`
/// where the source is small enough already.
fn interactive_edge(source_edge: u32) -> Option<u32> {
    (source_edge >= MIN_SOURCE_EDGE).then_some(source_edge / 2)
}

/// The reduced whole-photo graph renders half the fitted edge, so its mask
/// atlas can be small.
fn interactive_mask_edge() -> u32 {
    if PlatformCaps::CURRENT.constrained_memory() {
        384
    } else {
        512
    }
}

/// Whether the fitted preview is too slow to follow a drag. Between the two
/// thresholds the last answer stands; before there are timings, a device with
/// little memory is assumed to be slow.
fn gpu_is_slow(
    mode: InteractiveMode,
    median_ms: Option<f32>,
    was_slow: bool,
    constrained_memory: bool,
) -> bool {
    match mode {
        InteractiveMode::Always => true,
        InteractiveMode::Off => false,
        InteractiveMode::Auto => match median_ms {
            Some(ms) if ms > ENGAGE_ABOVE_MS => true,
            Some(ms) if ms < RELEASE_BELOW_MS => false,
            Some(_) => was_slow,
            None => constrained_memory,
        },
    }
}

/// Which preview the canvas shows while a control is dragged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DragPhase {
    /// The fitted preview follows every edit.
    #[default]
    Idle,
    /// A control is dragged: the reduced preview follows it and the fitted
    /// one waits.
    Dragging { last_edit: Instant },
    /// The drag ended or paused: the fitted preview catches up while the
    /// reduced one stays on screen.
    Settling { since: Instant },
}

/// What decides the next phase.
#[derive(Clone, Copy, Debug)]
struct DragInput {
    /// The reduced preview may stand in for the fitted one right now.
    eligible: bool,
    /// A widget is being dragged.
    dragging_control: bool,
    /// An edit was queued since the last frame.
    edit: bool,
    /// The sharp preview (the fitted one, or the crop when zoomed in) shows
    /// every edit and the GPU has finished it.
    sharp_current: bool,
    now: Instant,
}

impl DragPhase {
    fn next(self, input: DragInput) -> Self {
        let now = input.now;
        if !input.eligible {
            return Self::Idle;
        }
        match self {
            Self::Idle => {
                if input.dragging_control && input.edit {
                    Self::Dragging { last_edit: now }
                } else {
                    self
                }
            }
            Self::Dragging { last_edit } => {
                if input.dragging_control && input.edit {
                    Self::Dragging { last_edit: now }
                } else if !input.dragging_control
                    || now.saturating_duration_since(last_edit) >= HOLD_STILL
                {
                    Self::Settling { since: now }
                } else {
                    self
                }
            }
            Self::Settling { since } => {
                if input.dragging_control && input.edit {
                    Self::Dragging { last_edit: now }
                } else if input.sharp_current
                    && now.saturating_duration_since(since) >= SETTLE_DELAY
                {
                    Self::Idle
                } else {
                    self
                }
            }
        }
    }

    fn is_dragging(self) -> bool {
        matches!(self, Self::Dragging { .. })
    }

    /// Whether the reduced preview still owns the edits at `now`: while
    /// dragging, and for `CATCH_UP_DELAY` after.
    fn follows(self, now: Instant) -> bool {
        match self {
            Self::Idle => false,
            Self::Dragging { .. } => true,
            Self::Settling { since } => now.saturating_duration_since(since) < CATCH_UP_DELAY,
        }
    }
}

/// What a reduced graph shows.
enum Reduced {
    /// The whole photo, in the fitted preview's place.
    Whole {
        pipeline: PreviewPipeline,
        raw: Arc<LoadedRaw>,
    },
    /// The part of the photo the sharp crop covers, in its place. It has the
    /// crop's geometry, so the canvas draws it with the crop's coordinates.
    Crop(PreviewDetail),
}

/// The reduced graph.
struct InteractivePreview {
    reduced: Reduced,
    /// What it was reduced from, the fitted proxy or the sharp crop's raw. It
    /// is dropped once that changes.
    source: Arc<LoadedRaw>,
    /// It has rendered since it was built, so its texture holds a picture.
    rendered: bool,
}

impl InteractivePreview {
    fn pipeline(&self) -> &PreviewPipeline {
        match &self.reduced {
            Reduced::Whole { pipeline, .. } => pipeline,
            Reduced::Crop(detail) => &detail.pipeline,
        }
    }
}

/// A reduction of the current sharp source (the fitted proxy, or the zoomed
/// crop's raw) being prepared.
struct WarmUp {
    source: Arc<LoadedRaw>,
    receiver: mpsc::Receiver<Arc<LoadedRaw>>,
}

#[derive(Default)]
pub(crate) struct InteractiveState {
    graph: Option<InteractivePreview>,
    warm: Option<WarmUp>,
    /// The proxy a reduced graph could not be built for, which is not tried
    /// again.
    failed_for: Option<Arc<LoadedRaw>>,
    phase: DragPhase,
    /// The reduced preview owns the edits (`DragPhase::follows`): the sharp
    /// preview keeps what it owes until that ends.
    following: bool,
    /// The fitted preview is too slow to follow a drag (`gpu_is_slow`).
    slow: bool,
    /// The weakest stage an edit queued since the last frame needs.
    edit: Option<ProcessingStage>,
    /// What the reduced graph still has to render.
    pending: Option<ProcessingStage>,
    /// The reduced graph shows every edit made so far.
    synced: bool,
    /// The masks have to be uploaded to it before it renders.
    masks_pending: bool,
    /// The control drag being timed, for the diagnostics line when it ends.
    drag: Option<DragLog>,
}

struct DragLog {
    started: Instant,
    edits: u32,
    /// The reduced preview followed some of it.
    reduced_used: bool,
    /// How many batches of each kind had finished when it started.
    batches_before: [u64; 2],
}

impl InteractiveState {
    /// An edit was queued for the fitted preview.
    pub(crate) fn note_edit(&mut self, stage: ProcessingStage) {
        self.edit = Some(self.edit.map_or(stage, |existing| existing.min(stage)));
    }

    pub(in crate::app) fn pending_stage(&self) -> Option<ProcessingStage> {
        self.pending
    }

    /// A state in which `reduced` has rendered and follows a drag.
    #[cfg(test)]
    pub(in crate::app) fn showing_crop(reduced: PreviewDetail, source: Arc<LoadedRaw>) -> Self {
        Self {
            graph: Some(InteractivePreview {
                reduced: Reduced::Crop(reduced),
                source,
                rendered: true,
            }),
            phase: DragPhase::Dragging {
                last_edit: Instant::now(),
            },
            ..Self::default()
        }
    }

    /// The masks have to be uploaded to the reduced graph before it renders.
    pub(in crate::app) fn masks_pending(&self) -> bool {
        self.masks_pending
    }

    fn visible(&self) -> bool {
        self.phase != DragPhase::Idle && self.graph.as_ref().is_some_and(|graph| graph.rendered)
    }

    /// The texture to show instead of the fitted one.
    pub(crate) fn texture(&self) -> Option<egui::TextureId> {
        match self.graph.as_ref()? {
            InteractivePreview {
                reduced: Reduced::Whole { pipeline, .. },
                ..
            } if self.visible() => Some(pipeline.texture()),
            _ => None,
        }
    }

    /// The texture to show instead of the sharp crop's. It has the crop's
    /// coordinates (`PreviewDetailImage`).
    pub(crate) fn crop_texture(&self) -> Option<egui::TextureId> {
        match self.graph.as_ref()? {
            InteractivePreview {
                reduced: Reduced::Crop(detail),
                ..
            } if self.visible() => Some(detail.pipeline.texture()),
            _ => None,
        }
    }

    /// Moves to `next`, keeping what the reduced graph owes in step. `edit` is
    /// the weakest stage queued this frame, and `following` whether the
    /// reduced preview owns the edits (`DragPhase::follows`).
    fn enter(&mut self, next: DragPhase, following: bool, edit: Option<ProcessingStage>) {
        let entering = next.is_dragging() && !self.phase.is_dragging();
        self.following = following;
        if following {
            if !self.synced && (entering || edit.is_some()) {
                // Everything is rendered once, with the masks uploaded, so the
                // graph shows the edit as it is and not as it was when it
                // last followed a drag.
                self.pending = Some(ProcessingStage::Raw);
                self.masks_pending = true;
            } else if let Some(stage) = edit {
                self.pending = Some(self.pending.map_or(stage, |pending| pending.min(stage)));
            }
        } else {
            // An edit that arrives while the fitted preview owns the edits, or
            // work left over, means the reduced graph has fallen behind.
            if edit.is_some() || self.pending.is_some() {
                self.synced = false;
            }
            self.pending = None;
        }
        self.phase = next;
    }

    /// Drops the graph and everything prepared for it.
    pub(in crate::app) fn reset(&mut self) {
        self.graph = None;
        self.warm = None;
        self.phase = DragPhase::Idle;
        self.following = false;
        self.pending = None;
        self.synced = false;
        self.masks_pending = false;
    }
}

/// The preview whose picture is on the canvas: the reduced one while it
/// stands in, otherwise the fitted one. Takes the fields so a caller can hold
/// other parts of `PreviewState` mutably meanwhile.
pub(crate) fn shown_pipeline<'a>(
    fitted: Option<&'a PreviewPipeline>,
    interactive: &'a InteractiveState,
) -> Option<&'a PreviewPipeline> {
    match interactive.graph.as_ref() {
        Some(InteractivePreview {
            reduced: Reduced::Whole { pipeline, .. },
            ..
        }) if interactive.visible() => Some(pipeline),
        _ => fitted,
    }
}

impl CalibRawApp {
    /// Whether the reduced preview may stand in for the sharp one, whatever
    /// the speed of the GPU: a plain photo, nothing rebuilding, and zoomed in
    /// a crop that covers the view.
    fn interactive_usable(&self) -> bool {
        interactive_mode() != InteractiveMode::Off
            && self.preview.gpu_pipeline.is_some()
            && !self.preview.original_requested
            && !self.preview_is_preparing()
            && !self.lens_correction_busy()
            // An export tile needs the memory the reduced graph would hold.
            && self.export.task.is_none()
            // The reduced graph has no AI source.
            && !self.develop.target_exposure.ai_denoise_enabled
            // The clipping overlay is composed from the sharp output.
            && !self.preview.clipping.shadows
            && !self.preview.clipping.highlights
            && !self.fitted_mask_capacity_stale()
            // A mask being shaped has its own path; the reduced graph's masks
            // are uploaded as a whole when a drag starts.
            && self.masks.interaction_dirty_layer.is_none()
            && !self.masks.dirty_layers.iter().any(|dirty| *dirty)
            && (!self.preview.zoomed_in()
                // The crop's own masks, which stay marked while it is absent.
                || (!self.masks.detail_dirty_layers.iter().any(|dirty| *dirty)
                    && self.sharp_crop_covers_view()))
    }

    /// Whether the sharp crop is there to be reduced: it belongs to this view
    /// and is not being replaced. A drag already followed keeps it, whatever a
    /// control changed about what the crop would need (the border its filters
    /// reach); it is refreshed once the drag ends.
    fn sharp_crop_covers_view(&self) -> bool {
        self.preview.detail_rebuild.is_none()
            && if self.preview.interactive.phase == DragPhase::Idle {
                self.preview_detail_is_current()
            } else {
                self.preview
                    .detail
                    .as_ref()
                    .is_some_and(|detail| detail.revision == self.preview.revision)
            }
    }

    /// Whether the sharp preview has every edit queued so far rendered. It
    /// is the crop when zoomed in, as the fitted preview is not on the canvas.
    fn sharp_preview_rendered(&self) -> bool {
        if self.preview.zoomed_in() {
            self.preview.detail_pending_stage.is_none() && self.preview_detail_is_current()
        } else {
            self.preview.pending_stage.is_none()
        }
    }

    /// Whether the reduced preview owns the edits: a control is being dragged,
    /// or was a moment ago. The sharp preview waits meanwhile.
    pub(in crate::app) fn interactive_following(&self) -> bool {
        self.preview.interactive.following
    }

    /// Keeps the reduced graph (builds it when the editor is quiet, drops it
    /// when it cannot be used) and advances the drag phase.
    pub(in crate::app) fn advance_interactive_preview(&mut self, frame: &eframe::Frame) {
        let now = Instant::now();
        let edit = self.preview.interactive.edit.take();
        let usable = self.interactive_usable();
        let sharp_batch = if self.preview.zoomed_in() {
            Batch::Detail
        } else {
            Batch::Fitted
        };
        let median = self.preview.dispatch_gate.median_latency_ms(sharp_batch);
        let slow = gpu_is_slow(
            interactive_mode(),
            median,
            self.preview.interactive.slow,
            self.caps().constrained_memory(),
        );
        self.preview.interactive.slow = slow;
        self.maintain_interactive_graph(frame, usable && slow);

        let eligible = usable && slow && self.preview.interactive.graph.is_some();
        // The GPU has finished what it was given, too, so the sharp picture
        // that replaces the reduced one is on the screen.
        let sharp_current = self.sharp_preview_rendered()
            && frame.wgpu_render_state().is_none_or(|render_state| {
                self.preview.dispatch_gate.poll_open(&render_state.device)
            });
        let before = self.preview.interactive.phase;
        let dragging_control = super::interaction::control_dragged(&self.egui_ctx);
        let next = before.next(DragInput {
            eligible,
            dragging_control,
            edit: edit.is_some(),
            sharp_current,
            now,
        });
        self.preview
            .interactive
            .enter(next, next.follows(now), edit);
        self.log_drag(next, edit.is_some(), dragging_control, now);
        match next {
            DragPhase::Idle => {}
            DragPhase::Dragging { .. } => self.egui_ctx.request_repaint_after(HOLD_STILL),
            DragPhase::Settling { since } => {
                // The frame that ends the wait starts the sharp preview.
                let wait = CATCH_UP_DELAY.saturating_sub(now.saturating_duration_since(since));
                self.egui_ctx.request_repaint_after(if wait.is_zero() {
                    SETTLE_DELAY
                } else {
                    wait
                });
            }
        }
    }

    /// One line per control drag for the diagnostics, with the rates and
    /// timings that show what the reduced preview gains (or would).
    fn log_drag(&mut self, next: DragPhase, edit: bool, dragging_control: bool, now: Instant) {
        let zoomed = self.preview.zoomed_in();
        let gate = &self.preview.dispatch_gate;
        let state = &mut self.preview.interactive;
        let sharp_batch = if zoomed { Batch::Detail } else { Batch::Fitted };
        let batches = [
            gate.completed(Batch::Interactive),
            gate.completed(sharp_batch),
        ];
        if state.drag.is_none() && dragging_control && edit {
            state.drag = Some(DragLog {
                started: now,
                edits: 0,
                reduced_used: false,
                batches_before: batches,
            });
        }
        let Some(drag) = state.drag.as_mut() else {
            return;
        };
        drag.edits += u32::from(edit);
        drag.reduced_used |= next.is_dragging();
        if dragging_control {
            return;
        }
        let seconds = now.saturating_duration_since(drag.started).as_secs_f64();
        let reduced = batches[0] - drag.batches_before[0];
        let sharp = batches[1] - drag.batches_before[1];
        calibraw_core::diagnostics::record(format!(
            "Control drag{}: {} edits over {seconds:.2}s, reduced preview {} ({:?} mode, GPU judged {}), {reduced} reduced and {sharp} {} GPU batches ({:.1} a second); median batch reduced {:?} ms, {} {:?} ms",
            if zoomed { " (zoomed in)" } else { "" },
            drag.edits,
            if drag.reduced_used { "used" } else { "not used" },
            interactive_mode(),
            if state.slow { "slow" } else { "fast" },
            if zoomed { "crop" } else { "fitted" },
            (reduced + sharp) as f64 / seconds.max(0.001),
            gate.median_latency_ms(Batch::Interactive).map(f32::round),
            if zoomed { "crop" } else { "fitted" },
            gate.median_latency_ms(sharp_batch).map(f32::round),
        ));
        state.drag = None;
    }

    /// What the reduced graph would be made from: the sharp crop's raw when
    /// zoomed in, otherwise the fitted proxy. `None` where there is nothing to
    /// reduce, or it is small enough already.
    fn reduction(&self) -> Option<Reduction> {
        let source = if self.preview.zoomed_in() {
            &self
                .preview
                .detail
                .as_ref()
                .filter(|detail| detail.revision == self.preview.revision)?
                .raw
        } else {
            self.develop.preview_raw.as_ref()?
        };
        Some(Reduction {
            edge: interactive_edge(source.width.max(source.height))?,
            source: Arc::clone(source),
        })
    }

    /// Builds the reduced graph for the current sharp preview when it is
    /// wanted and the editor is quiet, and drops it when it is not wanted or
    /// out of date.
    fn maintain_interactive_graph(&mut self, frame: &eframe::Frame, wanted: bool) {
        let reduction = self.reduction().filter(|_| wanted);
        let needed_layers = self.preview_mask_stack().masks.len().max(1);
        let state = &mut self.preview.interactive;
        let Some(reduction) = reduction else {
            if state.graph.is_some() || state.warm.is_some() {
                state.reset();
            }
            return;
        };
        let out_of_date = state.graph.as_ref().is_some_and(|graph| {
            !Arc::ptr_eq(&graph.source, &reduction.source)
                || graph.pipeline().gpu().mask_layer_capacity() < needed_layers
        });
        if out_of_date {
            state.reset();
        }
        if state
            .warm
            .as_ref()
            .is_some_and(|warm| !Arc::ptr_eq(&warm.source, &reduction.source))
        {
            state.warm = None;
        }
        if state.graph.is_some() {
            return;
        }
        if let Some(warm) = state.warm.as_ref() {
            match warm.receiver.try_recv() {
                Ok(reduced) => {
                    state.warm = None;
                    self.build_interactive_graph(frame, reduction.source, reduced);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.warm = None;
                    state.failed_for = Some(reduction.source);
                    self.report_error(
                        ErrorKind::Preview,
                        "The reduced preview worker stopped unexpectedly.".to_owned(),
                    );
                }
            }
            return;
        }
        if state
            .failed_for
            .as_ref()
            .is_some_and(|failed| Arc::ptr_eq(failed, &reduction.source))
        {
            return;
        }
        if !self.interactive_quiet() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = self.egui_ctx.clone();
        let Reduction { source, edge } = reduction;
        let worker_source = Arc::clone(&source);
        let spawn = std::thread::Builder::new()
            .name("calibraw-interactive-preview".to_owned())
            .spawn(move || {
                let reduced = Arc::new(build_proxy(&worker_source, ProxySpec { max_edge: edge }));
                let _ = sender.send(reduced);
                context.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.preview.interactive.warm = Some(WarmUp { source, receiver });
            }
            Err(error) => {
                self.preview.interactive.failed_for = Some(source);
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not start the reduced preview: {error}"),
                );
            }
        }
    }

    /// Whether nothing is being edited or rendered, so preparing the reduced
    /// graph takes time from no one.
    fn interactive_quiet(&self) -> bool {
        !self.preview.processing_pending()
            && !super::interaction::pointer_engaged(&self.egui_ctx)
            && (!self.preview.zoomed_in() || self.preview_detail_is_current())
    }

    /// Builds the reduced graph of `source` from `reduced`, the worker's
    /// reduction of it, and keeps a graph that does not fit as a failure not
    /// to be tried again.
    fn build_interactive_graph(
        &mut self,
        frame: &eframe::Frame,
        source: Arc<LoadedRaw>,
        reduced: Arc<LoadedRaw>,
    ) {
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        let built = if self.preview.zoomed_in() {
            self.preview
                .detail
                .as_ref()
                .filter(|detail| Arc::ptr_eq(&detail.raw, &source))
                .map(|sharp| {
                    self.build_interactive_crop(
                        &render_state.device,
                        &render_state.queue,
                        |pipeline| self.present_pipeline(pipeline, render_state),
                        sharp,
                        &reduced,
                    )
                    .map(Reduced::Crop)
                })
        } else {
            Some(self.build_whole_graph(render_state, &reduced))
        };
        // The sharp crop was replaced while the worker ran.
        let Some(built) = built else {
            return;
        };
        match built {
            Ok(reduced_graph) => {
                let graph = InteractivePreview {
                    reduced: reduced_graph,
                    source,
                    rendered: false,
                };
                calibraw_core::diagnostics::record(format!(
                    "Interactive preview graph built{}: {}x{} ({:.1} MiB)",
                    if matches!(graph.reduced, Reduced::Crop(_)) {
                        " for the zoomed crop"
                    } else {
                        ""
                    },
                    reduced.width,
                    reduced.height,
                    graph.pipeline().gpu().resident_bytes() as f64 / (1024.0 * 1024.0)
                ));
                self.preview.interactive.graph = Some(graph);
                self.preview.interactive.synced = false;
            }
            Err(error) => {
                self.preview.interactive.failed_for = Some(source);
                // Memory the sharp previews and an export tile leave is not
                // enough: expected on a small device, and nothing is lost.
                if error.to_string().contains("GPU pipelines already reserve") {
                    calibraw_core::diagnostics::record(format!(
                        "Interactive preview graph skipped: {error:#}"
                    ));
                } else {
                    self.report_error(
                        ErrorKind::Preview,
                        format!("Could not build the reduced preview: {error:#}"),
                    );
                }
            }
        }
    }

    /// The reduced graph of the whole photo, from the reduced fitted proxy.
    fn build_whole_graph(
        &self,
        render_state: &eframe::egui_wgpu::RenderState,
        reduced: &Arc<LoadedRaw>,
    ) -> anyhow::Result<Reduced> {
        let template = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template)
            .ok_or_else(|| anyhow::anyhow!("the fitted preview is gone"))?;
        let masks = self.preview_mask_stack();
        let params = GpuParams::new(&self.develop.target_exposure, &masks, reduced)
            .with_vignette_geometry(self.develop.geometry);
        let options = fitted_pipeline_options(masks.masks.len())
            .mask_atlas_edge(interactive_mask_edge())
            .programs(&template);
        let pipeline = RawGpuPipeline::new(
            &render_state.device,
            &render_state.queue,
            reduced,
            &params,
            options,
        )?;
        Ok(Reduced::Whole {
            pipeline: self.present_pipeline(pipeline, render_state),
            raw: Arc::clone(reduced),
        })
    }

    /// Renders the next pending stage of the reduced graph.
    pub(in crate::app) fn advance_interactive_stage(&mut self, frame: &eframe::Frame) {
        let Some(stage) = self.preview.interactive.pending else {
            return;
        };
        let preview_masks = self.preview_mask_stack();
        let Some(full_raw) = self.develop.loaded_raw.clone() else {
            self.preview.interactive.pending = None;
            return;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        let state = &self.preview.interactive;
        let Some(graph) = state.graph.as_ref() else {
            self.preview.interactive.pending = None;
            return;
        };
        if stage == ProcessingStage::Raw
            && full_raw.uses_opposed_chroma(&self.develop.target_exposure)
        {
            full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }
        let result = match &graph.reduced {
            Reduced::Whole { pipeline, raw } => self.render_whole_stage(
                render_state,
                pipeline,
                raw,
                stage,
                &preview_masks,
                &full_raw,
            ),
            Reduced::Crop(detail) => self.render_interactive_crop_stage(
                &render_state.queue,
                &render_state.device,
                detail,
                stage,
                &preview_masks,
                &full_raw,
            ),
        };
        let state = &mut self.preview.interactive;
        if let Err(error) = result {
            if let Some(source) = state.graph.as_ref().map(|graph| Arc::clone(&graph.source)) {
                state.failed_for = Some(source);
            }
            state.reset();
            self.report_error(
                ErrorKind::Preview,
                format!("Could not render the reduced preview: {error:#}"),
            );
            return;
        }
        state.masks_pending = false;
        state.pending = match stage {
            ProcessingStage::Raw => Some(ProcessingStage::Tone),
            ProcessingStage::Tone => Some(ProcessingStage::Output),
            ProcessingStage::Output => None,
        };
        if state.pending.is_none() {
            state.synced = true;
            if let Some(graph) = state.graph.as_mut() {
                graph.rendered = true;
            }
        }
        self.egui_ctx.request_repaint();
    }

    /// Renders `stage` of the reduced graph of the whole photo. With
    /// `masks_pending` the masks go up first.
    fn render_whole_stage(
        &self,
        render_state: &eframe::egui_wgpu::RenderState,
        pipeline: &PreviewPipeline,
        raw: &LoadedRaw,
        stage: ProcessingStage,
        masks: &MaskStack,
        full_raw: &LoadedRaw,
    ) -> anyhow::Result<()> {
        if self.preview.interactive.masks_pending() {
            Self::upload_preview_masks(pipeline.gpu(), &render_state.queue, masks, raw)
                .map_err(anyhow::Error::msg)?;
        }
        let params = GpuParams::new(&self.develop.target_exposure, masks, raw)
            .with_vignette_geometry(self.develop.geometry);
        pipeline.gpu().dispatch_stage_with_remove(
            &render_state.queue,
            &render_state.device,
            &params,
            stage,
            RemoveSceneContext::full_frame(
                &self.inpaint.edits,
                full_raw,
                &self.develop.target_exposure,
            ),
        )
    }
}

/// What the reduced graph is made from, and how large it is.
struct Reduction {
    source: Arc<LoadedRaw>,
    /// The long edge of the reduction.
    edge: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(now: Instant) -> DragInput {
        DragInput {
            eligible: true,
            dragging_control: false,
            edit: false,
            sharp_current: false,
            now,
        }
    }

    #[test]
    fn a_dragged_control_that_edits_starts_the_reduced_preview() {
        let now = Instant::now();
        let dragging = DragInput {
            dragging_control: true,
            edit: true,
            ..input(now)
        };
        assert_eq!(
            DragPhase::Idle.next(dragging),
            DragPhase::Dragging { last_edit: now }
        );
        // A click, an edit by keyboard or a drag that edits nothing does not.
        for ignored in [
            DragInput {
                dragging_control: false,
                edit: true,
                ..input(now)
            },
            DragInput {
                dragging_control: true,
                edit: false,
                ..input(now)
            },
        ] {
            assert_eq!(DragPhase::Idle.next(ignored), DragPhase::Idle);
        }
    }

    #[test]
    fn a_drag_settles_when_it_ends_or_holds_still_and_resumes_on_the_next_edit() {
        let start = Instant::now();
        let dragging = DragPhase::Dragging { last_edit: start };
        let later = start + Duration::from_millis(100);
        // The release.
        assert_eq!(
            dragging.next(input(later)),
            DragPhase::Settling { since: later }
        );
        // Held still past the limit with the pointer down.
        let held = start + HOLD_STILL;
        assert_eq!(
            dragging.next(DragInput {
                dragging_control: true,
                ..input(held)
            }),
            DragPhase::Settling { since: held }
        );
        // Still moving.
        let moving = DragInput {
            dragging_control: true,
            edit: true,
            ..input(held)
        };
        assert_eq!(
            dragging.next(moving),
            DragPhase::Dragging { last_edit: held }
        );
        assert_eq!(
            DragPhase::Settling { since: start }.next(moving),
            DragPhase::Dragging { last_edit: held }
        );
    }

    #[test]
    fn the_fitted_preview_replaces_the_reduced_one_once_current_after_the_delay() {
        let start = Instant::now();
        let settling = DragPhase::Settling { since: start };
        let current = |at| DragInput {
            sharp_current: true,
            ..input(at)
        };
        assert_eq!(settling.next(current(start)), settling);
        assert_eq!(
            settling.next(DragInput {
                sharp_current: false,
                ..input(start + SETTLE_DELAY)
            }),
            settling
        );
        assert_eq!(
            settling.next(current(start + SETTLE_DELAY)),
            DragPhase::Idle
        );
    }

    #[test]
    fn losing_eligibility_ends_every_phase() {
        let now = Instant::now();
        let ineligible = DragInput {
            eligible: false,
            dragging_control: true,
            edit: true,
            ..input(now)
        };
        for phase in [
            DragPhase::Idle,
            DragPhase::Dragging { last_edit: now },
            DragPhase::Settling { since: now },
        ] {
            assert_eq!(phase.next(ineligible), DragPhase::Idle);
        }
    }

    #[test]
    fn the_reduced_graph_renders_everything_once_and_then_only_the_edits() {
        let now = Instant::now();
        let dragging = DragPhase::Dragging { last_edit: now };
        let settling = DragPhase::Settling { since: now };
        let mut state = InteractiveState::default();

        // The first drag: nothing is known to be in step, so all stages run
        // and the masks go up, whatever the edit was.
        state.enter(dragging, true, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Raw));
        assert!(state.masks_pending);
        assert!(state.following);

        // Later edits of the drag only add the stage they need.
        state.pending = None;
        state.masks_pending = false;
        state.synced = true;
        state.enter(dragging, true, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Output));
        state.enter(dragging, true, Some(ProcessingStage::Raw));
        assert_eq!(state.pending, Some(ProcessingStage::Raw));

        // A pause, or a release, keeps the reduced graph in charge for a
        // moment: it still renders what it owes, and an edit by other means
        // joins that.
        state.enter(settling, true, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Raw));
        assert!(state.synced && state.following);
        state.pending = None;
        state.enter(settling, true, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Output));

        // The drag resumes without a full render.
        state.pending = None;
        state.enter(dragging, true, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Output));
        assert!(!state.masks_pending);

        // Once the sharp preview takes over, what the reduced graph still owed
        // is dropped, and it is behind.
        state.enter(settling, false, None);
        assert_eq!(state.pending, None);
        assert!(!state.synced && !state.following);

        // An edit while the fitted preview owns them puts it behind, too.
        state.synced = true;
        state.enter(DragPhase::Idle, false, Some(ProcessingStage::Output));
        assert!(!state.synced);
        state.enter(dragging, true, None);
        assert_eq!(state.pending, Some(ProcessingStage::Raw));
        assert!(state.masks_pending);
    }

    #[test]
    fn the_sharp_previews_wait_while_the_reduced_one_owns_the_edits() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let frame = eframe::Frame::_new_kittest();
        assert!(!app.defer_background_preview_processing());

        // A crop that is ready is not installed meanwhile: installing renders
        // the whole crop, which the drag would have to wait behind.
        let (sender, receiver) = mpsc::channel();
        sender.send(PreviewDetailRebuildEvent::Cancelled).unwrap();
        app.preview.detail_rebuild = Some(DetailRebuild::new(receiver, Arc::default()));
        app.preview.interactive.following = true;
        assert!(app.defer_background_preview_processing());
        app.poll_preview_detail_rebuild_worker(&frame);
        assert!(app.preview.detail_rebuild.is_some());

        app.preview.interactive.following = false;
        assert!(!app.defer_background_preview_processing());
        app.poll_preview_detail_rebuild_worker(&frame);
        assert!(app.preview.detail_rebuild.is_none());
    }

    #[test]
    fn the_reduced_preview_owns_the_edits_for_a_moment_after_the_drag() {
        let start = Instant::now();
        assert!(!DragPhase::Idle.follows(start));
        assert!(DragPhase::Dragging { last_edit: start }.follows(start + Duration::from_secs(9)));
        let settling = DragPhase::Settling { since: start };
        assert!(settling.follows(start));
        assert!(settling.follows(start + CATCH_UP_DELAY - Duration::from_millis(1)));
        assert!(!settling.follows(start + CATCH_UP_DELAY));
    }

    #[test]
    fn a_followed_zoomed_drag_keeps_the_navigation_proxy_for_tone_changes_only() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let dragging = DragPhase::Dragging {
            last_edit: Instant::now(),
        };
        app.preview.interactive.phase = dragging;
        app.preview.interactive.following = true;
        app.preview.zoom = 2.0;
        // The reduced crop takes its tone statistics from the navigation
        // proxy while the fitted preview waits, so a change to them is not
        // waited for; an Output change does not touch them.
        for (pending, waits) in [
            (None, true),
            (Some(ProcessingStage::Output), true),
            (Some(ProcessingStage::Tone), false),
            (Some(ProcessingStage::Raw), false),
        ] {
            app.preview.navigation_pending_stage = pending;
            assert_eq!(app.defer_navigation_preview(), waits, "{pending:?}");
        }
        // The fitted preview waits for every kind of change.
        app.preview.pending_stage = Some(ProcessingStage::Raw);
        assert!(app.defer_background_preview_processing());
        // Unzoomed there is no proxy to keep.
        app.preview.zoom = 1.0;
        app.preview.navigation_pending_stage = Some(ProcessingStage::Raw);
        assert!(app.defer_navigation_preview());
        app.preview.interactive.phase = DragPhase::Idle;
        app.preview.interactive.following = false;
        assert!(!app.defer_navigation_preview());
    }

    #[test]
    fn only_a_rendered_graph_is_shown_and_only_while_a_phase_is_active() {
        let mut state = InteractiveState::default();
        assert!(!state.visible());
        state.phase = DragPhase::Dragging {
            last_edit: Instant::now(),
        };
        // No graph, no picture.
        assert!(!state.visible());
        assert_eq!(state.texture(), None);
    }

    #[test]
    fn the_reduced_graph_is_half_the_fitted_proxy_and_never_for_a_small_one() {
        assert_eq!(interactive_edge(1_691), Some(845));
        assert_eq!(interactive_edge(MIN_SOURCE_EDGE), Some(640));
        assert_eq!(interactive_edge(MIN_SOURCE_EDGE - 1), None);
        assert_eq!(interactive_edge(640), None);
    }

    #[test]
    fn a_slow_gpu_is_told_by_the_timings_with_a_gap_against_flapping() {
        use InteractiveMode::{Always, Auto, Off};
        let slow =
            |mode, median, was_slow, constrained| gpu_is_slow(mode, median, was_slow, constrained);
        assert!(slow(Auto, Some(120.0), false, false));
        assert!(!slow(Auto, Some(12.0), true, true));
        // Between the thresholds the last answer stands.
        assert!(slow(Auto, Some(35.0), true, false));
        assert!(!slow(Auto, Some(35.0), false, true));
        // Without timings a small device is assumed slow, a desktop fast.
        assert!(slow(Auto, None, false, true));
        assert!(!slow(Auto, None, false, false));
        assert!(slow(Always, Some(5.0), false, false));
        assert!(!slow(Off, Some(500.0), true, true));
    }

    /// What replaces the reduced picture on release must not jump: rendered
    /// from a reduction of the same photo, it matches the fitted picture
    /// averaged down to its size.
    #[test]
    fn the_reduced_graph_shows_the_same_picture_at_half_the_size() {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "reduced preview test",
            adapter: calibraw_gpu::HeadlessAdapter::Software,
            texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
        });
        let (device, queue) = match gpu {
            Ok(gpu) => (gpu.device, gpu.queue),
            Err(calibraw_gpu::HeadlessDeviceError::NoAdapter(_)) => {
                eprintln!("reduced preview test skipped: no headless GPU adapter");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let (width, height) = (192u32, 128u32);
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
                pixels.extend([0.02 + 0.6 * u, 0.02 + 0.4 * v, 0.05 + 0.3 * u * v]);
            }
        }
        let raw = LoadedRaw::from_scene_linear_rec2020(width, height, pixels).unwrap();
        let reduced = build_proxy(&raw, ProxySpec { max_edge: 96 });
        assert_eq!((reduced.width, reduced.height), (96, 64));
        let exposure = ExposureParams {
            exposure: 0.7,
            ..ExposureParams::default()
        };
        let masks = MaskStack::default();
        let render = |raw: &LoadedRaw| {
            let params = GpuParams::new(&exposure, &masks, raw);
            let pipeline =
                RawGpuPipeline::new(&device, &queue, raw, &params, fitted_pipeline_options(0))
                    .unwrap();
            pipeline.recompute(&queue, &device, &params);
            pipeline
                .read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)
                .unwrap()
        };
        let fitted = crate::pipeline::downscale_display_rgba8(
            &render(&raw),
            [width, height],
            [reduced.width, reduced.height],
        );
        let reduced_picture = render(&reduced);
        let (mut total, mut count) = (0u64, 0u64);
        for (a, b) in fitted.chunks_exact(4).zip(reduced_picture.chunks_exact(4)) {
            for channel in 0..3 {
                total += u64::from(a[channel].abs_diff(b[channel]));
                count += 1;
            }
        }
        let mean = total as f64 / count as f64;
        eprintln!("reduced against fitted: mean error {mean:.2} of 255");
        assert!(mean < 1.5, "mean error {mean:.2} of 255");
    }
}
