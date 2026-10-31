//! Carousel exports: a stencil on the output, cut into equal slides, one file each.
//!
//! The stencil sits on the photo's output frame, after crop, rotation and quarter turns, and picks
//! a window of the output raster. The photo's geometry stays as edited, so crop-relative effects
//! such as the vignette render as they do in a normal export. The window is sharpened as one image
//! and only then cut ([`SlideWriter`]), so neighbouring slides continue each other pixel for pixel.
//! A watermark is placed on every slide, each as its own frame.

use super::*;
use std::ops::RangeInclusive;

/// The shape of one slide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarouselRatio {
    #[default]
    FourFive,
    ThreeFour,
    Square,
    /// Width and height of a slide, each in `1..=MAX_TERM`.
    Custom {
        width: u16,
        height: u16,
    },
}

impl CarouselRatio {
    /// The largest term of a custom ratio. Slides are whole multiples of their ratio, so this
    /// keeps the smallest possible stencil small enough for small photos.
    pub const MAX_TERM: u16 = 99;

    /// Width and height of a slide, in their lowest terms.
    fn lowest_terms(self) -> (u32, u32) {
        let (width, height) = self.terms();
        let (width, height) = (u32::from(width), u32::from(height));
        let (mut divisor, mut remainder) = (width, height);
        while remainder != 0 {
            (divisor, remainder) = (remainder, divisor % remainder);
        }
        (width / divisor, height / divisor)
    }

    /// The ratio with these terms: a preset when one matches.
    pub fn from_terms(width: u16, height: u16) -> Self {
        match (width, height) {
            (4, 5) => Self::FourFive,
            (3, 4) => Self::ThreeFour,
            (1, 1) => Self::Square,
            _ => Self::Custom { width, height },
        }
    }

    /// Width and height of a slide; custom terms are clamped to `1..=MAX_TERM`, as saved settings
    /// may hold any.
    pub fn terms(self) -> (u16, u16) {
        match self {
            Self::FourFive => (4, 5),
            Self::ThreeFour => (3, 4),
            Self::Square => (1, 1),
            Self::Custom { width, height } => (
                width.clamp(1, Self::MAX_TERM),
                height.clamp(1, Self::MAX_TERM),
            ),
        }
    }
}

/// How many slides a carousel has, their shape and where its stencil sits on the output frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Carousel {
    slides: u8,
    ratio: CarouselRatio,
    size: f32,
    center: [f32; 2],
}

impl Carousel {
    pub const SLIDES: RangeInclusive<u8> = 2..=6;
    pub const DEFAULT_SLIDES: u8 = 3;
    /// The stencil's size, in percent of the largest that fits the output frame.
    pub const SIZE_RANGE: RangeInclusive<f32> = 25.0..=100.0;
    pub const LARGEST_SIZE: f32 = 100.0;

    /// The largest stencil that fits, centred.
    pub fn new(slides: u8, ratio: CarouselRatio) -> Self {
        Self {
            slides: slides.clamp(*Self::SLIDES.start(), *Self::SLIDES.end()),
            ratio,
            size: Self::LARGEST_SIZE,
            center: [0.5; 2],
        }
    }

    pub fn slides(self) -> u8 {
        self.slides
    }

    pub fn ratio(self) -> CarouselRatio {
        self.ratio
    }

    pub fn size(self) -> f32 {
        self.size
    }

    /// The same stencil shape, largest and centred again.
    pub fn recentered(self) -> Self {
        Self::new(self.slides, self.ratio)
    }

    pub fn with_slides(self, slides: u8) -> Self {
        Self {
            slides: slides.clamp(*Self::SLIDES.start(), *Self::SLIDES.end()),
            ..self
        }
    }

    pub fn with_ratio(self, ratio: CarouselRatio) -> Self {
        Self { ratio, ..self }
    }

    /// Keeps the size within [`Self::SIZE_RANGE`]; a non-finite size is the largest.
    pub fn with_size(self, size: f32) -> Self {
        let size = if size.is_finite() {
            size.clamp(*Self::SIZE_RANGE.start(), *Self::SIZE_RANGE.end())
        } else {
            Self::LARGEST_SIZE
        };
        Self { size, ..self }
    }

    /// Moves the stencil's centre, 0–1 across the output frame. [`Self::layout`] keeps the
    /// stencil inside the frame, so a centre too near an edge stops it at that edge.
    pub fn with_center(self, center: [f32; 2]) -> Self {
        Self {
            center: center.map(|axis| {
                if axis.is_finite() {
                    axis.clamp(0.0, 1.0)
                } else {
                    0.5
                }
            }),
            ..self
        }
    }

    /// Where the stencil lands for a photo with `geometry`, and what each slide measures once
    /// `settings` resize it. Fails when the photo is too small for the ratio or the result
    /// exceeds the export limits.
    pub fn layout(
        self,
        geometry: GeometryTransform,
        source_width: u32,
        source_height: u32,
        settings: &ExportSettings,
    ) -> Result<CarouselLayout> {
        let (frame_width, frame_height) = geometry
            .sanitized()
            .crop_pixel_dimensions(source_width, source_height);
        let slides = u32::from(self.slides);
        let (ratio_width, ratio_height) = self.ratio.lowest_terms();

        // Slides are whole multiples of the ratio, so each has exactly that shape.
        let largest_step = (frame_width / (slides * ratio_width)).min(frame_height / ratio_height);
        let step = (f64::from(largest_step) * f64::from(self.size) / 100.0) as u32;
        anyhow::ensure!(step > 0, "the photo is too small for this carousel");
        let stencil_width = slides * ratio_width * step;
        let stencil_height = ratio_height * step;
        let stencil_left = stencil_start(self.center[0], frame_width, stencil_width);
        let stencil_top = stencil_start(self.center[1], frame_height, stencil_height);

        // Resize settings apply to one slide, as that is the file they describe.
        let (resized_width, resized_height) =
            settings.output_dimensions(ratio_width * step, ratio_height * step);
        let output_step = (resized_width / ratio_width)
            .min(resized_height / ratio_height)
            .max(1);
        let width = slides * ratio_width * output_step;
        let height = ratio_height * output_step;
        validate_export_dimensions(width, height)?;

        let scale = f64::from(output_step) / f64::from(step);
        let scaled = |pixels: u32| (f64::from(pixels) * scale).round() as u32;
        let (raster_width, raster_height) = (scaled(frame_width), scaled(frame_height));
        validate_export_dimensions(raster_width, raster_height)?;

        let [frame_width, frame_height] = [frame_width, frame_height].map(|edge| edge as f32);
        Ok(CarouselLayout {
            frame_uv: [
                stencil_left as f32 / frame_width,
                stencil_top as f32 / frame_height,
                (stencil_left + stencil_width) as f32 / frame_width,
                (stencil_top + stencil_height) as f32 / frame_height,
            ],
            slides: self.slides,
            window: OutputWindow {
                raster_width,
                raster_height,
                left: scaled(stencil_left).min(raster_width.saturating_sub(width)),
                top: scaled(stencil_top).min(raster_height.saturating_sub(height)),
                width,
                height,
            },
        })
    }
}

impl Default for Carousel {
    fn default() -> Self {
        Self::new(Self::DEFAULT_SLIDES, CarouselRatio::default())
    }
}

/// The first pixel of a stencil of `stencil` pixels centred on `center` (0–1 across `frame`
/// pixels), kept inside the frame.
fn stencil_start(center: f32, frame: u32, stencil: u32) -> u32 {
    let start = f64::from(center) * f64::from(frame) - f64::from(stencil) * 0.5;
    (start.round().max(0.0) as u32).min(frame.saturating_sub(stencil))
}

/// A [`Carousel`] resolved for one photo and its export settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarouselLayout {
    frame_uv: [f32; 4],
    slides: u8,
    window: OutputWindow,
}

impl CarouselLayout {
    /// The stencil's left, top, right and bottom edges, 0–1 across the output frame.
    pub fn frame_uv(&self) -> [f32; 4] {
        self.frame_uv
    }

    pub fn slides(&self) -> u8 {
        self.slides
    }

    /// Width and height of each slide's file, in pixels.
    pub fn slide_size(&self) -> (u32, u32) {
        (
            self.window.width / u32::from(self.slides),
            self.window.height,
        )
    }

    pub(super) fn window(&self) -> OutputWindow {
        self.window
    }
}

/// The part of the output raster an export writes. The geometry maps the photo onto the whole
/// raster, so a window's pixels are those a whole export at that size would have there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OutputWindow {
    pub(super) raster_width: u32,
    pub(super) raster_height: u32,
    pub(super) left: u32,
    pub(super) top: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl OutputWindow {
    pub(super) const fn whole(width: u32, height: u32) -> Self {
        Self {
            raster_width: width,
            raster_height: height,
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    pub(super) fn is_whole(self) -> bool {
        self == Self::whole(self.raster_width, self.raster_height)
    }
}
