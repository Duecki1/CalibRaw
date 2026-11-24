//! The scene the RAW stage (highlight reconstruction, demosaic, chromatic aberration, colour
//! denoise and Remove) leaves for every tile of an export, kept from the tone prepass so the
//! final pass does not run that stage again.
//!
//! Each tile stores its core, and on the photo's edges the halo past them, into one canvas of
//! the photo and a halo around it. A final tile then reads its padded region from the canvas.
//! Where a tile reads its halo, the canvas holds what the tile owning that pixel computed rather
//! than what the tile itself would have computed. The two agree wherever a core pixel's result
//! depends on them: a halo pixel is computed exactly once the tile holds the raw samples within
//! the RAW stage's supports around it, and `required_export_tile_halo` reserves those supports
//! on top of the later stages'. Past the canvas (the overhang of the last row and column of
//! tiles) the scene texture keeps the previous tile's texels, which lie farther than the halo
//! from the core.

use super::*;
use crate::pipeline::ExportTile;

/// The RAW stage's scene of an export's tiles, in the scene texture's format.
pub(super) struct RawStageStore<'a> {
    texels: Vec<u8>,
    /// Canvas pixel `(x, y)` is photo pixel `(x - halo, y - halo)`.
    width: u32,
    height: u32,
    halo: u32,
    photo: [u32; 2],
    texel_bytes: u32,
    _reservation: MemoryReservation<'a>,
}

/// Share of the memory the system has available at export start, as a divisor, the store may
/// take. With the export tile's sixteenth (`MOBILE_EXPORT_AVAILABLE_MEMORY_DIVISOR`) it leaves
/// the output raster and the previews room within about a quarter of it. The store's pages are
/// touched one tile at a time during the tone prepass, so a store admitted from total RAM alone
/// grows towards its full size near the prepass's end: a Redmi 24094RAD4G (PowerVR BXM-8-256)
/// export with such a store ended without an error 21 tiles into a 24-tile tone prepass.
const AVAILABLE_MEMORY_DIVISOR: u64 = 8;

/// A region of a tile: its origin in the tile's padded pixels and its extent.
#[derive(Clone, Copy)]
pub(super) struct TileRegion {
    pub(super) origin: [u32; 2],
    pub(super) extent: [u32; 2],
}

impl<'a> RawStageStore<'a> {
    /// A store for the tiles of `plan` with `texel_bytes` per texel, if `memory` has room for it,
    /// it takes at most its share of the memory the system has available now
    /// ([`AVAILABLE_MEMORY_DIVISOR`]; unchecked where the platform does not report it) and the
    /// allocation succeeds.
    pub(super) fn new(
        plan: &TilePlan,
        texel_bytes: u32,
        memory: &'a IntermediateMemory,
    ) -> Option<Self> {
        let halo = plan.spec.halo;
        let width = plan.full_width.checked_add(halo.checked_mul(2)?)?;
        let height = plan.full_height.checked_add(halo.checked_mul(2)?)?;
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))?
            .checked_mul(u64::from(texel_bytes))?;
        if let Some(available) = calibraw_core::system_memory::available_memory_bytes() {
            if bytes > available / AVAILABLE_MEMORY_DIVISOR {
                const MIB: f64 = 1024.0 * 1024.0;
                calibraw_core::diagnostics::record(format!(
                    "Export RAW-stage scene ({:.1} MiB) exceeds 1/{AVAILABLE_MEMORY_DIVISOR} of the {:.0} MiB of system memory available",
                    bytes as f64 / MIB,
                    available as f64 / MIB,
                ));
                return None;
            }
        }
        let reservation = memory.reserve(bytes)?;
        let texels = zeroed_bytes(usize::try_from(bytes).ok()?)?;
        Some(Self {
            texels,
            width,
            height,
            halo,
            photo: [plan.full_width, plan.full_height],
            texel_bytes,
            _reservation: reservation,
        })
    }

    /// Bytes of one stored texel, in the scene texture's format.
    pub(super) fn texel_bytes(&self) -> u32 {
        self.texel_bytes
    }

    /// Bytes the store holds.
    pub(super) fn bytes(&self) -> usize {
        self.texels.len()
    }

    /// The part of `tile` the store keeps from it: its core, extended by the halo on the sides
    /// where the core meets the photo's edge.
    pub(super) fn owned_region(&self, tile: ExportTile) -> TileRegion {
        let axis = |core: u32, core_extent: u32, local_core: u32, photo: u32| {
            let start = if core == 0 {
                local_core - self.halo
            } else {
                local_core
            };
            let end = if core + core_extent >= photo {
                local_core + core_extent + self.halo
            } else {
                local_core + core_extent
            };
            (start, end - start)
        };
        let (x, width) = axis(
            tile.core_x,
            tile.core_width,
            tile.local_core_x,
            self.photo[0],
        );
        let (y, height) = axis(
            tile.core_y,
            tile.core_height,
            tile.local_core_y,
            self.photo[1],
        );
        TileRegion {
            origin: [x, y],
            extent: [width, height],
        }
    }

    /// Stores row `row` of `tile`'s owned region ([`Self::owned_region`]).
    pub(super) fn store_row(&mut self, tile: ExportTile, row: u32, texels: &[u8]) {
        let region = self.owned_region(tile);
        let [x, y] = self.canvas_position(tile, region.origin);
        let start = self.texel_offset(x, y + row);
        self.texels[start..start + texels.len()].copy_from_slice(texels);
    }

    /// Writes `tile`'s padded region, as far as the canvas reaches, into the scene texture of
    /// `pipeline`.
    pub(super) fn write_tile(
        &self,
        pipeline: &RawGpuPipeline,
        queue: &wgpu::Queue,
        tile: ExportTile,
    ) -> Result<()> {
        let [x, y] = self.canvas_position(tile, [0, 0]);
        let extent = [
            tile.padded_width.min(self.width - x),
            tile.padded_height.min(self.height - y),
        ];
        let bytes_per_row = self.width * self.texel_bytes;
        pipeline.write_scene_region(
            queue,
            &self.texels[self.texel_offset(x, y)..],
            bytes_per_row,
            [0, 0],
            extent,
        )
    }

    /// The canvas position of the pixel at `local` in `tile`'s padded pixels. Tiles start at
    /// most a halo before the photo, which is where the canvas starts.
    fn canvas_position(&self, tile: ExportTile, local: [u32; 2]) -> [u32; 2] {
        let halo = i64::from(self.halo);
        let axis = |origin: i32, local: u32| (i64::from(origin) + halo + i64::from(local)) as u32;
        [
            axis(tile.global_origin_x, local[0]),
            axis(tile.global_origin_y, local[1]),
        ]
    }

    fn texel_offset(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * self.texel_bytes as usize
    }
}

/// `len` zeroed bytes, or `None` when the allocation fails. A large zeroed allocation comes from
/// the system as untouched pages, so unlike zeroing a reserved `Vec` it costs nothing until a
/// tile stores its texels there. Every texel a tile reads is stored by its owner first.
fn zeroed_bytes(len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }
    let layout = std::alloc::Layout::array::<u8>(len).ok()?;
    // SAFETY: `layout` has a non-zero size. A non-null result points to `len` zeroed, so
    // initialised, bytes from the global allocator with the alignment of `u8`, which is the
    // allocation a `Vec<u8>` of capacity `len` owns and frees.
    unsafe {
        let pointer = std::alloc::alloc_zeroed(layout);
        if pointer.is_null() {
            return None;
        }
        Some(Vec::from_raw_parts(pointer, len, len))
    }
}
