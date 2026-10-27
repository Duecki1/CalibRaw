use crate::file_ops::write_atomically;
use crate::luts::LookLut;
use crate::pipeline::remove::RemovePatchSidecarCache;
use crate::pipeline::{
    lensfun_catalog, ExposureParams, GeometryTransform, LensCorrection, LensfunCorrections,
    LensfunLens, LoadedRaw, ManualLensCorrection, MaskGeometry, MaskImage, MaskKind, MaskStack,
    RemoveEditState, SubjectRefinement, WhiteBalance, MAX_LOCAL_MASKS, MAX_MASK_COMPONENTS,
    MAX_PATH_POINTS, REMOVE_MAX_PATCHES_PER_STROKE, REMOVE_MAX_STROKES,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod files;
mod mask_assets;
mod remove_assets;
mod size_limits;
use crate::migrations::{self, LegacyEffectSliders};
pub use files::*;
use mask_assets::*;
use remove_assets::*;
pub use size_limits::*;

// The newest schema of the "CalibRaw edit sidecar" format. Bump this for every incompatible
// serialized layout change; pre-release AuRaw sidecars use a different format discriminator.
// Schemas 2 and 3 share the layout of schema 1. Schema 2 adds the Relight effect and manual lens
// correction, schema 3 the LUT; edits are written with the oldest schema that holds them
// (`migrations::older_readers`).
pub const SIDECAR_SCHEMA_VERSION: u32 = 3;
pub const DEVELOPED_THUMBNAIL_CACHE_VERSION_SALT: u64 = 0x4155_5241_5700_0007;
pub const SIDECAR_SUFFIX: &str = ".calibraw";
/// The AI-denoise result saved next to a RAW. It is large derived data, so it
/// lives outside the sidecar and only while the saved edit uses AI denoise.
pub const AI_DENOISE_SUFFIX: &str = ".calibraw-denoise";
/// The people analysis kept next to a RAW, so its people masks change person
/// and parts without running a model. Derived data like the AI-denoise
/// result, kept while the saved edit has a people mask.
pub const PEOPLE_ANALYSIS_SUFFIX: &str = ".calibraw-people";
pub const DEVELOPED_THUMBNAIL_SUFFIX: &str = ".calibraw-thumb.jpg";
const DEVELOPED_THUMBNAIL_FINGERPRINT_SUFFIX: &str = ".calibraw-thumb.fingerprint";
pub const MAX_SIDECAR_BYTES: u64 = if cfg!(target_os = "android") {
    32 * 1024 * 1024
} else {
    256 * 1024 * 1024
};

const SIDECAR_FORMAT: &str = "CalibRaw edit sidecar";
const MAX_BRUSH_DABS: usize = 1_000_000;
const MAX_OBJECT_STROKES: usize = 4096;
const MAX_OBJECT_STROKE_POINTS: usize = 1_000_000;
const MAX_MASK_IMAGE_EDGE: u32 = 8192;
const MAX_MASK_ASSET_REFS: usize = MAX_LOCAL_MASKS * MAX_MASK_COMPONENTS;
const MAX_MASK_ASSETS: usize = MAX_MASK_ASSET_REFS + 1;
const MAX_REMOVE_ASSET_REFS: usize = REMOVE_MAX_STROKES * REMOVE_MAX_PATCHES_PER_STROKE;
/// Memory a sidecar may expand to when its mask and retouch assets are decoded; the
/// mask and retouch budgets are checked separately against this one value.
const MAX_DECODED_ASSET_BYTES: u64 = if cfg!(target_os = "android") {
    256 * 1024 * 1024
} else {
    512 * 1024 * 1024
};
/// Longest edge, in pixels, of a stored retouch patch.
const MAX_REMOVE_PATCH_EDGE: u32 = 32_768;
/// Decoded bytes per retouch patch pixel: three `u16` scene colour channels plus one
/// `u8` alpha.
const REMOVE_PATCH_BYTES_PER_PIXEL: u64 = 3 * 2 + 1;
const MAX_EDIT_NAME_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidecarTarget {
    Desktop {
        raw_path: PathBuf,
    },
    /// A photo in Android's media library.
    Android {
        raw_uri: String,
        display_name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct LensEditState {
    pub enabled: bool,
    pub maker: String,
    pub model: String,
    /// Apply the profile's distortion, scale and lateral chromatic aberration
    /// data. Sidecars written before this option existed apply everything.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub geometry: bool,
    /// Apply the profile's vignetting data.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub vignetting: bool,
    /// Hand-set corrections applied instead of the profile, which stays
    /// selected for switching back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<ManualLensCorrection>,
}

impl Default for LensEditState {
    fn default() -> Self {
        Self {
            enabled: false,
            maker: String::new(),
            model: String::new(),
            geometry: true,
            vignetting: true,
            manual: None,
        }
    }
}

impl LensEditState {
    pub fn corrections(&self) -> LensfunCorrections {
        LensfunCorrections {
            geometry: self.geometry,
            vignetting: self.vignetting,
        }
    }

    /// The correction this edit applies to `raw`, if any. A profile missing
    /// from `raw`'s catalog is still requested by name, and an edit without a
    /// lens name uses the catalog's automatic match.
    pub fn requested_correction(&self, raw: &LoadedRaw) -> Option<LensCorrection> {
        if !self.enabled {
            return None;
        }
        if let Some(manual) = self.manual {
            return (!manual.is_identity()).then_some(LensCorrection::Manual(manual));
        }
        let catalog = lensfun_catalog(raw);
        let lens = catalog
            .lenses
            .into_iter()
            .find(|lens| lens.maker == self.maker && lens.model == self.model)
            .or_else(|| {
                (!self.maker.is_empty() || !self.model.is_empty()).then(|| LensfunLens {
                    maker: self.maker.clone(),
                    model: self.model.clone(),
                    ..LensfunLens::default()
                })
            })
            .or(catalog.auto_match)?;
        Some(LensCorrection::Profile(LensfunLens {
            corrections: self.corrections(),
            ..lens
        }))
    }
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct EditState {
    #[serde(serialize_with = "migrations::older_readers::serialize_exposure")]
    pub exposure: ExposureParams,
    #[serde(default)]
    pub geometry: GeometryTransform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_profile: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_lut: Option<LookLut>,
    pub masks: Arc<MaskStack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_refinement: Option<SubjectRefinement>,
    pub lens: LensEditState,
    #[serde(default, skip_serializing_if = "arc_remove_is_empty")]
    pub remove: Arc<RemoveEditState>,
    #[serde(default)]
    pub ai_masks_need_update: bool,
}

fn arc_remove_is_empty(remove: &Arc<RemoveEditState>) -> bool {
    remove.is_empty()
}

pub fn default_edit_state() -> EditState {
    EditState {
        exposure: ExposureParams::scene_referred_default(),
        geometry: GeometryTransform::default(),
        camera_profile: None,
        look_lut: None,
        masks: Arc::new(MaskStack::default()),
        subject_refinement: None,
        lens: LensEditState::default(),
        remove: Arc::new(RemoveEditState::default()),
        ai_masks_need_update: false,
    }
}

pub fn edit_state_has_adjustments(edits: &EditState) -> bool {
    let default = default_edit_state();
    edits.exposure != default.exposure
        || edits.geometry != default.geometry
        || edits.camera_profile != default.camera_profile
        || edits.look_lut != default.look_lut
        || edits.masks != default.masks
        || edits.subject_refinement != default.subject_refinement
        || edits.lens != default.lens
        || edits.remove != default.remove
}

fn synchronize_subject_refinement(edits: &mut EditState) {
    let refinement = edits.subject_refinement.clone().or_else(|| {
        (!edits.masks.subject_refinement.is_empty()).then(|| edits.masks.subject_refinement.clone())
    });
    let refinement = refinement.filter(|refinement| !refinement.is_empty());
    Arc::make_mut(&mut edits.masks).subject_refinement = refinement.clone().unwrap_or_default();
    edits.subject_refinement = refinement;
}

/// Culling metadata is independent of development adjustments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoFlag {
    Rejected,
    #[default]
    Unflagged,
    Picked,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PhotoReview {
    pub flag: PhotoFlag,
    pub rating: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SidecarMetadata {
    pub review: PhotoReview,
    pub editing_time_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
struct SidecarDocument {
    format: String,
    schema_version: u32,
    edits: EditState,
    #[serde(default)]
    review: PhotoReview,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    editing_time_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mask_assets: Vec<SidecarMaskAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mask_asset_refs: Vec<SidecarMaskAssetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene_depth_asset: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remove_assets: Vec<SidecarRemoveAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remove_asset_refs: Vec<SidecarRemoveAssetRef>,
}

const fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[derive(Deserialize)]
struct SidecarHeader {
    format: String,
    schema_version: u32,
    /// Collected in the same pass that reads the header.
    #[serde(default)]
    edits: LegacyEffectSliders,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
struct SidecarMaskAsset {
    width: u32,
    height: u32,
    #[serde(with = "crate::base64_arc_bytes")]
    png: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct SidecarMaskAssetRef {
    mask_index: usize,
    component_index: usize,
    asset_index: usize,
}

struct MaskAssets {
    assets: Vec<SidecarMaskAsset>,
    references: Vec<SidecarMaskAssetRef>,
    scene_depth_asset: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SidecarRemoveEncoding {
    Scene16f,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
struct SidecarRemoveAsset {
    width: u32,
    height: u32,
    encoding: SidecarRemoveEncoding,
    #[serde(with = "crate::base64_arc_bytes")]
    rgb_png: Arc<[u8]>,
    #[serde(with = "crate::base64_arc_bytes")]
    alpha_png: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct SidecarRemoveAssetRef {
    stroke_index: usize,
    patch_index: usize,
    asset_index: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSidecar {
    pub edits: EditState,
    pub review: PhotoReview,
    pub editing_time_ms: u64,
    pub migrated: bool,
}

#[derive(Debug)]
pub enum SidecarError {
    Io(std::io::Error),
    Invalid(String),
    Unsupported(String),
    Platform(String),
    TooLarge(u64),
}

impl fmt::Display for SidecarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Invalid(message) | Self::Unsupported(message) | Self::Platform(message) => {
                formatter.write_str(message)
            }
            Self::TooLarge(bytes) => write!(
                formatter,
                "sidecar is {bytes} bytes; the safety limit is {MAX_SIDECAR_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for SidecarError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) | Self::Unsupported(_) | Self::Platform(_) | Self::TooLarge(_) => None,
        }
    }
}

impl From<std::io::Error> for SidecarError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

mod desktop;
mod transfer;

pub(crate) use transfer::is_manual_mask_kind;
pub use transfer::{
    adjustment_group_is_edited, transfer_edits, AdjustmentCopySettings, AdjustmentPasteMode,
    EditSelection,
};

pub use desktop::sidecar_path_for_raw;
pub use desktop::{
    ai_denoise_path_for_raw, copy_developed_thumbnail_cache, desktop_sidecar_fingerprint,
    developed_thumbnail_cache_is_fresh, developed_thumbnail_path_for_raw,
    invalidate_developed_thumbnail_cache, load_developed_thumbnail_cache,
    people_analysis_path_for_raw, raw_companion_paths, remove_desktop_edits,
    save_developed_thumbnail_cache,
};

pub fn encode(edits: EditState) -> Result<Vec<u8>, SidecarError> {
    encode_with_review(edits, PhotoReview::default())
}

pub fn encode_with_review(edits: EditState, review: PhotoReview) -> Result<Vec<u8>, SidecarError> {
    encode_with_review_and_editing_time(edits, review, 0)
}

pub fn encode_with_review_and_editing_time(
    mut edits: EditState,
    review: PhotoReview,
    editing_time_ms: u64,
) -> Result<Vec<u8>, SidecarError> {
    if review.rating > 5 {
        return Err(SidecarError::Invalid(
            "rating must be between 0 and 5".to_owned(),
        ));
    }
    synchronize_subject_refinement(&mut edits);
    validate_edit_state(&edits)?;
    let schema_version = migrations::older_readers::schema_version(&edits, SIDECAR_SCHEMA_VERSION);
    let mask_assets = extract_mask_assets(&mut edits)?;
    let (remove_assets, remove_asset_refs) = extract_remove_assets(&mut edits)?;
    let document = SidecarDocument {
        format: SIDECAR_FORMAT.to_owned(),
        schema_version,
        edits,
        review,
        editing_time_ms,
        mask_assets: mask_assets.assets,
        mask_asset_refs: mask_assets.references,
        scene_depth_asset: mask_assets.scene_depth_asset,
        remove_assets,
        remove_asset_refs,
    };
    let mut writer = CappedVec::new(MAX_SIDECAR_BYTES);
    serde_json::to_writer(&mut writer, &document).map_err(|error| {
        if writer.limit_reached {
            SidecarError::TooLarge(MAX_SIDECAR_BYTES + 1)
        } else {
            SidecarError::Invalid(format!("could not serialize edit: {error}"))
        }
    })?;
    Ok(writer.bytes)
}

fn decode_versioned_document(
    bytes: &[u8],
    schema_version: u32,
) -> Result<(SidecarDocument, bool), SidecarError> {
    // Keep each future public schema's decoder and migration in its own match arm so an older
    // layout can never be deserialized as the current schema by accident. Schemas 1 to 3 share
    // one layout.
    match schema_version {
        migrations::older_readers::V1_SCHEMA_VERSION
        | migrations::older_readers::V2_SCHEMA_VERSION
        | SIDECAR_SCHEMA_VERSION => {
            serde_json::from_slice::<SidecarDocument>(bytes)
                .map(|document| (document, false))
                .map_err(|error| SidecarError::Invalid(format!("invalid sidecar JSON: {error}")))
        }
        version if version > SIDECAR_SCHEMA_VERSION => Err(SidecarError::Unsupported(format!(
            "sidecar schema {version} is newer than supported schema {SIDECAR_SCHEMA_VERSION}"
        ))),
        version => Err(SidecarError::Unsupported(format!(
            "sidecar schema {version} has no migration path to supported schema {SIDECAR_SCHEMA_VERSION}"
        ))),
    }
}

pub fn decode(bytes: &[u8]) -> Result<LoadedSidecar, SidecarError> {
    if bytes.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(SidecarError::TooLarge(bytes.len() as u64));
    }
    let header: SidecarHeader = serde_json::from_slice(bytes)
        .map_err(|error| SidecarError::Invalid(format!("invalid sidecar JSON: {error}")))?;
    if header.format != SIDECAR_FORMAT {
        return Err(SidecarError::Invalid(
            "not a CalibRaw edit sidecar".to_owned(),
        ));
    }

    let (mut document, schema_migrated) = decode_versioned_document(bytes, header.schema_version)?;
    // Mask indices still match the serialized masks here.
    let edits_migrated = migrations::migrate_edits(&mut document.edits, header.edits);
    restore_mask_assets(
        &mut document.edits,
        &document.mask_assets,
        &document.mask_asset_refs,
        document.scene_depth_asset,
    )?;
    restore_remove_assets(
        &mut document.edits,
        &document.remove_assets,
        &document.remove_asset_refs,
    )?;

    synchronize_subject_refinement(&mut document.edits);

    validate_edit_state(&document.edits)?;
    document.edits.exposure.sanitize_tone_curves();
    for mask in &mut Arc::make_mut(&mut document.edits.masks).masks {
        mask.adjustments.sanitize_tone_curves();
    }
    validate_edit_state(&document.edits)?;

    Ok(LoadedSidecar {
        edits: document.edits,
        review: PhotoReview {
            rating: document.review.rating.min(5),
            ..document.review
        },
        editing_time_ms: document.editing_time_ms,
        migrated: schema_migrated || edits_migrated,
    })
}

mod validation;
use validation::{invalid, validate_image};

fn validate_scene_depth(masks: &MaskStack) -> Result<(), SidecarError> {
    if let Some(image) = &masks.scene_depth {
        validate_image(image.width, image.height, image.pixels.len(), 1)?;
    }
    Ok(())
}

pub(crate) fn validate_edit_state(edits: &EditState) -> Result<(), SidecarError> {
    validate_scene_depth(&edits.masks)?;
    validation::validate_edit_state(edits)
}
