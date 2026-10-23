//! The reduced preview shown while a control is dragged.
//!
//! On a GPU that needs more than a few frames for the fitted preview, a slider
//! drag falls far behind the pointer. While a control is dragged, a second
//! graph of the whole photo at half the fitted edge (a quarter of the pixels
//! and of the GPU time) follows it, and the fitted preview waits. When the
//! drag ends, the fitted preview renders everything it missed
//! (`queue_preview_processing` keeps merging the stage it owes while it waits)
//! and replaces the reduced one once it is done.
//!
//! Which GPUs need it is measured, not assumed (`gpu_is_slow`): the dispatch
//! gate times the fitted batches. Zoomed in, the crop follows the drag at the
//! screen's density as before (`defer_background_preview_processing`).
//!
//! The reduced graph is built from the fitted proxy on a worker once the
//! editor is quiet, so a drag finds it ready; one that starts earlier just
//! uses the fitted preview. It exists only while the photo is fitted, so it
//! never competes with the zoomed crop for memory.

use super::preview_dispatch_gate::Batch;
use super::*;

/// The smallest fitted edge that gets a reduced graph; half of it must still
/// reach the smallest proxy (`MIN_PROXY_EDGE`).
const MIN_FITTED_EDGE: u32 = 1280;

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

/// The edge of the reduced graph for a fitted proxy of `fitted_edge`, or
/// `None` where the proxy is small enough already.
fn interactive_edge(fitted_edge: u32) -> Option<u32> {
    (fitted_edge >= MIN_FITTED_EDGE).then_some(fitted_edge / 2)
}

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
    /// The fitted preview shows every edit and the GPU has finished it.
    fitted_current: bool,
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
                } else if input.fitted_current
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
}

/// The reduced graph.
pub(crate) struct InteractivePreview {
    pipeline: PreviewPipeline,
    raw: Arc<LoadedRaw>,
    /// The fitted proxy it was reduced from; it is dropped once that changes.
    source: Arc<LoadedRaw>,
    /// It has rendered since it was built, so its texture holds a picture.
    rendered: bool,
}

/// A reduction of the fitted proxy being prepared.
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

    fn visible(&self) -> bool {
        self.phase != DragPhase::Idle && self.graph.as_ref().is_some_and(|graph| graph.rendered)
    }

    /// The texture to show instead of the fitted one.
    pub(crate) fn texture(&self) -> Option<egui::TextureId> {
        if !self.visible() {
            return None;
        }
        self.graph.as_ref().map(|graph| graph.pipeline.texture())
    }

    /// Moves to `next`, keeping what the reduced graph owes in step. `edit` is
    /// the weakest stage queued this frame.
    fn enter(&mut self, next: DragPhase, edit: Option<ProcessingStage>) {
        let entering = next.is_dragging() && !self.phase.is_dragging();
        if next.is_dragging() {
            if entering && !self.synced {
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
    fn reset(&mut self) {
        self.graph = None;
        self.warm = None;
        self.phase = DragPhase::Idle;
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
        Some(graph) if interactive.visible() => Some(&graph.pipeline),
        _ => fitted,
    }
}

impl CalibRawApp {
    /// Whether the reduced preview may stand in for the fitted one, whatever
    /// the speed of the GPU: a plain photo shown whole, nothing rebuilding.
    fn interactive_usable(&self) -> bool {
        interactive_mode() != InteractiveMode::Off
            && self.preview.gpu_pipeline.is_some()
            && !self.preview.zoomed_in()
            && !self.preview.original_requested
            && !self.preview_is_preparing()
            && !self.lens_correction_busy()
            // The reduced graph has no AI source.
            && !self.develop.target_exposure.ai_denoise_enabled
            // The clipping overlay is composed from the fitted output.
            && !self.preview.clipping.shadows
            && !self.preview.clipping.highlights
            && !self.fitted_mask_capacity_stale()
            // A mask being shaped has its own path; the reduced graph's masks
            // are uploaded as a whole when a drag starts.
            && self.masks.interaction_dirty_layer.is_none()
            && !self.masks.dirty_layers.iter().any(|dirty| *dirty)
    }

    /// Whether a control is being dragged and the reduced preview follows it.
    pub(in crate::app) fn interactive_drag_active(&self) -> bool {
        self.preview.interactive.phase.is_dragging()
    }

    /// Keeps the reduced graph (builds it when the editor is quiet, drops it
    /// when it cannot be used) and advances the drag phase.
    pub(in crate::app) fn advance_interactive_preview(&mut self, frame: &eframe::Frame) {
        let now = Instant::now();
        let edit = self.preview.interactive.edit.take();
        let usable = self.interactive_usable();
        let median = self.preview.dispatch_gate.median_latency_ms(Batch::Fitted);
        let slow = gpu_is_slow(
            interactive_mode(),
            median,
            self.preview.interactive.slow,
            self.caps().constrained_memory(),
        );
        self.preview.interactive.slow = slow;
        self.maintain_interactive_graph(frame, usable && slow);

        let state = &self.preview.interactive;
        let eligible = usable && slow && state.graph.is_some();
        let fitted_current = self.preview.pending_stage.is_none()
            && frame.wgpu_render_state().is_none_or(|render_state| {
                self.preview.dispatch_gate.poll_open(&render_state.device)
            });
        let before = state.phase;
        let dragging_control = super::interaction::control_dragged(&self.egui_ctx);
        let next = before.next(DragInput {
            eligible,
            dragging_control,
            edit: edit.is_some(),
            fitted_current,
            now,
        });
        self.preview.interactive.enter(next, edit);
        self.log_drag(next, edit.is_some(), dragging_control, now);
        match next {
            DragPhase::Idle => {}
            DragPhase::Dragging { .. } => self.egui_ctx.request_repaint_after(HOLD_STILL),
            DragPhase::Settling { .. } => self.egui_ctx.request_repaint_after(SETTLE_DELAY),
        }
    }

    /// One line per control drag for the diagnostics, with the rates and
    /// timings that show what the reduced preview gains (or would).
    fn log_drag(&mut self, next: DragPhase, edit: bool, dragging_control: bool, now: Instant) {
        let gate = &self.preview.dispatch_gate;
        let state = &mut self.preview.interactive;
        let batches = [
            gate.completed(Batch::Interactive),
            gate.completed(Batch::Fitted),
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
        let fitted = batches[1] - drag.batches_before[1];
        calibraw_core::diagnostics::record(format!(
            "Control drag: {} edits over {seconds:.2}s, reduced preview {} ({:?} mode, GPU judged {}), {reduced} reduced and {fitted} fitted GPU batches ({:.1} a second); median batch reduced {:?} ms, fitted {:?} ms",
            drag.edits,
            if drag.reduced_used { "used" } else { "not used" },
            interactive_mode(),
            if state.slow { "slow" } else { "fast" },
            (reduced + fitted) as f64 / seconds.max(0.001),
            gate.median_latency_ms(Batch::Interactive).map(f32::round),
            gate.median_latency_ms(Batch::Fitted).map(f32::round),
        ));
        state.drag = None;
    }

    /// Builds the reduced graph for the current fitted proxy when it is wanted
    /// and the editor is quiet, and drops it when it is not wanted or out of
    /// date.
    fn maintain_interactive_graph(&mut self, frame: &eframe::Frame, wanted: bool) {
        let preview_raw = self.develop.preview_raw.clone();
        let is_current = |source: &Arc<LoadedRaw>| {
            preview_raw
                .as_ref()
                .is_some_and(|raw| Arc::ptr_eq(raw, source))
        };
        let needed_layers = self.preview_mask_stack().masks.len().max(1);
        let state = &mut self.preview.interactive;
        let out_of_date = state.graph.as_ref().is_some_and(|graph| {
            !is_current(&graph.source) || graph.pipeline.gpu().mask_layer_capacity() < needed_layers
        });
        if out_of_date || (!wanted && state.graph.is_some()) {
            state.reset();
        }
        if state
            .warm
            .as_ref()
            .is_some_and(|warm| !is_current(&warm.source))
            || !wanted
        {
            state.warm = None;
        }
        let Some(source) = preview_raw else {
            return;
        };
        if !wanted || state.graph.is_some() {
            return;
        }
        if let Some(warm) = state.warm.as_ref() {
            match warm.receiver.try_recv() {
                Ok(reduced) => {
                    state.warm = None;
                    self.build_interactive_graph(frame, source, reduced);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.warm = None;
                    state.failed_for = Some(source);
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
            .is_some_and(|failed| Arc::ptr_eq(failed, &source))
        {
            return;
        }
        let quiet = !self.preview.processing_pending()
            && !super::interaction::pointer_engaged(&self.egui_ctx);
        let Some(edge) = interactive_edge(source.width.max(source.height)).filter(|_| quiet) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let context = self.egui_ctx.clone();
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

    fn build_interactive_graph(
        &mut self,
        frame: &eframe::Frame,
        source: Arc<LoadedRaw>,
        reduced: Arc<LoadedRaw>,
    ) {
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        let Some(template) = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template)
        else {
            return;
        };
        let masks = self.preview_mask_stack();
        let params = GpuParams::new(&self.develop.target_exposure, &masks, &reduced)
            .with_vignette_geometry(self.develop.geometry);
        let options = PipelineOptions::new(ProcessingQuality::Preview)
            .mask_atlas_edge(interactive_mask_edge())
            .mask_layers(preview_mask_layer_capacity(masks.masks.len()))
            .programs(&template);
        match RawGpuPipeline::new(
            &render_state.device,
            &render_state.queue,
            &reduced,
            &params,
            options,
        ) {
            Ok(pipeline) => {
                calibraw_core::diagnostics::record(format!(
                    "Interactive preview graph built: {}x{} ({:.1} MiB)",
                    reduced.width,
                    reduced.height,
                    pipeline.resident_bytes() as f64 / (1024.0 * 1024.0)
                ));
                self.preview.interactive.graph = Some(InteractivePreview {
                    pipeline: self.present_pipeline(pipeline, render_state),
                    raw: reduced,
                    source,
                    rendered: false,
                });
                self.preview.interactive.synced = false;
            }
            Err(error) => {
                self.preview.interactive.failed_for = Some(source);
                // Memory the fitted preview and an export tile leave is not
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
        let upload = if state.masks_pending {
            Self::upload_preview_masks(
                graph.pipeline.gpu(),
                &render_state.queue,
                &preview_masks,
                &graph.raw,
            )
        } else {
            Ok(())
        };
        if stage == ProcessingStage::Raw
            && full_raw.uses_opposed_chroma(&self.develop.target_exposure)
        {
            full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }
        let params = GpuParams::new(&self.develop.target_exposure, &preview_masks, &graph.raw)
            .with_vignette_geometry(self.develop.geometry);
        let result = upload.map_err(anyhow::Error::msg).and_then(|()| {
            graph.pipeline.gpu().dispatch_stage_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                stage,
                RemoveSceneContext::full_frame(
                    &self.inpaint.edits,
                    &full_raw,
                    &self.develop.target_exposure,
                ),
            )
        });
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(now: Instant) -> DragInput {
        DragInput {
            eligible: true,
            dragging_control: false,
            edit: false,
            fitted_current: false,
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
            fitted_current: true,
            ..input(at)
        };
        assert_eq!(settling.next(current(start)), settling);
        assert_eq!(
            settling.next(DragInput {
                fitted_current: false,
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
        let mut state = InteractiveState::default();

        // The first drag: nothing is known to be in step, so all stages run
        // and the masks go up, whatever the edit was.
        state.enter(dragging, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Raw));
        assert!(state.masks_pending);

        // Later edits of the drag only add the stage they need.
        state.pending = None;
        state.masks_pending = false;
        state.synced = true;
        state.enter(dragging, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Output));
        state.enter(dragging, Some(ProcessingStage::Raw));
        assert_eq!(state.pending, Some(ProcessingStage::Raw));

        // After a pause the drag resumes without a full render.
        state.pending = None;
        state.enter(DragPhase::Settling { since: now }, None);
        assert!(state.synced);
        state.enter(dragging, Some(ProcessingStage::Output));
        assert_eq!(state.pending, Some(ProcessingStage::Output));
        assert!(!state.masks_pending);

        // An edit while the fitted preview owns them puts it behind.
        state.enter(DragPhase::Idle, Some(ProcessingStage::Output));
        assert!(!state.synced);
        state.enter(dragging, None);
        assert_eq!(state.pending, Some(ProcessingStage::Raw));
        assert!(state.masks_pending);
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
        assert_eq!(interactive_edge(MIN_FITTED_EDGE), Some(640));
        assert_eq!(interactive_edge(MIN_FITTED_EDGE - 1), None);
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
