//! Rendering only the parts of the output stage an edit reaches.
//!
//! A preview's output stage runs in three parts, each reading the result of
//! the part before from a work texture:
//!
//! - [`OutputPart::Scene`]: scene preparation, tone, local tone, the detail
//!   pyramid, the scene effects, the blurs (Lens Blur among them) and Wet
//!   Surfaces, up to the creative input;
//! - [`OutputPart::Creative`]: scene emission, the Glow passes and map,
//!   Pixelate's blocks, the creative pass and the pass after it (Rain, Wet
//!   Surfaces), up to the view input;
//! - [`OutputPart::View`]: the view transform and display (`apply_view_node`).
//!
//! [`RawGpuPipeline::dispatch_stage`] compares what a render reads
//! ([`OutputInputs`]) with what the pipeline's last preview render read, and
//! starts at the first part whose inputs changed, while the work texture
//! holding that part's input is still intact. A drag of a creative or view
//! control then skips Lens Blur and everything else before it. The parts read
//! the same inputs and run the same passes as a whole render, so the output is
//! the same.
//!
//! What each part reads is decided by which shaders read a value:
//!
//! - the view node alone reads the global and mask colour mixer, colour
//!   grading, hue rotation, brightness, Blacks, point colours and the LUT
//!   (view_transform.wgsl, look_lut.wgsl), and Vignette's frame in the
//!   cropped output (film_finish.wgsl, the only caller of
//!   `effect_output_uv`), which a crop or straighten changes;
//! - the view node reads the display tone curve (Contrast and the curve's
//!   shape, `sigmoid_*` in tonemap.wgsl); before it only the Glow's core
//!   does, in the creative pass (styled_glow.wgsl);
//! - a mask slot is read by the passes its effect runs in
//!   ([`MaskEffect::passes`]), an adjustment's by the scene passes and the
//!   view node;
//! - the render tier ([`RenderTier`]) by the parts with an effect that has a
//!   Draft tier or reads the scene bake, whose lights it builds;
//! - everything else counts as read by the Scene part.
//!
//! The tier differs from the other inputs: a part rendered Final stays while
//! the tier is Draft, as Draft only makes a drag quicker and shows the same
//! look with less detail, and a part rendered Draft renders again once the
//! tier is Final. A drag of a View or Creative control therefore leaves a
//! Final Lens Blur as it is, and needs no Final render of it on release. Lens
//! Blur's resources and the scene lights, which the tier sizes, are prepared
//! only by renders that run the parts reading them
//! ([`RawGpuPipeline::prepare_tiered_effects`]), so a part that stays keeps
//! what it was rendered from.
//!
//! Anything else that writes the work textures or changes what the output
//! stage reads without being a parameter (the RAW and tone stages, exports,
//! new tone statistics a crop inherits) forgets the last render
//! ([`OutputResume::forget`]); uploaded mask layers start at the first part
//! that reads them.

use super::*;
use crate::pipeline::{EffectPass, SceneNeeds};
use std::sync::atomic::AtomicU64;

/// A part of the output stage, in pass order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum OutputPart {
    Scene,
    Creative,
    View,
}

impl OutputPart {
    const ALL: [Self; 3] = [Self::Scene, Self::Creative, Self::View];

    const fn of_pass(pass: EffectPass) -> Self {
        match pass {
            EffectPass::CaptureSharpening
            | EffectPass::SceneEffects
            | EffectPass::Blur
            | EffectPass::BeforeCreative => Self::Scene,
            EffectPass::Creative | EffectPass::CreativeLight | EffectPass::AfterCreative => {
                Self::Creative
            }
            EffectPass::DisplayFinish => Self::View,
        }
    }

    /// The first part that reads `slot`: its effect's first pass, the Scene
    /// part for an adjustment and for an ID no effect has.
    fn of_slot(slot: &MaskData) -> Self {
        match MaskEffect::from_shader_id(slot.effect_id()) {
            None | Some(MaskEffect::Adjustment) => Self::Scene,
            Some(effect) => effect
                .passes()
                .iter()
                .map(|pass| Self::of_pass(*pass))
                .min()
                .unwrap_or(Self::Scene),
        }
    }
}

/// Where the next output render starts, with the work texture holding that
/// part's input (`true`: the second).
#[derive(Clone, Copy, Debug)]
pub(super) enum OutputStart {
    Scene,
    Creative {
        input_in_tex2: bool,
    },
    View {
        input_in_tex2: bool,
        /// Where the creative input stays, which the view node leaves alone.
        creative_input_in_tex2: Option<bool>,
    },
}

impl OutputStart {
    /// The part the render starts at.
    pub(super) const fn part(self) -> OutputPart {
        match self {
            Self::Scene => OutputPart::Scene,
            Self::Creative { .. } => OutputPart::Creative,
            Self::View { .. } => OutputPart::View,
        }
    }

    /// How `stage_timing` names the render: "Output", or the part it starts at.
    pub(super) const fn timing_label(self) -> &'static str {
        match self {
            Self::Scene => "Output",
            Self::Creative { .. } => "Output from Creative",
            Self::View { .. } => "Output from View",
        }
    }
}

/// Which work texture holds each part's input after an output render
/// (`true`: the second).
#[derive(Clone, Copy, Debug)]
pub(super) struct OutputTextures {
    /// `None` once a pass after the creative input overwrote it (Rain and Wet
    /// Surfaces' pass after the creative pass).
    pub(super) creative_input_in_tex2: Option<bool>,
    pub(super) view_input_in_tex2: bool,
}

/// What the passes of the Scene part are, which decides the work texture its
/// result ends in and what the scene bake holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SceneShape {
    intermediate: bool,
    local_tone: bool,
    scene_effects: bool,
    detail_levels: Option<usize>,
    blur: bool,
    progressive_blur: bool,
    defocus_slots: usize,
    wet_surfaces: bool,
    weather_air: bool,
    scene_geometry: bool,
    scene_image: bool,
}

impl SceneShape {
    fn of(params: &GpuParams) -> Self {
        Self {
            intermediate: params.needs_intermediate_adjustment_passes(),
            local_tone: params.needs_local_tone_pass(),
            scene_effects: params.needs_scene_effects_pass(),
            detail_levels: params.detail_pyramid_levels(),
            blur: params.needs_blur_passes(),
            progressive_blur: params.needs_progressive_blur_passes(),
            defocus_slots: params.defocus_slots(),
            wet_surfaces: params.needs_wet_surfaces_pass(),
            weather_air: params.needs_weather_air_pass(),
            scene_geometry: params.needs_scene_geometry(),
            scene_image: params.needs_scene_image(),
        }
    }
}

/// What an output render reads: the uniforms as uploaded, the mask slots in
/// use, the resources the parameters name and the shared maps' builds. What
/// only the View part reads (the LUT among it) is never compared: every
/// render runs that part.
pub(super) struct OutputInputs {
    camera: CameraUniforms,
    scene_tone: SceneToneUniforms,
    effects: EffectsUniforms,
    slots: Box<[MaskData]>,
    shape: SceneShape,
    depth: Option<SceneDepth>,
    profile: Arc<ProfileColor>,
    highlight_model: Option<Arc<HighlightModel>>,
    render_tier: RenderTier,
    /// The parts that read `render_tier`, by [`OutputPart`].
    tier_parts: [bool; OutputPart::ALL.len()],
    /// The build of the Lens Blur map the Scene part reads, if any.
    defocus_map: Option<u64>,
    /// The build of the Glow map the Creative part reads, if any.
    styled_glow_map: Option<u64>,
}

impl OutputInputs {
    pub(super) fn new(
        params: &GpuParams,
        uploaded: &UploadedStageUniforms,
        maps: SharedMapBuilds,
    ) -> Self {
        Self {
            camera: uploaded.camera,
            scene_tone: uploaded.scene_tone,
            effects: uploaded.effects,
            slots: params.mask_slots().into(),
            shape: SceneShape::of(params),
            depth: params.depth_result.clone(),
            profile: Arc::clone(&params.profile_color),
            highlight_model: params.highlight_model.clone(),
            render_tier: params.render_tier(),
            tier_parts: parts_reading_tier(params.mask_slots()),
            defocus_map: maps.defocus,
            styled_glow_map: maps.styled_glow,
        }
    }

    /// Takes the shared maps' builds after a render, which rebuilt the maps
    /// this pipeline owns.
    pub(super) fn with_map_builds(self, maps: SharedMapBuilds) -> Self {
        Self {
            defocus_map: maps.defocus,
            styled_glow_map: maps.styled_glow,
            ..self
        }
    }

    /// The first part whose inputs other than the render tier differ between
    /// these and `next`.
    fn first_changed_part(&self, next: &Self) -> OutputPart {
        if !self.same_scene_inputs(next) {
            OutputPart::Scene
        } else if !self.same_creative_inputs(next) {
            OutputPart::Creative
        } else {
            OutputPart::View
        }
    }

    /// The first part that reads the render tier and was rendered more
    /// coarsely than these inputs ask: Draft when they ask for Final.
    fn first_part_to_refine(&self, rendered: &PartTiers) -> Option<OutputPart> {
        OutputPart::ALL.into_iter().find(|part| {
            self.tier_parts[*part as usize]
                && rendered[*part as usize] == RenderTier::Draft
                && self.render_tier == RenderTier::Final
        })
    }

    fn same_scene_inputs(&self, next: &Self) -> bool {
        bytes_equal(&self.camera, &next.camera)
            && bytes_equal(
                &scene_tone_before_creative(&self.scene_tone),
                &scene_tone_before_creative(&next.scene_tone),
            )
            && bytes_equal(
                &effects_before_creative(&self.effects),
                &effects_before_creative(&next.effects),
            )
            && self.shape == next.shape
            && same_depth_result(self.depth.as_ref(), next.depth.as_ref())
            && Arc::ptr_eq(&self.profile, &next.profile)
            && same_arc(self.highlight_model.as_ref(), next.highlight_model.as_ref())
            && self.defocus_map == next.defocus_map
            && self.slots.len() == next.slots.len()
            && self.slots.iter().zip(&next.slots[..]).all(|(slot, next)| {
                slot.metadata == next.metadata
                    && (OutputPart::of_slot(slot) > OutputPart::Scene
                        || bytes_equal(&slot_before_view(slot), &slot_before_view(next)))
            })
    }

    /// Whether the Creative part's inputs are the same, given the Scene
    /// part's are.
    fn same_creative_inputs(&self, next: &Self) -> bool {
        bytes_equal(
            &effects_before_view(&self.effects),
            &effects_before_view(&next.effects),
        ) && (!next.slots.iter().any(styled_glow::is_styled_glow)
            || bytes_equal(
                &scene_tone_before_view(&self.scene_tone),
                &scene_tone_before_view(&next.scene_tone),
            ))
            && self.styled_glow_map == next.styled_glow_map
            && self.slots.iter().zip(&next.slots[..]).all(|(slot, next)| {
                OutputPart::of_slot(slot) == OutputPart::View
                    || bytes_equal(&slot_before_view(slot), &slot_before_view(next))
            })
    }

    /// The first part that reads one of `layers` of the mask atlases: that of
    /// the first active slot masked by one. `None` when no active slot reads
    /// them, as inactive slots skip their mask.
    fn first_part_reading(&self, layers: &[bool; MAX_LOCAL_MASKS]) -> Option<OutputPart> {
        self.slots
            .iter()
            .filter(|slot| slot.is_active())
            .filter(|slot| {
                layers
                    .get(slot.point_color_meta[2] as usize)
                    .is_some_and(|dirty| *dirty)
            })
            .map(OutputPart::of_slot)
            .min()
    }
}

/// The parts whose result depends on the render tier: those an active slot
/// runs in whose effect has a Draft tier ([`MaskEffect::has_draft_tier`]) or
/// reads the scene bake, whose lights the tier builds.
fn parts_reading_tier(slots: &[MaskData]) -> [bool; OutputPart::ALL.len()] {
    let mut parts = [false; OutputPart::ALL.len()];
    for effect in slots.iter().filter_map(|slot| {
        let effect = slot.active_effect()?;
        let reads_tier = effect.has_draft_tier()
            || slot
                .scene_needs()
                .is_some_and(|needs| needs != SceneNeeds::NONE);
        reads_tier.then_some(effect)
    }) {
        for pass in effect.passes() {
            parts[OutputPart::of_pass(*pass) as usize] = true;
        }
    }
    parts
}

/// The tier each part's result was rendered in, by [`OutputPart`].
type PartTiers = [RenderTier; OutputPart::ALL.len()];

/// The builds of the full-frame maps a pipeline reads, which change when the
/// pipeline that owns them renders, without this one's parameters changing.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SharedMapBuilds {
    pub(super) defocus: Option<u64>,
    pub(super) styled_glow: Option<u64>,
}

/// The last preview render of the output stage, to start the next one from.
pub(super) struct OutputResume {
    last: Option<LastRender>,
    /// Mask layers uploaded since `last` rendered.
    dirty_layers: [bool; MAX_LOCAL_MASKS],
}

impl OutputResume {
    pub(super) const fn new() -> Self {
        Self {
            last: None,
            dirty_layers: [false; MAX_LOCAL_MASKS],
        }
    }

    /// Where a render of `next` starts.
    pub(super) fn start(&self, next: &OutputInputs) -> OutputStart {
        let Some(LastRender {
            inputs: last,
            textures,
            tiers,
        }) = &self.last
        else {
            return OutputStart::Scene;
        };
        let part = [
            Some(last.first_changed_part(next)),
            next.first_part_reading(&self.dirty_layers),
            next.first_part_to_refine(tiers),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(OutputPart::Scene);
        match part {
            OutputPart::Scene => OutputStart::Scene,
            OutputPart::Creative => match textures.creative_input_in_tex2 {
                Some(input_in_tex2) => OutputStart::Creative { input_in_tex2 },
                None => OutputStart::Scene,
            },
            OutputPart::View => OutputStart::View {
                input_in_tex2: textures.view_input_in_tex2,
                creative_input_in_tex2: textures.creative_input_in_tex2,
            },
        }
    }

    /// Remembers a render of `inputs` from `start` that left its parts'
    /// inputs in `textures`. The parts before `start` keep the tier they were
    /// rendered in.
    pub(super) fn record(
        &mut self,
        inputs: OutputInputs,
        start: OutputStart,
        textures: OutputTextures,
    ) {
        let kept = self.last.as_ref().map(|last| last.tiers);
        let tiers = OutputPart::ALL.map(|part| match kept {
            Some(kept) if part < start.part() => kept[part as usize],
            _ => inputs.render_tier,
        });
        self.last = Some(LastRender {
            inputs,
            textures,
            tiers,
        });
        self.dirty_layers = [false; MAX_LOCAL_MASKS];
    }

    /// Forgets the last render: the next one starts at the Scene part.
    pub(super) fn forget(&mut self) {
        self.last = None;
        self.dirty_layers = [false; MAX_LOCAL_MASKS];
    }

    /// Notes an upload of mask `layer`, which the next render reads anew.
    pub(super) fn note_mask_layer(&mut self, layer: usize) {
        match self.dirty_layers.get_mut(layer) {
            Some(dirty) => *dirty = true,
            // Not a layer slots name: nothing could tell which part reads it.
            None => self.forget(),
        }
    }
}

/// The last preview render of the output stage.
struct LastRender {
    inputs: OutputInputs,
    textures: OutputTextures,
    /// The tier each part's result was rendered in.
    tiers: PartTiers,
}

/// A build of something other pipelines read: a number no other build of any
/// pipeline has, which changes each time it is built again.
#[derive(Debug)]
pub(super) struct BuildRevision(AtomicU64);

impl BuildRevision {
    pub(super) fn new() -> Self {
        Self(AtomicU64::new(next_build()))
    }

    pub(super) fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    pub(super) fn bump(&self) {
        self.0.store(next_build(), Ordering::Release);
    }
}

fn next_build() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn bytes_equal<T: Pod>(a: &T, b: &T) -> bool {
    bytemuck::bytes_of(a) == bytemuck::bytes_of(b)
}

fn same_arc<T>(a: Option<&Arc<T>>, b: Option<&Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn same_depth_result(a: Option<&SceneDepth>, b: Option<&SceneDepth>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => scene::same_depth(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// `scene_tone` without what only the view node reads. Blacks is the fourth
/// lane of `basic_tone`: the scene tone pass takes it as an unused argument
/// (`apply_local_basic_tone_values_with_low_strength` in tonemap.wgsl), and
/// the view node applies it as a display toe, with its response bit.
pub(super) fn scene_tone_before_view(scene_tone: &SceneToneUniforms) -> SceneToneUniforms {
    let [highlights, shadows, whites, _blacks] = scene_tone.basic_tone;
    SceneToneUniforms {
        basic_tone: [highlights, shadows, whites, 0.0],
        legacy_luminance_tone: scene_tone.legacy_luminance_tone & !LEGACY_LUMINANCE_BLACKS,
        hsl_hue_0: [0.0; 4],
        hsl_hue_1: [0.0; 4],
        hsl_saturation_0: [0.0; 4],
        hsl_saturation_1: [0.0; 4],
        hsl_luminance_0: [0.0; 4],
        hsl_luminance_1: [0.0; 4],
        grade_shadows: [0.0; 4],
        grade_midtones: [0.0; 4],
        grade_highlights: [0.0; 4],
        grade_global: [0.0; 4],
        grade_options: [0.0; 4],
        point_colors: Zeroable::zeroed(),
        point_color_meta: [0; 4],
        look_lut: [0.0; 4],
        look_lut_domain_min: [0.0; 4],
        look_lut_domain_scale: [0.0; 4],
        ..*scene_tone
    }
}

/// `scene_tone` without what only the Creative part and the view node read:
/// also the display tone curve, which the Glow's core reads in the creative
/// pass (`styled_glow_core_ceiling`) and nothing reads before it.
pub(super) fn scene_tone_before_creative(scene_tone: &SceneToneUniforms) -> SceneToneUniforms {
    SceneToneUniforms {
        sigmoid_curve: [0.0; 4],
        sigmoid_power: [0.0; 4],
        reference_sigmoid_curve: [0.0; 4],
        reference_sigmoid_power: [0.0; 4],
        ..scene_tone_before_view(scene_tone)
    }
}

/// `effects` without what only the view node reads: Vignette's frame in the
/// output and its fits (film_finish.wgsl).
pub(super) fn effects_before_view(effects: &EffectsUniforms) -> EffectsUniforms {
    EffectsUniforms {
        vignette_frame: [0.0; 4],
        vignette_projection: [[0.0; 4]; 3],
        vignette_dark_half_fit: [0.0; 4],
        vignette_dark_full_fit: [0.0; 4],
        vignette_light_half_fit: [0.0; 4],
        vignette_light_full_fit: [0.0; 4],
        ..*effects
    }
}

/// `effects` without what only the Creative part and the view node read:
/// also the Glow's diffusion radius and which of its pyramids are ready.
pub(super) fn effects_before_creative(effects: &EffectsUniforms) -> EffectsUniforms {
    EffectsUniforms {
        glow_diffusion: [0.0; 4],
        ..effects_before_view(effects)
    }
}

/// A mask slot without what only the view node reads: an adjustment's colour
/// mixer, grading, hue rotation, brightness, Blacks and point colours. Its
/// mask layer (`point_color_meta`) stays.
pub(super) fn slot_before_view(slot: &MaskData) -> MaskData {
    // Blacks is the second lane of an adjustment's `adjust_1`
    // (`apply_local_display_tone` in view_transform.wgsl), with its response
    // bit in `metadata[3]`; an effect slot keeps its own lanes and bits there.
    let mut adjust_1 = slot.adjust_1;
    let mut metadata = slot.metadata;
    if slot.effect_id() == MaskEffect::Adjustment.shader_id() {
        adjust_1[1] = 0.0;
        metadata[3] &= !LEGACY_LUMINANCE_BLACKS;
    }
    MaskData {
        metadata,
        adjust_1,
        grade_shadows: [0.0; 4],
        grade_midtones: [0.0; 4],
        grade_highlights: [0.0; 4],
        grade_global: [0.0; 4],
        grade_options: [0.0; 4],
        hsl_hue_0: [0.0; 4],
        hsl_hue_1: [0.0; 4],
        hsl_saturation_0: [0.0; 4],
        hsl_saturation_1: [0.0; 4],
        hsl_luminance_0: [0.0; 4],
        hsl_luminance_1: [0.0; 4],
        point_colors: Zeroable::zeroed(),
        ..*slot
    }
}
