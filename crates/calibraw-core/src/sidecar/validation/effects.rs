//! Validation of mask effect settings against their controls' specs.

use super::*;
use crate::pipeline::{EffectControl, EffectControls, MaskEffect, MaskEffectSettings};

/// Checks every effect's settings, whichever effect the slot shows: each
/// value must be finite and within its control's range.
pub(super) fn validate_effect_settings(settings: &MaskEffectSettings) -> Result<(), SidecarError> {
    // Controls lend their values mutably, for the cards; validation reads a copy.
    let mut settings = *settings;
    for (effect, controls) in settings.all_controls() {
        validate_effect_controls(effect, &controls)?;
    }
    Ok(())
}

fn validate_effect_controls(
    effect: MaskEffect,
    controls: &EffectControls<'_>,
) -> Result<(), SidecarError> {
    let finite = controls.all().all(|control| match control {
        EffectControl::Slider(value, _)
        | EffectControl::Angle(value, _)
        | EffectControl::Seed(value, _) => value.is_finite(),
        EffectControl::Color(color, _) => color.iter().all(|channel| channel.is_finite()),
        EffectControl::Position(position, _) => position.iter().all(|value| value.is_finite()),
        EffectControl::Toggle(..) => true,
    });
    if !finite {
        return invalid(&format!(
            "{} mask effect contains a non-finite value",
            effect.label()
        ));
    }
    let bounded_value = |value: f32, spec: crate::pipeline::effect_params::FloatParamSpec| {
        bounded(
            &format!("{} {}", effect.label(), spec.label),
            value,
            spec.min,
            spec.max,
        )
    };
    for control in controls.all() {
        match control {
            EffectControl::Slider(value, spec)
            | EffectControl::Angle(value, spec)
            | EffectControl::Seed(value, spec) => bounded_value(**value, *spec)?,
            EffectControl::Position(position, specs) => {
                for (value, spec) in position.iter().zip(specs) {
                    bounded_value(*value, *spec)?;
                }
            }
            EffectControl::Color(..) | EffectControl::Toggle(..) => {}
        }
    }
    for control in controls.all() {
        if let EffectControl::Color(color, spec) = control {
            for channel in color.iter() {
                bounded(
                    &format!("{} {} channel", effect.label(), spec.label),
                    *channel,
                    spec.min,
                    spec.max,
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn validate_curves(
    curves: &[&crate::pipeline::PointCurve],
    label: &str,
) -> Result<(), SidecarError> {
    for curve in curves {
        if !(2..=crate::pipeline::MAX_POINT_CURVE_POINTS as u32).contains(&curve.len) {
            return invalid("tone curve point count is invalid");
        }
        for point in curve.points {
            finite(label, &point)?;
        }
    }
    Ok(())
}
