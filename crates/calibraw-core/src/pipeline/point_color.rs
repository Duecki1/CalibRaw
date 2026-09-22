use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::{Index, IndexMut};

pub const MAX_POINT_COLORS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointColorRange {
    pub min: f32,
    pub inner_min: f32,
    pub inner_max: f32,
    pub max: f32,
}

impl PointColorRange {
    pub const fn new(min: f32, inner_min: f32, inner_max: f32, max: f32) -> Self {
        Self {
            min,
            inner_min,
            inner_max,
            max,
        }
    }

    /// A soft range covering the complete channel domain.
    pub const fn full() -> Self {
        Self::new(-1.0, -0.25, 0.25, 1.0)
    }

    pub fn weight(&self, offset: f32) -> f32 {
        if !offset.is_finite() {
            return 0.0;
        }
        if (self.inner_min..=self.inner_max).contains(&offset) {
            return 1.0;
        }
        if offset <= self.min || offset >= self.max {
            return 0.0;
        }
        if offset < self.inner_min {
            smoothstep((offset - self.min) / (self.inner_min - self.min))
        } else {
            smoothstep((self.max - offset) / (self.max - self.inner_max))
        }
    }

    pub fn sanitized(self, hue: bool) -> Self {
        let limit = if hue { 0.5 } else { 1.0 };
        let mut values = [self.min, self.inner_min, self.inner_max, self.max];
        for value in &mut values {
            *value = if value.is_finite() {
                (*value).clamp(-limit, limit)
            } else {
                0.0
            };
        }
        values.sort_by(|a, b| a.total_cmp(b));
        Self::new(values[0], values[1], values[2], values[3])
    }
}

impl Default for PointColorRange {
    fn default() -> Self {
        Self::full()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointColor {
    pub sample_hsl: [f32; 3],
    pub hue_shift: f32,
    pub saturation_shift: f32,
    pub luminance_shift: f32,
    #[serde(default = "default_range_width")]
    pub range: f32,
    #[serde(default)]
    pub hue_range: PointColorRange,
    #[serde(default)]
    pub saturation_range: PointColorRange,
    #[serde(default)]
    pub luminance_range: PointColorRange,
}

const fn default_range_width() -> f32 {
    50.0
}

impl Default for PointColor {
    fn default() -> Self {
        Self::from_srgb([0.0, 0.0, 0.0])
    }
}

impl PointColor {
    pub fn from_srgb(rgb: [f32; 3]) -> Self {
        Self {
            sample_hsl: srgb_to_hsl(rgb),
            hue_shift: 0.0,
            saturation_shift: 0.0,
            luminance_shift: 0.0,
            range: default_range_width(),
            hue_range: PointColorRange::new(-0.125, -0.0625, 0.0625, 0.125),
            saturation_range: PointColorRange::new(-0.5, -0.25, 0.25, 0.5),
            luminance_range: PointColorRange::new(-0.5, -0.25, 0.25, 0.5),
        }
    }

    pub fn sample_rgb(&self) -> [f32; 3] {
        hsl_to_srgb(self.sample_hsl)
    }

    pub fn weight_for_hsl(&self, hsl: [f32; 3]) -> f32 {
        let scale = 0.2 + 1.6 * (self.range.clamp(0.0, 100.0) / 100.0);
        let hue_offset = hsl[0] - self.sample_hsl[0];
        let hue_weight = [-1.0_f32, 0.0, 1.0]
            .into_iter()
            .map(|turn| self.hue_range.weight((hue_offset + turn) / scale))
            .fold(0.0, f32::max);
        hue_weight
            * self
                .saturation_range
                .weight((hsl[1] - self.sample_hsl[1]) / scale)
            * self
                .luminance_range
                .weight((hsl[2] - self.sample_hsl[2]) / scale)
    }

    pub fn weight_for_srgb(&self, rgb: [f32; 3]) -> f32 {
        self.weight_for_hsl(srgb_to_hsl(rgb))
    }

    pub fn sanitized(mut self) -> Self {
        for value in &mut self.sample_hsl {
            if !value.is_finite() {
                *value = 0.0;
            }
        }
        self.sample_hsl[0] = self.sample_hsl[0].rem_euclid(1.0);
        self.sample_hsl[1] = self.sample_hsl[1].clamp(0.0, 1.0);
        self.sample_hsl[2] = self.sample_hsl[2].clamp(0.0, 1.0);
        self.hue_shift = finite_clamp(self.hue_shift, -100.0, 100.0);
        self.saturation_shift = finite_clamp(self.saturation_shift, -100.0, 100.0);
        self.luminance_shift = finite_clamp(self.luminance_shift, -100.0, 100.0);
        self.range = finite_clamp(self.range, 0.0, 100.0);
        self.hue_range = self.hue_range.sanitized(true);
        self.saturation_range = self.saturation_range.sanitized(false);
        self.luminance_range = self.luminance_range.sanitized(false);
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PointColors {
    values: [PointColor; MAX_POINT_COLORS],
    len: u8,
}

impl PartialEq for PointColors {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Default for PointColors {
    fn default() -> Self {
        Self {
            values: [PointColor::default(); MAX_POINT_COLORS],
            len: 0,
        }
    }
}
impl PointColors {
    pub fn len(&self) -> usize {
        self.len as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn as_slice(&self) -> &[PointColor] {
        &self.values[..self.len()]
    }
    pub fn as_mut_slice(&mut self) -> &mut [PointColor] {
        let len = self.len();
        &mut self.values[..len]
    }
    pub fn iter(&self) -> impl Iterator<Item = &PointColor> {
        self.as_slice().iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut PointColor> {
        self.as_mut_slice().iter_mut()
    }
    pub fn get(&self, index: usize) -> Option<&PointColor> {
        self.as_slice().get(index)
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut PointColor> {
        self.as_mut_slice().get_mut(index)
    }
    pub fn push(&mut self, color: PointColor) -> bool {
        if self.len() == MAX_POINT_COLORS {
            return false;
        }
        self.values[self.len as usize] = color;
        self.len += 1;
        true
    }
    pub fn remove(&mut self, index: usize) -> Option<PointColor> {
        let len = self.len();
        if index >= len {
            return None;
        }
        let value = self.values[index];
        self.values[index..len].rotate_left(1);
        self.len -= 1;
        Some(value)
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Index<usize> for PointColors {
    type Output = PointColor;
    fn index(&self, index: usize) -> &Self::Output {
        &self.as_slice()[index]
    }
}
impl IndexMut<usize> for PointColors {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}
impl std::ops::Deref for PointColors {
    type Target = [PointColor];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}
impl std::ops::DerefMut for PointColors {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Serialize for PointColors {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for PointColors {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = Vec::<PointColor>::deserialize(deserializer)?;
        if values.len() > MAX_POINT_COLORS {
            return Err(serde::de::Error::custom("too many point colors"));
        }
        let mut result = Self::default();
        for value in values {
            result.push(value);
        }
        Ok(result)
    }
}

fn finite_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0
    }
}
fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}
fn srgb_to_hsl(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|v| finite_clamp(v, 0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= f32::EPSILON {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        (g - b) / d
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [((h / 6.0).rem_euclid(1.0)), s, l]
}

fn hsl_to_srgb(hsl: [f32; 3]) -> [f32; 3] {
    let [h, s, l] = [
        hsl[0].rem_euclid(1.0),
        hsl[1].clamp(0.0, 1.0),
        hsl[2].clamp(0.0, 1.0),
    ];
    if s <= f32::EPSILON {
        return [l; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    [
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    ]
}
fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_sample_round_trips_through_hsl() {
        for rgb in [[1.0, 0.0, 0.0], [0.1, 0.4, 0.8], [0.3, 0.3, 0.3]] {
            let color = PointColor::from_srgb(rgb);
            let round_trip = color.sample_rgb();
            for (actual, expected) in round_trip.into_iter().zip(rgb) {
                assert!((actual - expected).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn point_colors_are_bounded_and_serialize_as_a_sequence() {
        let mut colors = PointColors::default();
        for _ in 0..MAX_POINT_COLORS {
            assert!(colors.push(PointColor::default()));
        }
        assert!(!colors.push(PointColor::default()));
        let encoded = serde_json::to_value(colors).unwrap();
        assert_eq!(encoded.as_array().unwrap().len(), MAX_POINT_COLORS);
        let decoded: PointColors = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, colors);
    }

    #[test]
    fn point_color_weight_is_one_at_the_sample() {
        let color = PointColor::from_srgb([0.2, 0.6, 0.4]);
        assert_eq!(color.weight_for_hsl(color.sample_hsl), 1.0);
    }

    #[test]
    fn hue_distance_wraps_at_the_seam_and_soft_ranges_fall_off() {
        let mut color = PointColor::from_srgb([1.0, 0.0, 0.0]);
        color.hue_range = PointColorRange::new(-0.2, -0.1, 0.1, 0.2);
        assert!(color.weight_for_hsl([0.99, 1.0, 0.5]) > 0.0);
        assert!(color.hue_range.weight(0.15) < 1.0);
        assert_eq!(color.hue_range.weight(0.1), 1.0);
        assert_eq!(color.hue_range.weight(0.2), 0.0);
    }

    #[test]
    fn range_knob_changes_influence_width() {
        let mut narrow = PointColor::from_srgb([1.0, 0.0, 0.0]);
        narrow.range = 0.0;
        let mut wide = narrow;
        wide.range = 100.0;
        let hsl = [
            narrow.sample_hsl[0] + 0.1,
            narrow.sample_hsl[1],
            narrow.sample_hsl[2],
        ];
        assert!(wide.weight_for_hsl(hsl) >= narrow.weight_for_hsl(hsl));
    }

    #[test]
    fn removing_and_clearing_colors_preserves_logical_equality() {
        let mut colors = PointColors::default();
        colors.push(PointColor::default());
        let mut copy = colors;
        assert_eq!(copy.remove(0), Some(PointColor::default()));
        assert_eq!(copy, PointColors::default());
        colors.clear();
        assert_eq!(colors, PointColors::default());
    }
}
