//! What every effect's settings provide: their starting point, when they
//! change the image, their controls and how they pack for the shader.

use super::lanes::{EffectLanes, PackContext};
use super::params::{ColorParamSpec, FloatParamSpec, ToggleSpec};

/// The settings of one effect (`MaskEffectSettings` holds one of each).
pub trait EffectSettings: Copy + Default + PartialEq {
    /// The lanes [`Self::pack`] fills, for the shaders (`effect_lanes!`).
    const LANES: &'static [super::lanes::LaneName];

    /// The settings an effect starts with when it is added or its card is
    /// reset.
    ///
    /// `Default` is the serialized baseline: omitted fields and omitted
    /// settings decode to it, so changing it would change saved edits. An
    /// effect whose starting point differs from that baseline overrides this.
    fn initial() -> Self {
        Self::default()
    }

    /// Whether these settings change the image.
    fn is_active(&self) -> bool;

    /// Every value an edit stores, in the order the effect's card shows it.
    /// Sidecars are validated against these specs, and effects without a card
    /// of their own get one drawn from them. A choice between modes (Radial
    /// Blur's) is valid once decoded and has no control here, so its effect
    /// draws its own card.
    fn controls(&mut self) -> EffectControls<'_>;

    /// Whether an active effect with these settings asks for scene depth,
    /// which is then generated (`ContentDependencies::scene_depth`).
    fn wants_scene_depth(&self) -> bool {
        false
    }

    /// How many slots the settings fill: one per instance of an effect with
    /// several in one card (Relight's lights, Light Rays' sources; see
    /// `instances.rs`), else one.
    fn instance_count(&self) -> usize {
        1
    }

    /// Writes the values the shader reads into [`Self::LANES`], for the slot
    /// of instance `context.instance`.
    fn pack(&self, lanes: &mut EffectLanes, context: &PackContext);
}

/// One stored value with its spec.
#[derive(Debug)]
pub enum EffectControl<'a> {
    Slider(&'a mut f32, FloatParamSpec),
    /// An angle in degrees, drawn as a dial.
    Angle(&'a mut f32, FloatParamSpec),
    /// A random pattern's seed: a number and a Shuffle button.
    Seed(&'a mut f32, FloatParamSpec),
    Color(&'a mut [f32; 3], ColorParamSpec),
    Toggle(&'a mut bool, ToggleSpec),
    /// A point given as percentages along x and y. A card of the effect's
    /// own draws it on a pad of its frame; a generated card shows its two
    /// coordinates.
    Position(&'a mut [f32; 2], [FloatParamSpec; 2]),
}

/// An effect's controls: those its card shows, then those collapsed under a
/// heading.
#[derive(Debug)]
pub struct EffectControls<'a> {
    pub shown: Vec<EffectControl<'a>>,
    pub details: Option<EffectDetails<'a>>,
}

/// The collapsed part of a card.
#[derive(Debug)]
pub struct EffectDetails<'a> {
    pub title: &'static str,
    pub controls: Vec<EffectControl<'a>>,
}

impl<'a> EffectControls<'a> {
    pub fn new(shown: Vec<EffectControl<'a>>) -> Self {
        Self {
            shown,
            details: None,
        }
    }

    /// Adds `controls`, collapsed under `title`.
    pub fn details(self, title: &'static str, controls: Vec<EffectControl<'a>>) -> Self {
        Self {
            details: Some(EffectDetails { title, controls }),
            ..self
        }
    }

    /// Every control, shown ones first.
    pub fn all(&self) -> impl Iterator<Item = &EffectControl<'a>> {
        self.shown
            .iter()
            .chain(self.details.iter().flat_map(|details| &details.controls))
    }
}
