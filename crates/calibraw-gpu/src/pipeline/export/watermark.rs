//! A watermark image blended over the finished export.
//!
//! The watermark is placed in the output frame (after crop, rotation and
//! resize), or on each slide of a carousel, and blended into the final linear
//! Rec.2020 rows after output sharpening and before encoding, so it is never
//! sharpened and every format and bit depth receives the same pixels.

use super::*;
use calibraw_core::color_math::{linear_srgb_to_rec2020, srgb_decode};
use std::ops::RangeInclusive;

/// A watermark image: sRGB-encoded RGBA8 with straight alpha, row-major.
pub struct WatermarkImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl WatermarkImage {
    /// The longest edge kept; [`Self::decode`] reduces larger images.
    const MAX_EDGE: u32 = 4096;

    fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        anyhow::ensure!(width > 0 && height > 0, "the watermark image is empty");
        anyhow::ensure!(
            width.max(height) <= Self::MAX_EDGE,
            "the watermark image is larger than {} pixels",
            Self::MAX_EDGE
        );
        let expected = u64::from(width) * u64::from(height) * 4;
        anyhow::ensure!(
            u64::try_from(rgba.len()).unwrap_or(u64::MAX) == expected,
            "watermark pixels do not match the image size"
        );
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Decodes a PNG, JPEG or TIFF file, reduced to [`Self::MAX_EDGE`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut image = image::load_from_memory(bytes)
            .context("the watermark is not a readable PNG, JPEG or TIFF image")?;
        if image.width().max(image.height()) > Self::MAX_EDGE {
            image = image.resize(
                Self::MAX_EDGE,
                Self::MAX_EDGE,
                image::imageops::FilterType::Lanczos3,
            );
        }
        let rgba = image.into_rgba8();
        Self::new(rgba.width(), rgba.height(), rgba.into_raw())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

impl std::fmt::Debug for WatermarkImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WatermarkImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// The side or corner of the output a watermark is placed against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WatermarkAnchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

impl WatermarkAnchor {
    /// Row by row, from the top left.
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top left",
            Self::Top => "Top",
            Self::TopRight => "Top right",
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
            Self::BottomLeft => "Bottom left",
            Self::Bottom => "Bottom",
            Self::BottomRight => "Bottom right",
        }
    }

    /// Horizontal and vertical alignment: 0 against the left or top edge,
    /// 0.5 centered, 1 against the right or bottom edge.
    const fn alignment(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::Top => [0.5, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::Left => [0.0, 0.5],
            Self::Center => [0.5, 0.5],
            Self::Right => [1.0, 0.5],
            Self::BottomLeft => [0.0, 1.0],
            Self::Bottom => [0.5, 1.0],
            Self::BottomRight => [1.0, 1.0],
        }
    }
}

/// Where a watermark sits in the output frame: against a side or corner
/// ([`WatermarkAnchor`]), a margin away from the edges it touches. The size is
/// the watermark's longer side and the margin a distance, both as a
/// percentage of the output's shorter edge, so the watermark looks the same
/// on portrait, landscape and square photos.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WatermarkPlacement {
    anchor: WatermarkAnchor,
    margin: f32,
    size: f32,
}

impl WatermarkPlacement {
    pub const SIZE_RANGE: RangeInclusive<f32> = 2.0..=100.0;
    pub const DEFAULT_SIZE: f32 = 20.0;
    pub const MARGIN_RANGE: RangeInclusive<f32> = 0.0..=25.0;
    pub const DEFAULT_MARGIN: f32 = 3.0;

    /// Keeps the margin and size within their ranges; non-finite values take
    /// the defaults.
    pub fn new(anchor: WatermarkAnchor, margin: f32, size: f32) -> Self {
        let within = |value: f32, range: RangeInclusive<f32>, default: f32| {
            if value.is_finite() {
                value.clamp(*range.start(), *range.end())
            } else {
                default
            }
        };
        Self {
            anchor,
            margin: within(margin, Self::MARGIN_RANGE, Self::DEFAULT_MARGIN),
            size: within(size, Self::SIZE_RANGE, Self::DEFAULT_SIZE),
        }
    }

    pub fn anchor(self) -> WatermarkAnchor {
        self.anchor
    }

    pub fn margin(self) -> f32 {
        self.margin
    }

    pub fn size(self) -> f32 {
        self.size
    }

    /// The watermark's left and top edges, width and height for an image of
    /// `image` pixels, in the units of `frame`, the output's width and
    /// height. A centered axis ignores the margin. A watermark wider than the
    /// space between the margins extends past the edge it is not anchored to.
    pub fn rect(self, image: [u32; 2], frame: [f32; 2]) -> [f32; 4] {
        let shorter_edge = frame[0].min(frame[1]);
        let longer_side = shorter_edge * self.size / 100.0;
        let margin = shorter_edge * self.margin / 100.0;
        let [width, height] = image.map(|edge| edge.max(1) as f32);
        let extent = if width >= height {
            [longer_side, longer_side * height / width]
        } else {
            [longer_side * width / height, longer_side]
        };
        let alignment = self.anchor.alignment();
        let origin =
            |axis: usize| margin + (frame[axis] - 2.0 * margin - extent[axis]) * alignment[axis];
        [origin(0), origin(1), extent[0], extent[1]]
    }
}

impl Default for WatermarkPlacement {
    fn default() -> Self {
        Self::new(
            WatermarkAnchor::default(),
            Self::DEFAULT_MARGIN,
            Self::DEFAULT_SIZE,
        )
    }
}

/// A watermark drawn over an export. The image is shared by every item of a
/// batch.
#[derive(Clone, Debug)]
pub struct ExportWatermark {
    pub image: Arc<WatermarkImage>,
    pub placement: WatermarkPlacement,
}

/// Images are immutable once loaded, so the same `Arc` means the same image;
/// comparing by identity avoids comparing millions of pixels whenever export
/// settings are compared.
impl PartialEq for ExportWatermark {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image) && self.placement == other.placement
    }
}

/// A watermark resolved for one slide size: its pixel rectangle and the
/// Lanczos weights that resample the image into it. Rows are resampled as
/// they are blended, so no resampled copy of the image is held.
pub(super) struct PreparedWatermark {
    image: Arc<WatermarkImage>,
    /// Values of one slide's part of an output row; a normal export is one slide.
    slide_row_values: usize,
    /// The watermark's top-left output pixel; it may lie outside the output.
    left: i64,
    top: i64,
    /// Source columns and rows for each watermark column and row.
    columns: Vec<Vec<SampleWeight>>,
    rows: Vec<Vec<SampleWeight>>,
    /// Linear value of each sRGB code value.
    linear: [f32; 256],
}

impl PreparedWatermark {
    pub(super) fn for_request(request: &ExportRequest<'_>) -> Result<Option<Self>> {
        request
            .watermark
            .map(|watermark| Self::new(watermark, request.slide_width(), request.window.height))
            .transpose()
    }

    fn new(watermark: &ExportWatermark, output_width: u32, output_height: u32) -> Result<Self> {
        let image = &watermark.image;
        let [left, top, width, height] = watermark.placement.rect(
            [image.width, image.height],
            [output_width as f32, output_height as f32],
        );
        let [left, top] = [left, top].map(|edge| edge.round() as i64);
        let [width, height] = [width, height].map(|edge| (edge.round() as u32).max(1));
        calibraw_core::diagnostics::record(format!(
            "Export watermark: source={}x{} placed={width}x{height} at ({left}, {top}) in {output_width}x{output_height}",
            image.width, image.height,
        ));
        Ok(Self {
            image: Arc::clone(image),
            slide_row_values: output_width.max(1) as usize * 3,
            left,
            top,
            columns: build_lanczos_contributions(image.width, width)?,
            rows: build_lanczos_contributions(image.height, height)?,
            linear: std::array::from_fn(|code| srgb_decode(code as f32 / 255.0)),
        })
    }

    /// Blends the watermark over output row `y` on every slide, each clipped
    /// to its own slide.
    pub(super) fn blend_row(&self, y: u32, row: &mut [f32]) {
        for slide_row in row.chunks_exact_mut(self.slide_row_values) {
            self.blend_slide_row(y, slide_row);
        }
    }

    /// Blends the watermark over row `y` of one slide, linear Rec.2020 RGB,
    /// with premultiplied "over" compositing in linear light.
    fn blend_slide_row(&self, y: u32, row: &mut [f32]) {
        let Some(weights) = usize::try_from(i64::from(y) - self.top)
            .ok()
            .and_then(|local_y| self.rows.get(local_y))
        else {
            return;
        };
        let output_width = (row.len() / 3) as i64;
        // Watermark columns that land inside the output.
        let first = (-self.left).clamp(0, self.columns.len() as i64) as usize;
        let last = (output_width - self.left).clamp(0, self.columns.len() as i64) as usize;
        let visible = &self.columns[first..last];
        // Contributions are in ascending source order.
        let (Some(source_first), Some(source_last)) = (
            visible.first().and_then(|samples| samples.first()),
            visible.last().and_then(|samples| samples.last()),
        ) else {
            return;
        };
        let source_first = source_first.index as usize;
        let source_last = source_last.index as usize;

        // Vertical pass over the source columns the visible part reads, in
        // premultiplied linear sRGB.
        let source_stride = self.image.width as usize * 4;
        let mut vertical = vec![[0.0f32; 4]; source_last - source_first + 1];
        for sample in weights {
            let start = sample.index as usize * source_stride;
            let source_row = &self.image.rgba[start..start + source_stride];
            let pixels = source_row[source_first * 4..(source_last + 1) * 4].chunks_exact(4);
            for (sum, pixel) in vertical.iter_mut().zip(pixels) {
                let alpha = f32::from(pixel[3]) / 255.0 * sample.weight;
                for channel in 0..3 {
                    sum[channel] += self.linear[usize::from(pixel[channel])] * alpha;
                }
                sum[3] += alpha;
            }
        }

        for (column, samples) in (first..).zip(visible) {
            let mut pixel = [0.0f32; 4];
            for sample in samples {
                let value = vertical[sample.index as usize - source_first];
                for channel in 0..4 {
                    pixel[channel] += value[channel] * sample.weight;
                }
            }
            // Lanczos rings at hard edges; keep a valid premultiplied pixel.
            let alpha = pixel[3].clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let color = linear_srgb_to_rec2020([
                pixel[0].clamp(0.0, alpha),
                pixel[1].clamp(0.0, alpha),
                pixel[2].clamp(0.0, alpha),
            ]);
            let start = (self.left + column as i64) as usize * 3;
            for channel in 0..3 {
                row[start + channel] = color[channel] + row[start + channel] * (1.0 - alpha);
            }
        }
    }
}
