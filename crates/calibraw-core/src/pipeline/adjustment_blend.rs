//! Fading adjustment groups part of the way between two edits, for the
//! amount of an applied preset. Only settings that can take values in between
//! fade; switches and choices keep the newer edit's value.
use super::{
    AdjustmentGroupSet, ColorGradeWheel, ColorGrading, ExposureParams, LoadedRaw, PointColor,
    PointColorRange, PointColors, PointCurve, WhiteBalance, MAX_POINT_CURVE_POINTS,
};

impl ExposureParams {
    /// `self`, an edit whose `groups` were just set over `before` (by
    /// applying a preset), with those groups `amount` of the way from
    /// `before`'s settings to `self`'s: 0 gives `before`'s, 1 gives `self`.
    ///
    /// Settings that cannot take a value in between, such as switches and
    /// choices, keep `self`'s at every amount. A custom white balance fades
    /// in temperature and tint on `raw`, which supplies the as-shot one; it
    /// keeps `self`'s without `raw`.
    pub fn faded_from(
        &self,
        before: &Self,
        groups: AdjustmentGroupSet,
        amount: f32,
        raw: Option<&LoadedRaw>,
    ) -> Self {
        if !amount.is_finite() || amount >= 1.0 {
            return *self;
        }
        let amount = amount.max(0.0);
        // Every setting outside `groups` is the same on both sides, so
        // fading the whole edit changes only those of `groups`.
        let mut from = *self;
        for group in groups.iter() {
            from.copy_group_from(before, group);
        }
        let mut faded = from.blend(self, amount);
        if let Some(raw) = raw {
            faded.white_balance =
                raw.blend_white_balance(from.white_balance, self.white_balance, amount);
        }
        faded.sanitize_tone_curves();
        faded
    }

    /// Every setting `amount` (0–1) of the way to `target`'s.
    fn blend(&self, target: &Self, amount: f32) -> Self {
        // Destructured without `..`: a new field does not compile until it
        // is given a way to fade.
        let Self {
            black_point,
            exposure,
            contrast,
            sigmoid: _,
            white_balance: _,
            hue,
            saturation,
            vibrance,
            tone_curve,
            tone_curve_red,
            tone_curve_green,
            tone_curve_blue,
            chroma_denoise,
            luminance_denoise,
            denoise_detail,
            denoise_quality: _,
            ai_denoise_enabled: _,
            ai_denoise_amount,
            demosaic_mode: _,
            dual_threshold,
            frequency_chroma,
            highlight_method: _,
            highlight_clip,
            highlight_reconstruction,
            chromatic_aberration,
            highlights,
            shadows,
            whites,
            blacks,
            texture,
            clarity,
            dehaze,
            sharpen_amount,
            sharpen_radius,
            sharpen_detail,
            sharpen_masking,
            hsl_hue,
            hsl_saturation,
            hsl_luminance,
            point_colors,
            point_color_visualize: _,
            color_grading,
        } = *self;
        Self {
            black_point: black_point.blend(target.black_point, amount),
            exposure: exposure.blend(target.exposure, amount),
            contrast: contrast.blend(target.contrast, amount),
            // The tone mapper's settings are one choice.
            sigmoid: target.sigmoid,
            // Needs the photo's as-shot white balance (`faded_from`).
            white_balance: target.white_balance,
            hue: hue.blend(target.hue, amount),
            saturation: saturation.blend(target.saturation, amount),
            vibrance: vibrance.blend(target.vibrance, amount),
            tone_curve: tone_curve.blend(target.tone_curve, amount),
            tone_curve_red: tone_curve_red.blend(target.tone_curve_red, amount),
            tone_curve_green: tone_curve_green.blend(target.tone_curve_green, amount),
            tone_curve_blue: tone_curve_blue.blend(target.tone_curve_blue, amount),
            chroma_denoise: chroma_denoise.blend(target.chroma_denoise, amount),
            luminance_denoise: luminance_denoise.blend(target.luminance_denoise, amount),
            denoise_detail: denoise_detail.blend(target.denoise_detail, amount),
            denoise_quality: target.denoise_quality,
            ai_denoise_enabled: target.ai_denoise_enabled,
            ai_denoise_amount: ai_denoise_amount.blend(target.ai_denoise_amount, amount),
            demosaic_mode: target.demosaic_mode,
            dual_threshold: dual_threshold.blend(target.dual_threshold, amount),
            frequency_chroma: frequency_chroma.blend(target.frequency_chroma, amount),
            highlight_method: target.highlight_method,
            highlight_clip: highlight_clip.blend(target.highlight_clip, amount),
            highlight_reconstruction: highlight_reconstruction
                .blend(target.highlight_reconstruction, amount),
            chromatic_aberration: chromatic_aberration.blend(target.chromatic_aberration, amount),
            highlights: highlights.blend(target.highlights, amount),
            shadows: shadows.blend(target.shadows, amount),
            whites: whites.blend(target.whites, amount),
            blacks: blacks.blend(target.blacks, amount),
            texture: texture.blend(target.texture, amount),
            clarity: clarity.blend(target.clarity, amount),
            dehaze: dehaze.blend(target.dehaze, amount),
            sharpen_amount: sharpen_amount.blend(target.sharpen_amount, amount),
            sharpen_radius: sharpen_radius.blend(target.sharpen_radius, amount),
            sharpen_detail: sharpen_detail.blend(target.sharpen_detail, amount),
            sharpen_masking: sharpen_masking.blend(target.sharpen_masking, amount),
            hsl_hue: hsl_hue.blend(target.hsl_hue, amount),
            hsl_saturation: hsl_saturation.blend(target.hsl_saturation, amount),
            hsl_luminance: hsl_luminance.blend(target.hsl_luminance, amount),
            point_colors: point_colors.blend(target.point_colors, amount),
            point_color_visualize: target.point_color_visualize,
            color_grading: color_grading.blend(target.color_grading, amount),
        }
    }
}

impl LoadedRaw {
    /// A white balance `amount` (0–1) of the way from `from` to `to` on this
    /// photo: temperature in mired, tint linearly, as the sliders show them.
    /// `to` when either cannot be shown, as on a photo without a
    /// white-balance model.
    pub fn blend_white_balance(
        &self,
        from: WhiteBalance,
        to: WhiteBalance,
        amount: f32,
    ) -> WhiteBalance {
        if from == to || amount >= 1.0 {
            return to;
        }
        let (Some((from_kelvin, from_tint)), Some((to_kelvin, to_tint))) =
            (self.shown_white_balance(from), self.shown_white_balance(to))
        else {
            return to;
        };
        let mired = (1.0e6 / from_kelvin).blend(1.0e6 / to_kelvin, amount);
        self.custom_white_balance(1.0e6 / mired, from_tint.blend(to_tint, amount))
            .unwrap_or(to)
    }
}

/// A setting that can take values between two of its values.
trait Blend: Copy + PartialEq {
    /// The value `amount` (0–1) of the way to `target`: `self` at 0,
    /// `target` at 1, and `self` whenever both are equal.
    fn blend(self, target: Self, amount: f32) -> Self;
}

impl Blend for f32 {
    fn blend(self, target: Self, amount: f32) -> Self {
        if self == target {
            self
        } else {
            self * (1.0 - amount) + target * amount
        }
    }
}

impl<const N: usize> Blend for [f32; N] {
    fn blend(self, target: Self, amount: f32) -> Self {
        std::array::from_fn(|index| self[index].blend(target[index], amount))
    }
}

impl Blend for PointCurve {
    /// Curves with the same number of points move each point. Otherwise both
    /// are sampled, between their points linearly, at the points of either
    /// (of the longer one when all would not fit) and the samples move.
    fn blend(self, target: Self, amount: f32) -> Self {
        if self == target || amount <= 0.0 {
            return self;
        }
        if amount >= 1.0 {
            return target;
        }
        let from = curve_points(&self);
        let to = curve_points(&target);
        let mut points = [[1.0, 1.0]; MAX_POINT_CURVE_POINTS];
        let len = if from.len() == to.len() {
            for (point, (from, to)) in points.iter_mut().zip(from.iter().zip(to)) {
                *point = from.blend(*to, amount);
            }
            from.len()
        } else {
            let mut positions: Vec<f32> = from.iter().chain(to).map(|point| point[0]).collect();
            positions.sort_by(f32::total_cmp);
            positions.dedup();
            if positions.len() > MAX_POINT_CURVE_POINTS {
                let longer = if from.len() > to.len() { from } else { to };
                positions = longer.iter().map(|point| point[0]).collect();
            }
            for (point, x) in points.iter_mut().zip(&positions) {
                *point = [*x, curve_value(from, *x).blend(curve_value(to, *x), amount)];
            }
            positions.len()
        };
        let mut curve = Self {
            points,
            len: len as u32,
        };
        curve.sanitize();
        curve
    }
}

/// The points of `curve` in use.
fn curve_points(curve: &PointCurve) -> &[[f32; 2]] {
    &curve.points[..(curve.len as usize).clamp(2, MAX_POINT_CURVE_POINTS)]
}

/// The output of the polyline through `points`, sorted by input, at `x`.
fn curve_value(points: &[[f32; 2]], x: f32) -> f32 {
    let Some(next) = points.iter().position(|point| point[0] >= x) else {
        return points.last().map_or(x, |point| point[1]);
    };
    if next == 0 {
        return points[0][1];
    }
    let [x0, y0] = points[next - 1];
    let [x1, y1] = points[next];
    if x1 - x0 <= f32::EPSILON {
        y1
    } else {
        y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    }
}

impl Blend for ColorGradeWheel {
    /// Hue and saturation move as a point on the colour wheel, so a fade
    /// between two hues passes through neither the hues between them nor
    /// a wrap around the circle.
    fn blend(self, target: Self, amount: f32) -> Self {
        if self == target || amount <= 0.0 {
            return self;
        }
        if amount >= 1.0 {
            return target;
        }
        let point = |wheel: Self| {
            let angle = wheel.hue.to_radians();
            [
                wheel.saturation * angle.cos(),
                wheel.saturation * angle.sin(),
            ]
        };
        let [x, y] = point(self).blend(point(target), amount);
        let saturation = x.hypot(y);
        let hue = if saturation <= 1e-6 {
            target.hue
        } else {
            y.atan2(x).to_degrees().rem_euclid(360.0)
        };
        Self {
            hue,
            saturation,
            luminance: self.luminance.blend(target.luminance, amount),
        }
    }
}

impl Blend for ColorGrading {
    fn blend(self, target: Self, amount: f32) -> Self {
        Self {
            shadows: self.shadows.blend(target.shadows, amount),
            midtones: self.midtones.blend(target.midtones, amount),
            highlights: self.highlights.blend(target.highlights, amount),
            global: self.global.blend(target.global, amount),
            blending: self.blending.blend(target.blending, amount),
            balance: self.balance.blend(target.balance, amount),
        }
    }
}

impl Blend for PointColorRange {
    fn blend(self, target: Self, amount: f32) -> Self {
        let [min, inner_min, inner_max, max] = [self.min, self.inner_min, self.inner_max, self.max]
            .blend(
                [target.min, target.inner_min, target.inner_max, target.max],
                amount,
            );
        Self::new(min, inner_min, inner_max, max)
    }
}

impl Blend for PointColor {
    /// The adjustment of one sampled colour; `target`'s sample.
    fn blend(self, target: Self, amount: f32) -> Self {
        Self {
            sample_hsl: target.sample_hsl,
            hue_shift: self.hue_shift.blend(target.hue_shift, amount),
            saturation_shift: self.saturation_shift.blend(target.saturation_shift, amount),
            luminance_shift: self.luminance_shift.blend(target.luminance_shift, amount),
            range: self.range.blend(target.range, amount),
            hue_range: self.hue_range.blend(target.hue_range, amount),
            saturation_range: self.saturation_range.blend(target.saturation_range, amount),
            luminance_range: self.luminance_range.blend(target.luminance_range, amount),
        }
    }
}

impl Blend for PointColors {
    /// The same sampled colours fade their adjustments. Otherwise `target`'s
    /// colours replace them, with shifts that grow from none.
    fn blend(self, target: Self, amount: f32) -> Self {
        if self == target || amount <= 0.0 {
            return self;
        }
        let same_samples = self.len() == target.len()
            && self
                .iter()
                .zip(target.iter())
                .all(|(from, to)| from.sample_hsl == to.sample_hsl);
        let mut blended = target;
        for (index, to) in blended.iter_mut().enumerate() {
            let from = if same_samples {
                self[index]
            } else {
                PointColor {
                    hue_shift: 0.0,
                    saturation_shift: 0.0,
                    luminance_shift: 0.0,
                    ..*to
                }
            };
            *to = from.blend(*to, amount);
        }
        blended
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::AdjustmentGroup;

    fn groups(groups: &[AdjustmentGroup]) -> AdjustmentGroupSet {
        groups.iter().copied().collect()
    }

    fn curve(points: &[[f32; 2]]) -> PointCurve {
        let mut curve = PointCurve::linear();
        curve.points[..points.len()].copy_from_slice(points);
        curve.len = points.len() as u32;
        curve
    }

    fn preset_edit() -> (ExposureParams, ExposureParams) {
        let before = ExposureParams {
            exposure: 0.5,
            clarity: 10.0,
            texture: 7.0,
            ..ExposureParams::default()
        };
        let mut applied = before;
        applied.exposure = 1.5;
        applied.contrast = 40.0;
        applied.highlights = -60.0;
        applied.clarity = 30.0;
        applied.hsl_hue[2] = 20.0;
        applied.tone_curve = curve(&[[0.0, 0.1], [0.5, 0.6], [1.0, 1.0]]);
        applied.color_grading.shadows = ColorGradeWheel {
            hue: 220.0,
            saturation: 30.0,
            luminance: -10.0,
        };
        (before, applied)
    }

    #[test]
    fn the_ends_give_the_edit_before_and_after_the_preset() {
        let (before, applied) = preset_edit();
        let all = AdjustmentGroupSet::ALL;
        assert_eq!(applied.faded_from(&before, all, 1.0, None), applied);
        assert_eq!(applied.faded_from(&before, all, 0.0, None), before);
    }

    #[test]
    fn a_fade_moves_continuous_settings_part_of_the_way() {
        let (before, applied) = preset_edit();
        let faded = applied.faded_from(&before, AdjustmentGroupSet::ALL, 0.25, None);
        assert_eq!(faded.exposure, 0.75);
        assert_eq!(faded.contrast, 10.0);
        assert_eq!(faded.highlights, -15.0);
        assert_eq!(faded.clarity, 15.0);
        assert_eq!(faded.hsl_hue[2], 5.0);
        // Unchanged settings keep their exact value.
        assert_eq!(faded.texture, 7.0);
        assert_eq!(faded.tone_curve.len, 3);
        assert!((faded.tone_curve.points[1][1] - 0.525).abs() < 1e-6);
        let shadows = faded.color_grading.shadows;
        assert!((shadows.saturation - 7.5).abs() < 1e-4);
        assert!((shadows.hue - 220.0).abs() < 1e-3);
        assert_eq!(shadows.luminance, -2.5);
    }

    #[test]
    fn only_the_given_groups_fade() {
        let (before, applied) = preset_edit();
        let faded = applied.faded_from(&before, groups(&[AdjustmentGroup::Effects]), 0.5, None);
        assert_eq!(faded.clarity, 20.0);
        assert_eq!(faded.exposure, applied.exposure);
        assert_eq!(faded.hsl_hue, applied.hsl_hue);
    }

    #[test]
    fn switches_and_choices_keep_the_applied_value() {
        let before = ExposureParams::default();
        let mut applied = before;
        applied.ai_denoise_enabled = true;
        applied.denoise_quality = crate::pipeline::DenoiseQuality::High;
        let faded = applied.faded_from(&before, groups(&[AdjustmentGroup::Detail]), 0.0, None);
        assert!(faded.ai_denoise_enabled);
        assert_eq!(faded.denoise_quality, applied.denoise_quality);
    }

    #[test]
    fn curves_with_other_points_fade_through_samples_of_both() {
        let from = curve(&[[0.0, 0.0], [1.0, 1.0]]);
        let to = curve(&[[0.0, 0.0], [0.25, 0.5], [1.0, 1.0]]);
        let half = from.blend(to, 0.5);
        assert_eq!(half.len, 3);
        assert_eq!(half.points[1], [0.25, 0.375]);
        assert_eq!(from.blend(to, 0.0), from);
        assert_eq!(from.blend(to, 1.0), to);
    }

    #[test]
    fn grading_hues_fade_across_the_wrap_without_passing_the_far_side() {
        let from = ColorGradeWheel {
            hue: 350.0,
            saturation: 20.0,
            luminance: 0.0,
        };
        let to = ColorGradeWheel { hue: 10.0, ..from };
        let half = from.blend(to, 0.5);
        assert!(half.hue < 1e-3 || half.hue > 360.0 - 1e-3, "{}", half.hue);
    }

    #[test]
    fn new_point_colors_fade_in_from_no_shift() {
        let mut to = PointColors::default();
        let mut color = PointColor::from_srgb([0.8, 0.2, 0.1]);
        color.hue_shift = 40.0;
        color.luminance_shift = -20.0;
        to.push(color);
        let faded = PointColors::default().blend(to, 0.5);
        assert_eq!(faded.len(), 1);
        assert_eq!(faded[0].hue_shift, 20.0);
        assert_eq!(faded[0].luminance_shift, -10.0);
        assert_eq!(faded[0].sample_hsl, color.sample_hsl);
    }
}
