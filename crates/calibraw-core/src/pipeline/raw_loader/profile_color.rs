//! The colour a camera profile gives a decoded photo, and changing it
//! without decoding the photo again.
//!
//! A camera profile (a DCP, or the photo's own matrix) never changes the
//! sensor samples, only how their colour is read: the camera-to-working
//! matrix, the profile's tables and tone curve, and the white-balance model.
//! A decode keeps what it read the colour from ([`ProfileColorSource`]), so
//! the same samples can be shown with another profile, exactly as decoding
//! the photo with that profile would show them.

use super::*;

/// The colour a camera profile gives a decoded photo. Views of one decode
/// (proxies, crops, tiles) share it.
#[derive(Clone, Debug)]
pub struct ProfileColor {
    /// Camera RGB to linear Rec.2020 at the as-shot white.
    pub cam_to_srgb: [[f32; 4]; 3],
    pub camera_profile: CameraProfile,
    /// The DCP file applied; `None` for the photo's own colour data.
    pub camera_profile_source: Option<PathBuf>,
    /// Moves the matrix and the profile's table weight with the white
    /// balance; `None` where temperature and tint do not apply.
    pub white_balance_model: Option<CameraWhiteBalanceModel>,
}

impl Default for ProfileColor {
    /// The identity matrix without a profile, for photos whose samples are
    /// already working-space colour.
    fn default() -> Self {
        Self {
            cam_to_srgb: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            white_balance_model: None,
        }
    }
}

/// What a decode read the photo's colour from, kept so the photo can take
/// another camera profile without being decoded again. Only sensor RAWs
/// decoded with LibRaw or Rawler have one.
#[cfg(libraw_available)]
pub use super::libraw_loader::ProfileColorSource;

/// Builds without LibRaw decode no sensor RAWs, so no photo has a colour
/// source.
#[cfg(not(libraw_available))]
#[derive(Debug)]
pub enum ProfileColorSource {}

impl LoadedRaw {
    /// This photo with the camera profile that decoding it with `mode`,
    /// `profile_folder` and `selected_profile` would give it, sharing its
    /// samples. `None` when the decode kept no colour source, or kept one for
    /// another profile mode; the photo must then be decoded again.
    pub fn with_camera_profile_selection(
        &self,
        mode: CameraProfileMode,
        profile_folder: Option<&Path>,
        selected_profile: Option<&Path>,
    ) -> Option<Result<Self>> {
        let source = self.color_source.as_ref()?;
        #[cfg(libraw_available)]
        {
            let selection = source.select(mode, profile_folder, selected_profile)?;
            Some(selection.map(|(color, candidates)| {
                let mut recolored = self.with_profile_color(color);
                recolored.available_camera_profiles = candidates;
                recolored
            }))
        }
        #[cfg(not(libraw_available))]
        {
            let _ = (mode, profile_folder, selected_profile);
            match **source {}
        }
    }

    /// The colour that decoding this photo with `mode`, `profile_folder` and
    /// `selected_profile` would give it, as
    /// [`Self::with_camera_profile_selection`] gives it, to preview that
    /// profile on views of the photo. `None` in the same cases.
    pub fn profile_color_for_selection(
        &self,
        mode: CameraProfileMode,
        profile_folder: Option<&Path>,
        selected_profile: Option<&Path>,
    ) -> Option<Result<Arc<ProfileColor>>> {
        let source = self.color_source.as_ref()?;
        #[cfg(libraw_available)]
        {
            let selection = source.select(mode, profile_folder, selected_profile)?;
            Some(selection.map(|(color, _)| color))
        }
        #[cfg(not(libraw_available))]
        {
            let _ = (mode, profile_folder, selected_profile);
            match **source {}
        }
    }

    /// This photo, a view derived from a decode, with the camera profile of
    /// `colored`, the same decode with another profile
    /// ([`Self::with_camera_profile_selection`]). Like a view derived from
    /// `colored` it shares `colored`'s highlight-chroma estimate.
    pub fn with_profile_color_of(&self, colored: &Self) -> Self {
        Self {
            available_camera_profiles: colored.available_camera_profiles.clone(),
            opposed_chroma_cache: Arc::clone(&colored.opposed_chroma_cache),
            opposed_chroma_source_identity: Arc::clone(&colored.opposed_chroma_source_identity),
            ..self.with_profile_color(Arc::clone(&colored.profile_color))
        }
    }

    /// This photo's samples with `color`. What was derived from the old
    /// colour starts again: the AI-denoised image (its input is in working
    /// colour) and the highlight-chroma estimate (it depends on the white
    /// balance multipliers).
    fn with_profile_color(&self, color: Arc<ProfileColor>) -> Self {
        Self {
            profile_color: color,
            ai_denoised: Arc::new(RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            ..self.clone()
        }
    }
}
