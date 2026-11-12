//! Effect families: effects of one kind, which the picker offers as one tile
//! and whose cards switch between them with a Style control.
//!
//! A family changes only how effects are offered. Each style is an effect of
//! the registry (`mask_effects!`), with Radial Blur's two modes as two
//! styles, so sidecars, presets, shader IDs and rendering stay those of the
//! effects.

use super::{
    EffectControl, EffectSettings, EffectSettingsVisitor, MaskEffect, MaskEffectCategory,
    MaskEffectSettings, RadialBlurMode,
};
use crate::pipeline::EffectComponent;

/// The families, named by each effect's `family` in the registry, which
/// lists their effects. An effect with no kin is a family of its own, named
/// after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectFamily {
    Blur,
    Sharpen,
    Glow,
    LightRays,
    Neon,
    Relight,
    Pixelate,
    Atmosphere,
    Grain,
    Halation,
    Vignette,
    Rain,
    WetSurfaces,
}

/// One way a family's effect can look: an effect, and for Radial Blur its
/// mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectStyle {
    effect: MaskEffect,
    /// Radial Blur's mode; `None` for every other effect.
    radial_mode: Option<RadialBlurMode>,
}

impl EffectStyle {
    pub const fn effect(self) -> MaskEffect {
        self.effect
    }

    /// The style's name on its family's Style control.
    pub const fn label(self) -> &'static str {
        match (self.effect, self.radial_mode) {
            (MaskEffect::Blur, _) => "Soft",
            (_, Some(mode)) => mode.label(),
            (effect, None) => effect.label(),
        }
    }

    /// The style `component` shows.
    pub fn of(component: &EffectComponent) -> Self {
        Self {
            effect: component.effect,
            radial_mode: (component.effect == MaskEffect::RadialBlur)
                .then_some(component.settings.radial_blur.mode),
        }
    }
}

impl EffectFamily {
    /// The family's styles, in the order its Style control lists them: its
    /// effects in the picker's order ([`MaskEffect::ALL`]), Radial Blur as
    /// Zoom and then Spin. A tile adds the first whose effect the list lacks,
    /// which shows that effect at its starting settings.
    pub fn styles(self) -> impl Iterator<Item = EffectStyle> {
        MaskEffect::ALL
            .into_iter()
            .filter(move |effect| effect.family() == Some(self))
            .flat_map(|effect| {
                let modes: &[Option<RadialBlurMode>] = match effect {
                    MaskEffect::RadialBlur => {
                        &[Some(RadialBlurMode::ALL[0]), Some(RadialBlurMode::ALL[1])]
                    }
                    _ => &[None],
                };
                modes.iter().map(move |&radial_mode| EffectStyle {
                    effect,
                    radial_mode,
                })
            })
    }

    /// The family's first effect, which leads its styles and places its tile.
    pub fn first_effect(self) -> MaskEffect {
        self.styles()
            .next()
            .map_or(MaskEffect::Adjustment, EffectStyle::effect)
    }

    /// Whether its cards offer more than one style.
    pub fn has_styles(self) -> bool {
        self.styles().nth(1).is_some()
    }

    /// The name of the family's tile and cards: its first effect's for a
    /// family of one.
    pub fn label(self) -> &'static str {
        match self {
            Self::Blur => "Blur",
            Self::Neon => "Neon",
            Self::Atmosphere => "Atmosphere",
            single => single.first_effect().label(),
        }
    }

    /// The picker category of the family's tile: its first effect's.
    pub fn category(self) -> Option<MaskEffectCategory> {
        self.first_effect().category()
    }

    /// The styles a list of `components` can still take: those whose effect
    /// it lacks, as it holds each effect at most once. Effects switched off
    /// for good ([`MaskEffect::is_disabled`]) are never offered.
    pub fn styles_to_add(
        self,
        components: &[EffectComponent],
    ) -> impl Iterator<Item = EffectStyle> + '_ {
        self.styles().filter(move |style| {
            !style.effect.is_disabled()
                && !components
                    .iter()
                    .any(|component| component.effect == style.effect)
        })
    }

    /// The styles `component`'s card offers among `components`, the list it
    /// is in: its own effect's, and those [`Self::styles_to_add`] offers.
    pub fn styles_for<'a>(
        self,
        component: &'a EffectComponent,
        components: &'a [EffectComponent],
    ) -> impl Iterator<Item = EffectStyle> + 'a {
        self.styles().filter(move |style| {
            style.effect == component.effect
                || (!style.effect.is_disabled()
                    && !components.iter().any(|other| other.effect == style.effect))
        })
    }
}

impl EffectComponent {
    /// Shows `style` instead: the effect it names at its starting settings,
    /// with every value whose field the current effect's settings share,
    /// within the new effect's ranges. The component stays enabled or not.
    pub fn set_style(&mut self, style: EffectStyle) {
        if style.effect != self.effect {
            let mut settings = MaskEffectSettings::initial(style.effect);
            if let (
                Some(serde_json::Value::Object(current)),
                Some(serde_json::Value::Object(mut fields)),
            ) = (
                self.settings.fields(self.effect),
                settings.fields(style.effect),
            ) {
                for (name, value) in current {
                    if let Some(field) = fields.get_mut(&name) {
                        if same_shape(field, &value) {
                            *field = value;
                        }
                    }
                }
                let mut carried = settings;
                if carried.set_fields(style.effect, serde_json::Value::Object(fields)) {
                    carried.visit_mut(style.effect, ClampToSpecs);
                    settings = carried;
                }
            }
            self.effect = style.effect;
            self.settings = settings;
        }
        if let Some(mode) = style.radial_mode {
            self.settings.radial_blur.mode = mode;
        }
    }
}

/// Whether `value` can stand for `field`: both numbers, both booleans, both
/// strings, or lists or objects of the same length.
fn same_shape(field: &serde_json::Value, value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (field, value) {
        (Value::Number(_), Value::Number(_))
        | (Value::Bool(_), Value::Bool(_))
        | (Value::String(_), Value::String(_)) => true,
        (Value::Array(field), Value::Array(value)) => field.len() == value.len(),
        (Value::Object(field), Value::Object(value)) => {
            field.len() == value.len() && field.keys().all(|key| value.contains_key(key))
        }
        _ => false,
    }
}

/// Brings every value of an effect's settings into its control's range.
struct ClampToSpecs;

impl EffectSettingsVisitor for ClampToSpecs {
    type Output = ();

    fn visit<S: EffectSettings>(self, _: MaskEffect, settings: &mut S) {
        let controls = settings.controls();
        let details = controls
            .details
            .into_iter()
            .flat_map(|details| details.controls);
        for control in controls.shown.into_iter().chain(details) {
            match control {
                EffectControl::Slider(value, spec)
                | EffectControl::Angle(value, spec)
                | EffectControl::Seed(value, spec) => *value = spec.clamp(*value),
                EffectControl::Color(color, spec) => *color = spec.clamp(*color),
                EffectControl::Position(point, [x, y]) => {
                    *point = [x.clamp(point[0]), y.clamp(point[1])];
                }
                EffectControl::Toggle(..) => {}
            }
        }
    }
}
