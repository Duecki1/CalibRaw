//! Reading sidecars and presets saved in older layouts, and moving app data
//! older versions saved elsewhere.
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
//! - Presets that carry global effects in the masks category
//!   (`presets.rs`).
//! - White balance stored as offsets (`WhiteBalance::LegacyOffset` and
//!   `older_reader_offsets`, `pipeline/basicadj.rs`), converted when the
//!   photo is opened (`LoadedRaw::resolve_white_balance`,
//!   `pipeline/raw_loader/white_balance.rs`). Their file layout is read and
//!   written by `white_balance_offsets`.
//! - Performance settings versions 1 to 5: the old desktop decoded-RAW
//!   cache default, automatic vignetting correction, Develop navigation
//!   labels and, on memory-constrained devices, the Preview quality tiers,
//!   updated when the settings are loaded (`calibraw-ui`,
//!   `performance_settings.rs`).
//! - The automatic ONNX Runtimes the WebGPU packages replaced (CPU, Linux
//!   CUDA and Windows DirectML), removed from the model folder on desktop
//!   start-up (`calibraw-ai`, `runtime/package.rs`, which lists them, and
//!   `calibraw-ui`, `app/ai/models.rs`).
//! - The Lensfun 0.3.4 database Android copied into app data, removed on
//!   start-up (`calibraw-ui`, `app/lifecycle/startup.rs`).
//! - Depth Anything V2 Small, the depth model before Distill-Any-Depth
//!   Small, removed from the model folder on start-up (`calibraw-ui`,
//!   `app/ai/models.rs`).
//!
//! The start-up removals run through `remove_retired_app_data`
//! (`calibraw-ui`, `app/lifecycle/startup.rs`). The desktop model folder and
//! ONNX Runtime selection moves (`desktop_model_folder`,
//! `onnx_runtime_selection`) run from `move_desktop_ai_files`
//! (`calibraw-ui`, `app/ai/models.rs`) before the app reads either.
//!
//! `older_readers` works the other way: it writes sidecars and presets so
//! that v1.2 and older can still open them.

mod component_names;
pub(crate) mod depth_range_feather;
pub mod desktop_model_folder;
mod effect_sliders;
pub(crate) mod older_readers;
pub mod onnx_runtime_selection;
pub(crate) mod point_curve_slots;
mod scene_depth_controls;
mod scene_depth_layout;
pub(crate) mod white_balance_offsets;

pub(crate) use effect_sliders::LegacyEffectSliders;
pub(crate) use scene_depth_layout::LegacySceneDepth;

use crate::sidecar::EditState;
use scene_depth_controls::LegacySceneControls;
use serde::{Deserialize, Deserializer};

/// What older layouts kept in a serialized `edits` object besides what the
/// edit decodes: the Effects-card sliders and Relight's old scene controls.
/// Read in the same pass as a document's header.
#[derive(Debug, Default)]
pub(crate) struct LegacyEdits {
    sliders: LegacyEffectSliders,
    scene_controls: LegacySceneControls,
}

impl<'de> Deserialize<'de> for LegacyEdits {
    /// Each reader looks at the same object.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let edits = serde_json::Value::deserialize(deserializer)?;
        let read = |error: serde_json::Error| serde::de::Error::custom(error);
        Ok(Self {
            sliders: LegacyEffectSliders::deserialize(&edits).map_err(read)?,
            scene_controls: LegacySceneControls::deserialize(&edits).map_err(read)?,
        })
    }
}

impl LegacyEdits {
    /// Drops the Effects-card sliders and keeps the rest.
    pub(crate) fn without_global(self) -> Self {
        Self {
            sliders: self.sliders.without_global(),
            ..self
        }
    }
}

/// Brings edits decoded from a sidecar or preset up to date. `legacy` must be
/// read from the same document, before any mask is removed or reordered.
/// Returns whether the edits changed, so the file can be saved again in the
/// current layout.
pub(crate) fn migrate_edits(edits: &mut EditState, legacy: LegacyEdits) -> bool {
    let effects = legacy.sliders.migrate(edits);
    let scene_controls = legacy.scene_controls.migrate(edits);
    let names = component_names::migrate(edits);
    effects || scene_controls || names
}
