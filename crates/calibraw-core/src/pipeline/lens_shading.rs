//! `LensShadingMap`: lens vignetting gains the GPU applies to a sensor mosaic
//! after highlight reconstruction.

use std::sync::Arc;

/// Lens vignetting correction of a sensor mosaic as linear R, G, B gains on a
/// fixed [`Self::EDGE`]×[`Self::EDGE`] grid over the whole photo.
///
/// The gains are not baked into the mosaic. Highlight reconstruction applies
/// them to its output, so clip detection and opposed-highlight chroma see the
/// recorded sensor values, and brightened corners are neither taken for
/// clipped samples nor limited to the 16-bit mosaic range.
///
/// Node `(i, j)` sits at normalized position `(i, j) / (EDGE - 1)` of the
/// photo's pixel-centre extent: pixel `x` of a `W`-pixel-wide photo is at
/// `x / (W - 1)`. Crops, proxies and tiles of the photo therefore share one
/// map; the GPU places each with its tile origin and full-image extent.
#[derive(Clone, Debug, PartialEq)]
pub struct LensShadingMap {
    /// Row-major from the top-left node.
    gains: Arc<[[f32; 3]]>,
}

impl LensShadingMap {
    /// Nodes along each axis. Vignetting varies smoothly, so bilinear
    /// interpolation between nodes stays within about 0.02% of a 2-stop
    /// radial falloff.
    pub const EDGE: u32 = 129;

    /// Samples `gain` at every node, given the node's normalized position.
    /// Returns `None` unless every gain is finite and non-negative.
    pub fn from_fn(gain: impl Fn([f32; 2]) -> [f32; 3]) -> Option<Self> {
        let last = (Self::EDGE - 1) as f32;
        let gains: Arc<[[f32; 3]]> = (0..Self::EDGE)
            .flat_map(|row| (0..Self::EDGE).map(move |column| [column, row]))
            .map(|[column, row]| gain([column as f32 / last, row as f32 / last]))
            .collect();
        gains
            .iter()
            .flatten()
            .all(|value| value.is_finite() && *value >= 0.0)
            .then_some(Self { gains })
    }

    /// The node gains, row-major from the top-left node.
    pub fn gains(&self) -> &[[f32; 3]] {
        &self.gains
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_span_the_photo_corner_to_corner() {
        let map = LensShadingMap::from_fn(|[u, v]| [u, v, 1.0]).unwrap();
        let edge = LensShadingMap::EDGE as usize;
        assert_eq!(map.gains().len(), edge * edge);
        assert_eq!(map.gains()[0], [0.0, 0.0, 1.0]);
        assert_eq!(map.gains()[edge - 1], [1.0, 0.0, 1.0]);
        assert_eq!(map.gains()[edge * edge - 1], [1.0, 1.0, 1.0]);
    }

    #[test]
    fn invalid_gains_are_refused() {
        assert!(LensShadingMap::from_fn(|_| [1.0, f32::NAN, 1.0]).is_none());
        assert!(LensShadingMap::from_fn(|_| [1.0, -0.5, 1.0]).is_none());
    }
}
