//! Baking a [`CameraLook`] into the 3D LUT the look-LUT stage applies, and
//! checking that the table keeps the colour cube's orientation.

use super::CameraLook;
use crate::color_math::{srgb_decode_signed, srgb_encode};
use crate::luts::{CubeLut, LutId, ResolvedLook};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Lattice points per axis of a baked camera look.
pub const BAKED_LUT_SIZE: u32 = 33;

/// What [`CameraLook::check_folds`] found in the baked lattice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeCheck {
    /// Lattice cells whose mapping reverses orientation: the Jacobian
    /// determinant of the cell, from the differences along its edges, is
    /// negative.
    pub folded_cells: usize,
    /// The smallest determinant of any cell, relative to the identity's.
    pub min_determinant: f32,
}

impl BakeCheck {
    pub fn has_folds(&self) -> bool {
        self.folded_cells > 0
    }
}

/// The tables baked from one fitted look. A look's fit never changes once
/// made, only its amount, so its clones share these.
#[derive(Default)]
pub(super) struct BakeCache {
    table: OnceLock<Arc<CubeLut>>,
    /// The last composition with a creative LUT. A leaf lock, held only to
    /// read or replace the entry, never while a table is computed.
    composed: Mutex<Option<ComposedTable>>,
}

impl fmt::Debug for BakeCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BakeCache")
    }
}

/// The table a composition made and what it was made from.
struct ComposedTable {
    creative: LutId,
    creative_amount: u32,
    camera_amount: u32,
    table: Arc<CubeLut>,
}

impl CameraLook {
    /// [`Self::bake`], made once per fit and shared by its clones.
    pub fn baked(&self) -> Arc<CubeLut> {
        Arc::clone(self.bakes.table.get_or_init(|| Arc::new(self.bake())))
    }

    /// The look the look-LUT stage renders for this camera look at its
    /// amount, followed by `creative`, the edit's own LUT, at its amount.
    ///
    /// Alone, the camera look renders its baked table at its amount. With a
    /// creative LUT, both are composed on the CPU into one table applied at
    /// full amount: each blends as the stage blends a LUT, linearly in
    /// linear light, and the creative LUT reads the camera look's result.
    /// The composed table is cached for the last creative LUT and amounts.
    pub fn resolved(&self, creative: Option<&ResolvedLook>) -> ResolvedLook {
        let exposure_ev = self.applied_exposure_ev();
        let Some(creative) = creative.filter(|creative| creative.amount > 0.0) else {
            return ResolvedLook {
                lut: self.baked(),
                amount: self.amount,
                exposure_ev,
            };
        };
        let creative_amount = creative.amount.min(1.0);
        let key = (
            creative.lut.id(),
            creative_amount.to_bits(),
            self.amount.to_bits(),
        );
        let cached = self
            .bakes
            .composed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|entry| (entry.creative, entry.creative_amount, entry.camera_amount) == key)
            .map(|entry| Arc::clone(&entry.table));
        let table = cached.unwrap_or_else(|| {
            let table = Arc::new(self.compose(&creative.lut, creative_amount));
            *self
                .bakes
                .composed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(ComposedTable {
                creative: key.0,
                creative_amount: key.1,
                camera_amount: key.2,
                table: Arc::clone(&table),
            });
            table
        });
        ResolvedLook {
            lut: table,
            amount: 1.0,
            exposure_ev,
        }
    }

    /// The camera look at its amount followed by `creative` at
    /// `creative_amount`, on a lattice at least as fine as either.
    fn compose(&self, creative: &CubeLut, creative_amount: f32) -> CubeLut {
        let camera = self.baked();
        let camera_amount = self.amount;
        let size = BAKED_LUT_SIZE.max(creative.size());
        let last = (size - 1) as f32;
        let blend = |linear: f32, target: f32, amount: f32| linear + amount * (target - linear);
        let mut table = Vec::with_capacity((size as usize).pow(3));
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let input = [r as f32 / last, g as f32 / last, b as f32 / last];
                    let looked = camera.sample_tetrahedral(input);
                    let camera_linear: [f32; 3] = std::array::from_fn(|channel| {
                        blend(
                            srgb_decode_signed(input[channel]),
                            srgb_decode_signed(looked[channel]),
                            camera_amount,
                        )
                    });
                    let graded = creative.sample_tetrahedral(camera_linear.map(srgb_encode));
                    table.push(std::array::from_fn(|channel| {
                        srgb_encode(blend(
                            camera_linear[channel],
                            srgb_decode_signed(graded[channel]),
                            creative_amount,
                        ))
                    }));
                }
            }
        }
        CubeLut::from_lattice(creative.title().map(str::to_owned), size, table)
            // The size is within the LUT limits and every entry is finite
            // and clamped by the encoding.
            .unwrap_or_else(|_| unreachable!("a composed look is a valid LUT"))
    }

    /// The curve, matrix and residual at full amount as a
    /// [`BAKED_LUT_SIZE`]³ LUT over sRGB-encoded `[0, 1]`, for the look-LUT
    /// stage. The amount is applied there, as for any LUT; the exposure
    /// offset is not part of the table.
    pub fn bake(&self) -> CubeLut {
        let table = self.lattice(Self::map_display);
        CubeLut::from_lattice(Some("Camera match".to_owned()), BAKED_LUT_SIZE, table)
            // The lattice size is within the LUT limits and every entry is
            // finite and clamped, so construction cannot fail.
            .unwrap_or_else(|_| unreachable!("a baked camera look is a valid LUT"))
    }

    /// Checks the baked lattice for folds. The check reads the lattice
    /// before the bake clamps it to `[0, 1]`: the clamp only flattens the
    /// colours beyond the display range onto its faces, while the cells it
    /// cuts through would show a spurious mixed orientation.
    pub fn check_folds(&self) -> BakeCheck {
        let size = BAKED_LUT_SIZE as usize;
        let table = self.lattice(Self::map_display_unclamped);
        let entry = |r: usize, g: usize, b: usize| table[r + size * (g + size * b)];
        let cell = 1.0 / (size - 1) as f32;
        let identity = cell * cell * cell;
        let mut folded_cells = 0;
        let mut min_determinant = f32::INFINITY;
        for b in 0..size - 1 {
            for g in 0..size - 1 {
                for r in 0..size - 1 {
                    // Each column is the mean of the four edge differences
                    // along one axis.
                    let mut columns = [[0.0_f32; 3]; 3];
                    for (axis, column) in columns.iter_mut().enumerate() {
                        for corner in 0..4 {
                            let (u, v) = (corner & 1, corner >> 1);
                            let (low, high) = match axis {
                                0 => (entry(r, g + u, b + v), entry(r + 1, g + u, b + v)),
                                1 => (entry(r + u, g, b + v), entry(r + u, g + 1, b + v)),
                                _ => (entry(r + u, g + v, b), entry(r + u, g + v, b + 1)),
                            };
                            for channel in 0..3 {
                                column[channel] += (high[channel] - low[channel]) * 0.25;
                            }
                        }
                    }
                    let [x, y, z] = columns;
                    let determinant = x[0] * (y[1] * z[2] - y[2] * z[1])
                        - y[0] * (x[1] * z[2] - x[2] * z[1])
                        + z[0] * (x[1] * y[2] - x[2] * y[1]);
                    let relative = determinant / identity;
                    min_determinant = min_determinant.min(relative);
                    if relative < -1e-4 {
                        folded_cells += 1;
                    }
                }
            }
        }
        BakeCheck {
            folded_cells,
            min_determinant,
        }
    }

    /// `map` at every point of the [`BAKED_LUT_SIZE`]³ lattice, red index
    /// fastest.
    fn lattice(&self, map: impl Fn(&Self, [f32; 3]) -> [f32; 3]) -> Vec<[f32; 3]> {
        let size = BAKED_LUT_SIZE as usize;
        let last = (size - 1) as f32;
        let mut table = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    table.push(map(
                        self,
                        [r as f32 / last, g as f32 / last, b as f32 / last],
                    ));
                }
            }
        }
        table
    }
}
