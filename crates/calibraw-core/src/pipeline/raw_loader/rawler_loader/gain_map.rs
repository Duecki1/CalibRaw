//! DNG GainMap opcodes (OpcodeList2, opcode 9): the lens shading phone
//! cameras record beside the samples instead of correcting them, one map per
//! position of the 2x2 Bayer tile. Rawler does not apply opcode lists.
//!
//! As darktable's rawprepare (`_check_gain_maps`), the maps are used only as
//! exactly four maps, one per Bayer position, each covering the whole stage-2
//! image; anything else is skipped and the photo decodes without them. A map
//! is sampled as the DNG SDK's `dng_opcode_GainMap`: pixel `(row, column)` of
//! the stage-2 image (the ActiveArea) sits at `(row / height, column / width)`
//! in map coordinates, and the gain is interpolated bilinearly between map
//! points, holding the edge values outside them.

use super::{entry, Geometry};
use crate::pipeline::LensShadingMap;
use anyhow::{bail, ensure, Context, Result};
use rawler::formats::tiff::{Value, IFD};
use rawler::tags::DngTag;

const GAIN_MAP_OPCODE: u32 = 9;
/// Bytes of a GainMap's parameters before its gains.
const GAIN_MAP_HEADER_BYTES: usize = 76;

/// One GainMap opcode. Pixel bounds are in the stage-2 image.
struct GainMap {
    top: u32,
    left: u32,
    bottom: u32,
    right: u32,
    plane: u32,
    planes: u32,
    row_pitch: u32,
    col_pitch: u32,
    /// Map points vertically and horizontally.
    points: [usize; 2],
    /// Spacing and origin of the map points, vertical then horizontal, as
    /// fractions of the stage-2 image height and width.
    spacing: [f64; 2],
    origin: [f64; 2],
    map_planes: u32,
    /// Row-major from the top-left map point.
    gains: Vec<f32>,
}

/// Big-endian reader over an opcode list; the DNG specification stores
/// opcode lists big-endian whatever the file's byte order.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        ensure!(len <= self.bytes.len(), "truncated DNG opcode list");
        let (taken, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }
}

impl GainMap {
    fn parse(params: &[u8]) -> Result<Self> {
        ensure!(
            params.len() >= GAIN_MAP_HEADER_BYTES
                && (params.len() - GAIN_MAP_HEADER_BYTES).is_multiple_of(4),
            "invalid DNG GainMap parameter size {}",
            params.len()
        );
        let mut reader = Reader { bytes: params };
        let [top, left, bottom, right, plane, planes, row_pitch, col_pitch] =
            [(); 8].map(|()| reader.u32());
        let points = [reader.u32()? as usize, reader.u32()? as usize];
        let spacing = [reader.f64()?, reader.f64()?];
        let origin = [reader.f64()?, reader.f64()?];
        let map_planes = reader.u32()?;
        let count = (params.len() - GAIN_MAP_HEADER_BYTES) / 4;
        ensure!(
            points[0]
                .checked_mul(points[1])
                .and_then(|n| n.checked_mul(map_planes as usize))
                == Some(count),
            "DNG GainMap gain count differs from its map size"
        );
        let gains = (0..count)
            .map(|_| reader.f32())
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            gains.iter().all(|gain| gain.is_finite() && *gain >= 0.0),
            "DNG GainMap contains invalid gains"
        );
        Ok(Self {
            top: top?,
            left: left?,
            bottom: bottom?,
            right: right?,
            plane: plane?,
            planes: planes?,
            row_pitch: row_pitch?,
            col_pitch: col_pitch?,
            points,
            spacing,
            origin,
            map_planes,
            gains,
        })
    }

    /// Whether this map covers one position of the 2x2 Bayer tile over a
    /// whole `height`×`width` stage-2 image with a usable grid.
    fn covers_bayer_position(&self, height: usize, width: usize) -> bool {
        self.plane == 0
            && self.planes == 1
            && self.map_planes == 1
            && self.row_pitch == 2
            && self.col_pitch == 2
            && self.top <= 1
            && self.left <= 1
            && self.bottom as usize == height
            && self.right as usize == width
            && self.points.iter().all(|&points| points >= 2)
            && self.spacing.iter().all(|s| s.is_finite() && *s > 0.0)
            && self.origin.iter().all(|o| o.is_finite())
    }

    /// The gain at stage-2 position `(row, column)` of a `height`×`width`
    /// image.
    fn gain_at(&self, row: f64, column: f64, [height, width]: [f64; 2]) -> f32 {
        let axis = |position: f64, extent: f64, axis: usize| {
            let last = self.points[axis] - 1;
            let point = ((position / extent - self.origin[axis]) / self.spacing[axis])
                .clamp(0.0, last as f64);
            let index = (point as usize).min(last - 1);
            (index, point - index as f64)
        };
        let (row, row_fraction) = axis(row, height, 0);
        let (column, column_fraction) = axis(column, width, 1);
        let at = |row: usize, column: usize| f64::from(self.gains[row * self.points[1] + column]);
        let top = at(row, column) * (1.0 - column_fraction) + at(row, column + 1) * column_fraction;
        let bottom = at(row + 1, column) * (1.0 - column_fraction)
            + at(row + 1, column + 1) * column_fraction;
        (top * (1.0 - row_fraction) + bottom * row_fraction) as f32
    }
}

/// Parses an opcode list into its GainMaps and the IDs of the other opcodes.
fn parse_opcode_list(bytes: &[u8]) -> Result<(Vec<GainMap>, Vec<u32>)> {
    let mut reader = Reader { bytes };
    let count = reader.u32()?;
    let mut maps = Vec::new();
    let mut skipped = Vec::new();
    for _ in 0..count {
        let id = reader.u32()?;
        let _version = reader.u32()?;
        let _flags = reader.u32()?;
        let size = reader.u32()? as usize;
        let params = reader.take(size)?;
        if id == GAIN_MAP_OPCODE {
            maps.push(GainMap::parse(params)?);
        } else {
            skipped.push(id);
        }
    }
    Ok((maps, skipped))
}

/// The GainMaps of a Bayer DNG, one per position of the 2x2 tile.
pub(super) struct BayerGainMaps {
    maps: Vec<GainMap>,
}

impl BayerGainMaps {
    /// The GainMaps in the RAW IFD's OpcodeList2. `None` without the list or
    /// when its GainMaps are not one per Bayer position over the whole
    /// stage-2 image of `geometry`; skipped opcodes are recorded in the
    /// diagnostics, and the photo then decodes without them.
    pub(super) fn read(raw: &IFD, geometry: Geometry) -> Option<Self> {
        let list = entry(raw, DngTag::OpcodeList2 as u16)?;
        let parsed = match &list.value {
            Value::Undefined(bytes) | Value::Byte(bytes) => parse_opcode_list(bytes),
            _ => Err(anyhow::anyhow!("DNG OpcodeList2 is not a byte list")),
        };
        match parsed.and_then(|(maps, skipped)| {
            if !skipped.is_empty() {
                skipped_opcodes(&format!("opcodes {skipped:?} are not applied"));
            }
            Self::bayer(maps, geometry)
        }) {
            Ok(maps) => maps,
            Err(error) => {
                skipped_opcodes(&format!("{error:#}; decoding without its GainMaps"));
                None
            }
        }
    }

    fn bayer(maps: Vec<GainMap>, geometry: Geometry) -> Result<Option<Self>> {
        if maps.is_empty() {
            return Ok(None);
        }
        let [top, left, bottom, right] = geometry.active_area;
        let (height, width) = (bottom - top, right - left);
        let mut positions = [false; 4];
        for map in &maps {
            ensure!(
                map.covers_bayer_position(height, width),
                "a DNG GainMap does not cover one Bayer position of the whole image"
            );
            let position = (map.top * 2 + map.left) as usize;
            ensure!(
                !positions[position],
                "two DNG GainMaps cover one Bayer position"
            );
            positions[position] = true;
        }
        if maps.len() != 4 {
            bail!(
                "DNG has {} GainMaps instead of one per Bayer position",
                maps.len()
            );
        }
        Ok(Some(Self { maps }))
    }

    /// The photo's lens shading: each CFA channel takes the map of its Bayer
    /// position. `channel_at(x, y)` is the CFA channel (red, green, blue,
    /// second green) of sensor pixel `(x, y)`, as the decode assigns it.
    pub(super) fn lens_shading(
        &self,
        geometry: Geometry,
        channel_at: impl Fn(usize, usize) -> u8,
    ) -> Result<LensShadingMap> {
        let [top, left, bottom, right] = geometry.active_area;
        let extent = [(bottom - top) as f64, (right - left) as f64];
        let mut by_channel: [Option<&GainMap>; 4] = [None; 4];
        for map in &self.maps {
            let channel = channel_at(left + map.left as usize, top + map.top as usize);
            let slot = by_channel
                .get_mut(usize::from(channel))
                .context("DNG GainMap covers a non-Bayer CFA channel")?;
            ensure!(slot.is_none(), "two DNG GainMaps cover one CFA channel");
            *slot = Some(map);
        }
        let [Some(red), Some(green), Some(blue), Some(second_green)] = by_channel else {
            bail!("DNG GainMaps do not cover every CFA channel");
        };
        let maps = [red, green, blue, second_green];
        let [width, height] = geometry.dimensions();
        let last = [
            f64::from(width.saturating_sub(1)),
            f64::from(height.saturating_sub(1)),
        ];
        LensShadingMap::from_fn(|[u, v]| {
            let (x, y) = geometry.source_position(f64::from(u) * last[0], f64::from(v) * last[1]);
            let (row, column) = (y - top as f64, x - left as f64);
            maps.map(|map| map.gain_at(row, column, extent))
        })
        .context("DNG GainMaps produced invalid gains")
    }
}

pub(super) fn skipped_opcodes(reason: &str) {
    log::warn!("DNG OpcodeList2: {reason}");
    crate::diagnostics::record(format!("DNG OpcodeList2: {reason}"));
}
