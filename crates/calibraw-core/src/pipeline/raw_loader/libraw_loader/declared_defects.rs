//! Photosites the RAW declares defective. LibRaw reads them but repairs them
//! only in `libraw_dcraw_process`, which CalibRaw does not run, so the decode
//! repairs them itself (`sensor_defects::repair`) before anything measures the
//! mosaic:
//!
//! - every zero sample of a camera whose zeros LibRaw marks bad (`zero_is_bad`:
//!   Panasonic RW2 files other than its encoding 5 ones, and cameras LibRaw
//!   identifies by file size), which `dcraw_process` repairs in
//!   `remove_zeroes`;
//! - the bad pixel opcodes of a DNG's OpcodeList1, `FixBadPixelsConstant` and
//!   `FixBadPixelsList`, which LibRaw keeps unparsed and rawspeed applies.
//!   Rawler refuses DNGs with an OpcodeList1, so they decode here.

use super::*;
use crate::pipeline::raw_loader::dng_opcodes::{self, Reader};
use crate::pipeline::sensor_defects::{repair, MosaicLayout, SensorDefects};

/// DNG opcode IDs.
const FIX_BAD_PIXELS_CONSTANT: u32 = 4;
const FIX_BAD_PIXELS_LIST: u32 = 5;

/// Where `copy_active_pixels` copied the mosaic from: the active area of
/// LibRaw's sensor buffer, at `origin` and `dimensions` before `flip`
/// oriented it.
pub(super) struct ActiveArea {
    pub(super) origin: [usize; 2],
    pub(super) dimensions: [usize; 2],
    pub(super) flip: i32,
}

impl ActiveArea {
    /// The index in the oriented mosaic of sensor buffer position `(x, y)`;
    /// `None` outside the active area.
    fn index_of(&self, x: usize, y: usize) -> Option<usize> {
        let [width, height] = self.dimensions;
        let x = x.checked_sub(self.origin[0]).filter(|&x| x < width)?;
        let y = y.checked_sub(self.origin[1]).filter(|&y| y < height)?;
        let (x, y) = oriented_target_pos(x, y, width, height, self.flip);
        let oriented_width = if matches!(self.flip, 5 | 6) {
            height
        } else {
            width
        };
        Some(y * oriented_width + x)
    }
}

/// Repairs the photosites LibRaw's decode `raw` declares defective in
/// `raw_pixels`, its active `area` laid out as `layout`.
pub(super) fn repair_declared_defects(
    raw: &ffi::libraw_data_t,
    area: &ActiveArea,
    layout: &MosaicLayout<'_>,
    raw_pixels: &mut [u16],
) {
    let color = &raw.rawdata.color;
    let mut defects = SensorDefects::none(raw_pixels.len());
    let mut sources = Vec::new();
    if raw.rawdata.ioparams.zero_is_bad != 0 {
        defects.add(&SensorDefects::where_sample_is(raw_pixels, 0));
        sources.push("zero samples");
    }
    let opcodes = opcode_list_1(color);
    if !opcodes.is_empty() {
        match dng_bad_pixels(opcodes, area, &color.curve, raw_pixels) {
            Ok(Some(declared)) => {
                defects.add(&declared);
                sources.push("DNG bad pixel opcodes");
            }
            Ok(None) => {}
            Err(error) => crate::diagnostics::record(format!(
                "DNG OpcodeList1 is unreadable, so its bad pixels are not repaired: {error:#}"
            )),
        }
    }
    if defects.is_empty() {
        return;
    }
    let declared = defects.count();
    let repaired = repair(layout, raw_pixels, &defects);
    crate::diagnostics::record(format!(
        "Repaired {repaired} of {declared} photosites the RAW declares defective ({})",
        sources.join(", ")
    ));
}

/// The decoded image's OpcodeList1, which LibRaw keeps unparsed; empty
/// without one.
fn opcode_list_1(color: &ffi::libraw_colordata_t) -> &[u8] {
    let list = &color.dng_levels.rawopcodes[0];
    if list.data.is_null() || list.len == 0 {
        return &[];
    }
    // SAFETY: a non-null `data` is LibRaw's copy of the list, `len` bytes it
    // allocated while parsing the file and frees when its context is recycled
    // or closed; `color` belongs to that context, which outlives this borrow.
    unsafe { std::slice::from_raw_parts(list.data.cast::<u8>(), list.len as usize) }
}

/// The photosites the bad pixel opcodes of `list`, an OpcodeList1, declare
/// in `area`; `None` when it has no such opcode or they declare none there.
/// `curve` is LibRaw's linearization curve (identity without a
/// LinearizationTable), which its samples have been through and a
/// `FixBadPixelsConstant` value, a stored value, has not.
fn dng_bad_pixels(
    list: &[u8],
    area: &ActiveArea,
    curve: &[u16; 0x10000],
    raw_pixels: &[u16],
) -> Result<Option<SensorDefects>> {
    let mut defects = SensorDefects::none(raw_pixels.len());
    let mut skipped = Vec::new();
    for opcode in dng_opcodes::opcodes(list)? {
        let mut params = Reader::new(opcode.params);
        match opcode.id {
            FIX_BAD_PIXELS_CONSTANT => {
                let constant = params.u32()?;
                // The decode's CFA places the colours, as rawspeed does too.
                let _bayer_phase = params.u32()?;
                // No stored 16-bit sample matches a larger constant.
                if let Ok(stored) = u16::try_from(constant) {
                    let value = curve[usize::from(stored)];
                    defects.add(&SensorDefects::where_sample_is(raw_pixels, value));
                }
            }
            FIX_BAD_PIXELS_LIST => {
                let _bayer_phase = params.u32()?;
                let points = params.u32()?;
                let rectangles = params.u32()?;
                // Positions in the stage-1 image, the whole sensor buffer:
                // a point is its row, then its column; a rectangle its top,
                // left, bottom and right, the last two exclusive.
                for _ in 0..points {
                    let row = params.u32()? as usize;
                    let column = params.u32()? as usize;
                    if let Some(index) = area.index_of(column, row) {
                        defects.insert(index);
                    }
                }
                for _ in 0..rectangles {
                    let [top, left, bottom, right] =
                        [params.u32()?, params.u32()?, params.u32()?, params.u32()?]
                            .map(|edge| edge as usize);
                    // Only the part inside the active area is visited.
                    let [area_x, area_y] = area.origin;
                    let [area_width, area_height] = area.dimensions;
                    let rows = top.max(area_y)..bottom.min(area_y + area_height);
                    let columns = left.max(area_x)..right.min(area_x + area_width);
                    for row in rows {
                        for column in columns.clone() {
                            if let Some(index) = area.index_of(column, row) {
                                defects.insert(index);
                            }
                        }
                    }
                }
            }
            id => skipped.push(id),
        }
    }
    if !skipped.is_empty() {
        crate::diagnostics::record(format!(
            "DNG OpcodeList1: opcodes {skipped:?} are not applied"
        ));
    }
    Ok((!defects.is_empty()).then_some(defects))
}
