//! Reading sidecars and presets saved in older layouts.
//!
//! Every conversion from an older layout lives in this folder, so the current
//! model carries no compatibility code. Each one is marked
//! `migration: remove in vX.Y.Z`: from that version on, the files it converts
//! are no longer read, and the migration is deleted together with its tests.
//! `cargo xtask migrations` lists every marker and fails once the workspace
//! version reaches one. AGENTS.md has the policy for choosing the version.
//!
//! Compatibility code that cannot be separated from the code it adapts stays
//! there with the same marker and is listed here:
//! - `LocalMask::effect` and `effect_settings`: masks with one legacy effect,
//!   converted when the mask is opened (`pipeline/masks.rs`).
//! - Inline scene-depth pixels of depth-range components, moved into the
//!   asset table on the next save (`sidecar/mask_assets.rs`).
//! - Presets that carry global effects in the masks category
//!   (`presets.rs`).
//! - The old AI-denoise result cache, removed on start-up
//!   (`calibraw-ui`, `app/ai_denoise.rs`).

mod component_names;
pub(crate) mod depth_range_feather;
mod effect_sliders;
pub(crate) mod point_curve_slots;

pub(crate) use effect_sliders::LegacyEffectSliders;

use crate::sidecar::EditState;

/// Brings edits decoded from a sidecar or preset up to date. `sliders` must be
/// read from the same document, before any mask is removed or reordered.
/// Returns whether the edits changed, so the file can be saved again in the
/// current layout.
pub(crate) fn migrate_edits(edits: &mut EditState, sliders: LegacyEffectSliders) -> bool {
    let effects = sliders.migrate(edits);
    let names = component_names::migrate(edits);
    effects || names
}
