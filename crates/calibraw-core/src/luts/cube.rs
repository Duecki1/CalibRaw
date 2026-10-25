//! Adobe/Resolve `.cube` 3D LUTs.
//!
//! Supported: `TITLE`, `LUT_3D_SIZE`, `DOMAIN_MIN`, `DOMAIN_MAX`, Resolve's
//! `LUT_3D_INPUT_RANGE`, `#` comment lines, CRLF line ends and a UTF-8 byte
//! order mark. Other keywords before the table are ignored. Files with a 1D
//! table (`LUT_1D_SIZE`, including Resolve's 1D shaper plus 3D files) are
//! refused rather than applied without their shaper.

use super::LutId;
use std::fmt;

/// Larger `.cube` files are refused before parsing. A 65-point LUT is about
/// 8 MiB of text.
pub const MAX_CUBE_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MIN_LUT_SIZE: u32 = 2;
/// The largest lattice accepted. 65 points is the common maximum of grading
/// tools; it bounds the GPU texture at 65³ texels.
pub const MAX_LUT_SIZE: u32 = 65;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CubeLutError {
    TooLarge(u64),
    MissingSize,
    Unsupported1d,
    SizeOutOfRange(u32),
    /// A line, counted from 1, that is neither a keyword nor three numbers.
    InvalidLine(usize),
    NonFinite(usize),
    WrongEntryCount {
        expected: usize,
        found: usize,
    },
    InvalidDomain,
}

impl fmt::Display for CubeLutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(bytes) => write!(
                formatter,
                "the LUT file is {bytes} bytes; the limit is {MAX_CUBE_FILE_BYTES} bytes"
            ),
            Self::MissingSize => formatter.write_str("not a 3D .cube LUT: LUT_3D_SIZE is missing"),
            Self::Unsupported1d => formatter.write_str("1D LUTs are not supported"),
            Self::SizeOutOfRange(size) => write!(
                formatter,
                "LUT size {size} is outside the supported {MIN_LUT_SIZE}–{MAX_LUT_SIZE}"
            ),
            Self::InvalidLine(line) => write!(formatter, "line {line} of the LUT is not valid"),
            Self::NonFinite(line) => {
                write!(formatter, "line {line} of the LUT holds a non-finite value")
            }
            Self::WrongEntryCount { expected, found } => write!(
                formatter,
                "the LUT holds {found} entries; its size needs {expected}"
            ),
            Self::InvalidDomain => {
                formatter.write_str("the LUT domain minimum must be below its maximum")
            }
        }
    }
}

impl std::error::Error for CubeLutError {}

/// A parsed 3D LUT.
///
/// `table` holds `size³` output colours with the red index changing fastest,
/// as `.cube` files list them. Inputs are mapped from
/// `domain_min..=domain_max` onto the lattice; outputs are stored as written.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeLut {
    id: LutId,
    title: Option<String>,
    size: u32,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    table: Box<[[f32; 3]]>,
}

impl CubeLut {
    pub fn id(&self) -> LutId {
        self.id
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Lattice points per axis.
    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn domain_min(&self) -> [f32; 3] {
        self.domain_min
    }

    pub fn domain_max(&self) -> [f32; 3] {
        self.domain_max
    }

    /// `size³` entries, red index fastest, then green, then blue.
    pub fn table(&self) -> &[[f32; 3]] {
        &self.table
    }

    /// Maps `rgb` through the LUT with tetrahedral interpolation, the reference
    /// the GPU pass is tested against. Inputs outside the domain are clamped
    /// to it.
    pub fn sample_tetrahedral(&self, rgb: [f32; 3]) -> [f32; 3] {
        let last = (self.size - 1) as f32;
        let mut base = [0_usize; 3];
        let mut fraction = [0.0_f32; 3];
        for channel in 0..3 {
            let range = self.domain_max[channel] - self.domain_min[channel];
            let position =
                ((rgb[channel] - self.domain_min[channel]) / range).clamp(0.0, 1.0) * last;
            // The upper cell corner must stay on the lattice, so 1.0 falls in
            // the last cell with a fraction of 1.
            let cell = position.floor().min(last - 1.0);
            base[channel] = cell as usize;
            fraction[channel] = position - cell;
        }
        let corner =
            |r: usize, g: usize, b: usize| self.entry(base[0] + r, base[1] + g, base[2] + b);
        let [fr, fg, fb] = fraction;
        // Each ordering of the fractions selects one of the six tetrahedra
        // that split the cell along its black-to-white diagonal.
        let (first, second, third, weights) = if fr > fg {
            if fg > fb {
                (
                    corner(1, 0, 0),
                    corner(1, 1, 0),
                    corner(1, 1, 1),
                    [fr, fg, fb],
                )
            } else if fr > fb {
                (
                    corner(1, 0, 0),
                    corner(1, 0, 1),
                    corner(1, 1, 1),
                    [fr, fb, fg],
                )
            } else {
                (
                    corner(0, 0, 1),
                    corner(1, 0, 1),
                    corner(1, 1, 1),
                    [fb, fr, fg],
                )
            }
        } else if fb > fg {
            (
                corner(0, 0, 1),
                corner(0, 1, 1),
                corner(1, 1, 1),
                [fb, fg, fr],
            )
        } else if fb > fr {
            (
                corner(0, 1, 0),
                corner(0, 1, 1),
                corner(1, 1, 1),
                [fg, fb, fr],
            )
        } else {
            (
                corner(0, 1, 0),
                corner(1, 1, 0),
                corner(1, 1, 1),
                [fg, fr, fb],
            )
        };
        let origin = corner(0, 0, 0);
        std::array::from_fn(|channel| {
            origin[channel]
                + weights[0] * (first[channel] - origin[channel])
                + weights[1] * (second[channel] - first[channel])
                + weights[2] * (third[channel] - second[channel])
        })
    }

    fn entry(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        let size = self.size as usize;
        self.table[r + size * (g + size * b)]
    }
}

/// Parses a `.cube` file. The returned LUT's id is the fingerprint of `bytes`.
pub fn parse_cube(bytes: &[u8]) -> Result<CubeLut, CubeLutError> {
    if bytes.len() as u64 > MAX_CUBE_FILE_BYTES {
        return Err(CubeLutError::TooLarge(bytes.len() as u64));
    }
    let id = LutId::of_bytes(bytes);
    let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes));

    let mut title = None;
    let mut size = None;
    let mut domain_min = [0.0_f32; 3];
    let mut domain_max = [1.0_f32; 3];
    let mut table: Vec<[f32; 3]> = Vec::new();
    let mut expected = 0_usize;

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')) {
            if size.is_none() {
                return Err(CubeLutError::MissingSize);
            }
            let entry = parse_triplet(line, number)?;
            if table.len() == expected {
                return Err(CubeLutError::WrongEntryCount {
                    expected,
                    found: expected + 1,
                });
            }
            table.push(entry);
            continue;
        }
        if !table.is_empty() {
            return Err(CubeLutError::InvalidLine(number));
        }
        let (keyword, rest) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(keyword, rest)| (keyword, rest.trim()));
        match keyword.to_ascii_uppercase().as_str() {
            "TITLE" => title = Some(rest.trim_matches('"').trim().to_owned()),
            "LUT_3D_SIZE" => {
                let parsed: u32 = rest
                    .parse()
                    .map_err(|_| CubeLutError::InvalidLine(number))?;
                if !(MIN_LUT_SIZE..=MAX_LUT_SIZE).contains(&parsed) {
                    return Err(CubeLutError::SizeOutOfRange(parsed));
                }
                size = Some(parsed);
                expected = (parsed as usize).pow(3);
                table.reserve_exact(expected);
            }
            "LUT_1D_SIZE" | "LUT_1D_INPUT_RANGE" => return Err(CubeLutError::Unsupported1d),
            "DOMAIN_MIN" => domain_min = parse_triplet(rest, number)?,
            "DOMAIN_MAX" => domain_max = parse_triplet(rest, number)?,
            "LUT_3D_INPUT_RANGE" => {
                let [low, high] = parse_numbers::<2>(rest, number)?;
                domain_min = [low; 3];
                domain_max = [high; 3];
            }
            _ => {}
        }
    }

    let size = size.ok_or(CubeLutError::MissingSize)?;
    if table.len() != expected {
        return Err(CubeLutError::WrongEntryCount {
            expected,
            found: table.len(),
        });
    }
    if (0..3).any(|channel| domain_min[channel] >= domain_max[channel]) {
        return Err(CubeLutError::InvalidDomain);
    }
    Ok(CubeLut {
        id,
        title: title.filter(|title| !title.is_empty()),
        size,
        domain_min,
        domain_max,
        table: table.into_boxed_slice(),
    })
}

fn parse_triplet(text: &str, line: usize) -> Result<[f32; 3], CubeLutError> {
    parse_numbers::<3>(text, line)
}

fn parse_numbers<const N: usize>(text: &str, line: usize) -> Result<[f32; N], CubeLutError> {
    let mut values = [0.0_f32; N];
    let mut fields = text.split_whitespace();
    for value in &mut values {
        *value = fields
            .next()
            .and_then(|field| field.parse().ok())
            .ok_or(CubeLutError::InvalidLine(line))?;
        if !value.is_finite() {
            return Err(CubeLutError::NonFinite(line));
        }
    }
    if fields.next().is_some() {
        return Err(CubeLutError::InvalidLine(line));
    }
    Ok(values)
}
