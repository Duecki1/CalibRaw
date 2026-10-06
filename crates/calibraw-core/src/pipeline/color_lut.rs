//! Three-dimensional colour lookup tables (LUTs): a creative look applied to
//! the finished, display-encoded image.
//!
//! A table maps an sRGB-encoded colour in `[0, 1]` to another. It is stored at
//! the resolution it was made at (a `.cube` file's `LUT_3D_SIZE`) as 16-bit
//! values and resampled to the GPU's fixed texture size when it is rendered.
//! Outputs outside `[0, 1]` are clamped, as the display cannot show them.
//!
//! Samples are ordered with red varying fastest, then green, then blue, the
//! order of a `.cube` file and of a 3D texture's x, y and z.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub const MIN_COLOR_LUT_EDGE: u32 = 2;
/// The largest table accepted. Common `.cube` sizes are 17, 33 and 65.
pub const MAX_COLOR_LUT_EDGE: u32 = 65;
pub const MAX_COLOR_LUT_NAME_CHARS: usize = 64;
/// A 65-point `.cube` file is about 8 MB of text.
pub const MAX_CUBE_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// The strength of a freshly imported look, in percent.
pub const FULL_COLOR_LUT_AMOUNT: f32 = 100.0;

#[derive(Debug)]
pub enum ColorLutError {
    Io(std::io::Error),
    Invalid(String),
    Unsupported(String),
    TooLarge(u64),
}

impl fmt::Display for ColorLutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Invalid(message) | Self::Unsupported(message) => formatter.write_str(message),
            Self::TooLarge(bytes) => write!(
                formatter,
                "the file is {bytes} bytes; the limit is {MAX_CUBE_FILE_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for ColorLutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ColorLutError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ColorLutError> {
    Err(ColorLutError::Invalid(message.into()))
}

/// A cube of output colours indexed by input colour.
#[derive(Clone, Debug)]
pub struct ColorLut {
    edge: u32,
    /// The input range the table's first and last points cover, per channel.
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    /// `edge³` RGB triples as 16-bit values, red fastest.
    samples: Arc<[u16]>,
}

impl PartialEq for ColorLut {
    fn eq(&self, other: &Self) -> bool {
        self.edge == other.edge
            && self.domain_min == other.domain_min
            && self.domain_max == other.domain_max
            && (Arc::ptr_eq(&self.samples, &other.samples) || self.samples == other.samples)
    }
}

impl ColorLut {
    /// Builds a table from `samples` (`edge³` RGB triples, red fastest).
    pub fn new(
        edge: u32,
        domain_min: [f32; 3],
        domain_max: [f32; 3],
        samples: Vec<u16>,
    ) -> Result<Self, ColorLutError> {
        if !(MIN_COLOR_LUT_EDGE..=MAX_COLOR_LUT_EDGE).contains(&edge) {
            return invalid(format!(
                "a LUT needs between {MIN_COLOR_LUT_EDGE} and {MAX_COLOR_LUT_EDGE} points per axis, not {edge}"
            ));
        }
        let expected = (edge as usize).pow(3) * 3;
        if samples.len() != expected {
            return invalid(format!(
                "a {edge}-point LUT needs {expected} values, not {}",
                samples.len()
            ));
        }
        let finite_range = (0..3).all(|channel| {
            domain_min[channel].is_finite()
                && domain_max[channel].is_finite()
                && domain_max[channel] > domain_min[channel]
        });
        if !finite_range {
            return invalid("the LUT's input range is not valid");
        }
        Ok(Self {
            edge,
            domain_min,
            domain_max,
            samples: samples.into(),
        })
    }

    /// Builds a `[0, 1]` table by evaluating `map` at each of its points.
    pub fn from_function(
        edge: u32,
        map: impl Fn([f32; 3]) -> [f32; 3],
    ) -> Result<Self, ColorLutError> {
        if !(MIN_COLOR_LUT_EDGE..=MAX_COLOR_LUT_EDGE).contains(&edge) {
            return invalid(format!("a LUT cannot have {edge} points per axis"));
        }
        let last = (edge - 1) as f32;
        let mut samples = Vec::with_capacity((edge as usize).pow(3) * 3);
        for blue in 0..edge {
            for green in 0..edge {
                for red in 0..edge {
                    let output = map([red as f32 / last, green as f32 / last, blue as f32 / last]);
                    samples.extend(output.map(quantize));
                }
            }
        }
        Self::new(edge, [0.0; 3], [1.0; 3], samples)
    }

    pub fn edge(&self) -> u32 {
        self.edge
    }

    /// The table's output for `rgb`, an sRGB-encoded colour. Linear
    /// interpolation between points, as the GPU does.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let last = (self.edge - 1) as f32;
        let mut lower = [0usize; 3];
        let mut upper = [0usize; 3];
        let mut fraction = [0.0f32; 3];
        for channel in 0..3 {
            let span = self.domain_max[channel] - self.domain_min[channel];
            let position =
                ((rgb[channel] - self.domain_min[channel]) / span).clamp(0.0, 1.0) * last;
            let floor = position.floor().min(last - 1.0).max(0.0);
            lower[channel] = floor as usize;
            upper[channel] = floor as usize + 1;
            fraction[channel] = position - floor;
        }
        let edge = self.edge as usize;
        let at = |red: usize, green: usize, blue: usize, channel: usize| {
            f32::from(self.samples[((blue * edge + green) * edge + red) * 3 + channel]) / 65535.0
        };
        let mut output = [0.0f32; 3];
        for (channel, value) in output.iter_mut().enumerate() {
            let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let plane = |blue: usize| {
                let green_low = mix(
                    at(lower[0], lower[1], blue, channel),
                    at(upper[0], lower[1], blue, channel),
                    fraction[0],
                );
                let green_high = mix(
                    at(lower[0], upper[1], blue, channel),
                    at(upper[0], upper[1], blue, channel),
                    fraction[0],
                );
                mix(green_low, green_high, fraction[1])
            };
            *value = mix(plane(lower[2]), plane(upper[2]), fraction[2]);
        }
        output
    }

    /// The table resampled to `edge` points per axis as half-float RGBA bits
    /// (alpha 1), red fastest, ready to upload as a 3D texture. Points of the
    /// new grid sit at input values `i / (edge - 1)`.
    pub fn gpu_texels(&self, edge: u32) -> Vec<u16> {
        let last = (edge.max(2) - 1) as f32;
        let one = half::f16::ONE.to_bits();
        let mut texels = Vec::with_capacity((edge as usize).pow(3) * 4);
        for blue in 0..edge {
            for green in 0..edge {
                for red in 0..edge {
                    let output =
                        self.sample([red as f32 / last, green as f32 / last, blue as f32 / last]);
                    texels.extend(output.map(|value| half::f16::from_f32(value).to_bits()));
                    texels.push(one);
                }
            }
        }
        texels
    }
}

fn quantize(value: f32) -> u16 {
    if value.is_finite() {
        (value.clamp(0.0, 1.0) * 65535.0).round() as u16
    } else {
        0
    }
}

/// A table parsed from a `.cube` file.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeFile {
    pub lut: ColorLut,
    /// The file's `TITLE`, when it has one.
    pub title: Option<String>,
}

impl CubeFile {
    /// Parses the Adobe/Resolve `.cube` text format. 1D tables are rejected.
    pub fn parse(text: &str) -> Result<Self, ColorLutError> {
        let mut edge = None;
        let mut title = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut values: Vec<f32> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let mut words = line.split_whitespace();
            let keyword = words.next().unwrap_or_default();
            if keyword
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
            {
                let floats = |words: std::str::SplitWhitespace<'_>, count: usize| {
                    let parsed: Vec<f32> = words.filter_map(|word| word.parse().ok()).collect();
                    (parsed.len() == count && parsed.iter().all(|value| value.is_finite()))
                        .then_some(parsed)
                };
                match keyword.to_ascii_uppercase().as_str() {
                    "TITLE" => {
                        let title_text = line[keyword.len()..].trim().trim_matches('"').trim();
                        if !title_text.is_empty() {
                            title = Some(title_text.to_owned());
                        }
                    }
                    "LUT_1D_SIZE" => {
                        return Err(ColorLutError::Unsupported(
                            "1D LUTs are not supported; use a 3D .cube file".to_owned(),
                        ))
                    }
                    "LUT_3D_SIZE" => {
                        let size: u32 = words
                            .next()
                            .and_then(|word| word.parse().ok())
                            .ok_or_else(|| {
                                ColorLutError::Invalid(format!(
                                    "line {number}: invalid LUT_3D_SIZE"
                                ))
                            })?;
                        if size > MAX_COLOR_LUT_EDGE {
                            return Err(ColorLutError::Unsupported(format!(
                                "LUTs with more than {MAX_COLOR_LUT_EDGE} points per axis are not supported (this one has {size})"
                            )));
                        }
                        if size < MIN_COLOR_LUT_EDGE {
                            return invalid(format!("line {number}: LUT_3D_SIZE is too small"));
                        }
                        edge = Some(size);
                        values.reserve((size as usize).pow(3) * 3);
                    }
                    "DOMAIN_MIN" => {
                        let parsed = floats(words, 3).ok_or_else(|| {
                            ColorLutError::Invalid(format!("line {number}: invalid DOMAIN_MIN"))
                        })?;
                        domain_min.copy_from_slice(&parsed);
                    }
                    "DOMAIN_MAX" => {
                        let parsed = floats(words, 3).ok_or_else(|| {
                            ColorLutError::Invalid(format!("line {number}: invalid DOMAIN_MAX"))
                        })?;
                        domain_max.copy_from_slice(&parsed);
                    }
                    "LUT_3D_INPUT_RANGE" => {
                        let parsed = floats(words, 2).ok_or_else(|| {
                            ColorLutError::Invalid(format!(
                                "line {number}: invalid LUT_3D_INPUT_RANGE"
                            ))
                        })?;
                        domain_min = [parsed[0]; 3];
                        domain_max = [parsed[1]; 3];
                    }
                    // Other keywords (video-range hints, vendor extensions) do
                    // not change how the table is read.
                    _ => {}
                }
                continue;
            }

            let Some(edge) = edge else {
                return invalid(format!("line {number}: LUT data before LUT_3D_SIZE"));
            };
            let expected = (edge as usize).pow(3) * 3;
            let mut count = 0;
            for word in line.split_whitespace() {
                let value: f32 = word.parse().map_err(|_| {
                    ColorLutError::Invalid(format!("line {number}: “{word}” is not a number"))
                })?;
                if !value.is_finite() {
                    return invalid(format!("line {number}: “{word}” is not a finite number"));
                }
                values.push(value);
                count += 1;
            }
            if count != 3 {
                return invalid(format!(
                    "line {number}: expected three values, found {count}"
                ));
            }
            if values.len() > expected {
                return invalid(format!(
                    "line {number}: more data than a {edge}-point LUT holds"
                ));
            }
        }

        let Some(edge) = edge else {
            return invalid("not a 3D .cube file: LUT_3D_SIZE is missing");
        };
        let expected = (edge as usize).pow(3) * 3;
        if values.len() != expected {
            return invalid(format!(
                "a {edge}-point LUT needs {} colours, but the file has {}",
                expected / 3,
                values.len() / 3
            ));
        }
        let samples = values.into_iter().map(quantize).collect();
        Ok(Self {
            lut: ColorLut::new(edge, domain_min, domain_max, samples)?,
            title,
        })
    }
}

/// A look applied to a photo: a table, a name for the interface, and how much
/// of it to apply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorLutEdit {
    pub name: String,
    pub lut: Arc<ColorLut>,
    /// Percent of the look mixed into the image, `0..=100`.
    #[serde(default = "full_amount")]
    pub amount: f32,
}

fn full_amount() -> f32 {
    FULL_COLOR_LUT_AMOUNT
}

impl ColorLutEdit {
    /// A look at full strength. The name is cleaned up for display.
    pub fn new(name: &str, lut: ColorLut) -> Self {
        Self {
            name: clean_name(name),
            lut: Arc::new(lut),
            amount: FULL_COLOR_LUT_AMOUNT,
        }
    }

    /// Reads a `.cube` file. The look is named after the file's `TITLE`, or
    /// after the file.
    pub fn from_cube_file(path: &Path) -> Result<Self, ColorLutError> {
        let file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        if length > MAX_CUBE_FILE_BYTES {
            return Err(ColorLutError::TooLarge(length));
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(MAX_CUBE_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let cube = CubeFile::parse(text.trim_start_matches('\u{feff}'))?;
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::new(cube.title.as_deref().unwrap_or(&stem), cube.lut))
    }

    /// The amount as a `0..=1` mix factor, 0 for a damaged value.
    pub fn mix(&self) -> f32 {
        if self.amount.is_finite() {
            (self.amount / 100.0).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Whether the name and amount are acceptable in a saved edit.
    pub fn validate(&self) -> Result<(), ColorLutError> {
        if self.name.chars().count() > MAX_COLOR_LUT_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            return invalid("the LUT name is too long or contains control characters");
        }
        if !self.amount.is_finite() || !(0.0..=100.0).contains(&self.amount) {
            return invalid("the LUT amount must be between 0 and 100");
        }
        Ok(())
    }
}

fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let cleaned: String = cleaned
        .trim()
        .chars()
        .take(MAX_COLOR_LUT_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "Colour LUT".to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[derive(Deserialize, Serialize)]
struct ColorLutDocument {
    edge: u32,
    #[serde(default = "default_domain_min", skip_serializing_if = "is_default_min")]
    domain_min: [f32; 3],
    #[serde(default = "default_domain_max", skip_serializing_if = "is_default_max")]
    domain_max: [f32; 3],
    /// Little-endian 16-bit samples.
    #[serde(with = "crate::base64_arc_bytes")]
    samples: Arc<[u8]>,
}

fn default_domain_min() -> [f32; 3] {
    [0.0; 3]
}

fn default_domain_max() -> [f32; 3] {
    [1.0; 3]
}

fn is_default_min(value: &[f32; 3]) -> bool {
    *value == default_domain_min()
}

fn is_default_max(value: &[f32; 3]) -> bool {
    *value == default_domain_max()
}

impl Serialize for ColorLut {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = self
            .samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        ColorLutDocument {
            edge: self.edge,
            domain_min: self.domain_min,
            domain_max: self.domain_max,
            samples: bytes.into(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ColorLut {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let document = ColorLutDocument::deserialize(deserializer)?;
        if document.samples.len() % 2 != 0 {
            return Err(serde::de::Error::custom(
                "LUT samples are not 16-bit values",
            ));
        }
        let samples = document
            .samples
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Self::new(
            document.edge,
            document.domain_min,
            document.domain_max,
            samples,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
