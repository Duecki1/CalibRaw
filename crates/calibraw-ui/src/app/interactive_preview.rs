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
use crate::i18n::{tr, tr_fmt};

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
                        tr("The reduced preview worker stopped unexpectedly.").to_owned(),
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
                    tr_fmt(
                        "Could not start the reduced preview: {error}",
                        &[("error", &error)],
                    ),
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
                        tr_fmt(
                            "Could not build the reduced preview: {error}",
                            &[("error", &format!("{error:#}"))],
                        ),
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
            .ok_or_else(|| anyhow::anyhow!(tr("the fitted preview is gone")))?;
        let masks = self.preview_mask_stack();
        let params = GpuParams::new(&self.develop.target_exposure, &masks, reduced)
            .with_vignette_geometry(self.develop.geometry)
            .with_look_lut(self.develop.target_look.as_ref());
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
                tr_fmt(
                    "Could not render the reduced preview: {error}",
                    &[("error", &format!("{error:#}"))],
                ),
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
            .with_vignette_geometry(self.develop.geometry)
            .with_look_lut(self.develop.target_look.as_ref());
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
