//! What a decode read a photo's colour from, and the colour any camera
//! profile gives it. Decodes resolve their profile here too, so a photo given
//! another profile afterwards gets exactly the colour a decode with that
//! profile would give it.

use super::*;
use crate::pipeline::raw_loader::ProfileColor;

/// The calibration signature of a profile embedded in the RAW, which a chosen
/// DCP's calibration matrices are matched to.
#[derive(Clone, Debug)]
pub(in crate::pipeline::raw_loader) enum EmbeddedSignature {
    /// Not read, because the decode could not choose a DCP
    /// ([`CameraProfileMode::MatrixOnly`]).
    Unread,
    Read(Option<String>),
}

/// The signature `mode` matches DCPs against for the RAW at `path`.
pub(in crate::pipeline::raw_loader) fn read_embedded_signature(
    path: &Path,
    mode: CameraProfileMode,
) -> Result<EmbeddedSignature> {
    Ok(match mode {
        CameraProfileMode::Automatic | CameraProfileMode::DcpProfiles => EmbeddedSignature::Read(
            read_optional_profile(path)?.and_then(|profile| profile.camera_calibration_signature),
        ),
        CameraProfileMode::MatrixOnly => EmbeddedSignature::Unread,
    })
}

/// The camera profile a decode applies, and the profiles it could apply.
pub(in crate::pipeline::raw_loader) struct CameraProfileSelection {
    pub(in crate::pipeline::raw_loader) profile: Option<DcpProfile>,
    /// The file of `profile`.
    pub(in crate::pipeline::raw_loader) source: Option<PathBuf>,
    pub(in crate::pipeline::raw_loader) candidates: Vec<CameraProfileCandidate>,
}

/// What LibRaw read a photo's colour from.
#[derive(Clone, Debug)]
pub(in crate::pipeline::raw_loader) struct LibRawColorInputs {
    pub(super) color: LibRawColor,
    /// As-shot multipliers in the sensor's channel order.
    pub(super) physical_wb: [f32; 4],
    pub(super) cdesc: [u8; 4],
    pub(super) baseline_exposure: Option<f32>,
    pub(super) embedded_icc: Option<Vec<u8>>,
}

/// What Rawler read a photo's colour from.
#[derive(Clone, Debug)]
pub(in crate::pipeline::raw_loader) struct RawlerColorInputs {
    pub(in crate::pipeline::raw_loader) xyz_to_cam: [[f32; 3]; 4],
    /// As-shot multipliers: red, green, blue, green.
    pub(in crate::pipeline::raw_loader) wb_coeffs: [f32; 4],
    pub(in crate::pipeline::raw_loader) embedded: DcpProfile,
    pub(in crate::pipeline::raw_loader) analog_balance: [[f32; 4]; 4],
    pub(in crate::pipeline::raw_loader) baseline_exposure: Option<f32>,
}

#[derive(Clone, Debug)]
pub(in crate::pipeline::raw_loader) enum DecoderColor {
    LibRaw(Box<LibRawColorInputs>),
    Rawler(Box<RawlerColorInputs>),
}

/// What a decode read a sensor RAW's colour from
/// (`crate::pipeline::ProfileColorSource`).
#[derive(Clone, Debug)]
pub struct ProfileColorSource {
    camera_make: String,
    camera_model: String,
    signature: EmbeddedSignature,
    decoder: DecoderColor,
}

impl ProfileColorSource {
    pub(in crate::pipeline::raw_loader) fn new(
        camera_make: &str,
        camera_model: &str,
        signature: EmbeddedSignature,
        decoder: DecoderColor,
    ) -> Self {
        Self {
            camera_make: camera_make.to_owned(),
            camera_model: camera_model.to_owned(),
            signature,
            decoder,
        }
    }

    /// The colour `profile`, read from `source`, gives the photo; the
    /// photo's own colour data without one.
    pub(in crate::pipeline::raw_loader) fn resolve(
        &self,
        profile: Option<DcpProfile>,
        source: Option<PathBuf>,
    ) -> Result<ProfileColor> {
        let (cam_to_srgb, white_balance_model, camera_profile) = match &self.decoder {
            DecoderColor::LibRaw(inputs) => {
                let LibRawColorInputs {
                    color,
                    physical_wb,
                    cdesc,
                    baseline_exposure,
                    embedded_icc,
                } = &**inputs;
                let calibration_compatible = profile
                    .as_ref()
                    .is_none_or(DcpProfile::calibration_is_compatible);
                let (cam_to_srgb, profile_weight, white_balance_model) = camera_to_working_matrix(
                    color,
                    *physical_wb,
                    *cdesc,
                    profile.as_ref(),
                    calibration_compatible,
                )?;
                let mut camera_profile = profile
                    .map(|profile| CameraProfile::from_dcp(profile, profile_weight))
                    .unwrap_or_default();
                apply_resolved_default_exposure(&mut camera_profile, *baseline_exposure);
                camera_profile.embedded_camera_icc = embedded_icc.clone();
                (cam_to_srgb, white_balance_model, camera_profile)
            }
            DecoderColor::Rawler(inputs) => {
                let RawlerColorInputs {
                    xyz_to_cam,
                    wb_coeffs,
                    embedded,
                    analog_balance,
                    baseline_exposure,
                } = &**inputs;
                let (cam_to_srgb, weight, white_balance_model) =
                    camera_to_working_matrix_from_profiles(
                        *xyz_to_cam,
                        *wb_coeffs,
                        *b"RGB\0",
                        Some(embedded),
                        profile.as_ref(),
                        *analog_balance,
                    )?;
                let mut camera_profile = profile
                    .map(|profile| CameraProfile::from_dcp(profile, weight))
                    .unwrap_or_default();
                apply_resolved_default_exposure(&mut camera_profile, *baseline_exposure);
                (cam_to_srgb, white_balance_model, camera_profile)
            }
        };
        Ok(ProfileColor {
            cam_to_srgb,
            camera_profile,
            camera_profile_source: source,
            white_balance_model,
        })
    }

    /// The colour and profile list a decode with `mode`, `profile_folder`
    /// and `selected_profile` gives the photo. `None` when the decode did not
    /// read what that needs: the embedded signature, unread in
    /// [`CameraProfileMode::MatrixOnly`].
    pub(in crate::pipeline::raw_loader) fn select(
        &self,
        mode: CameraProfileMode,
        profile_folder: Option<&Path>,
        selected_profile: Option<&Path>,
    ) -> Option<Result<(Arc<ProfileColor>, Vec<CameraProfileCandidate>)>> {
        let signature = match (mode, &self.signature) {
            (CameraProfileMode::MatrixOnly, _) => None,
            (_, EmbeddedSignature::Read(signature)) => signature.clone(),
            (_, EmbeddedSignature::Unread) => return None,
        };
        let selection = select_camera_profiles(
            mode,
            profile_folder,
            selected_profile,
            &self.camera_make,
            &self.camera_model,
            signature,
        );
        Some(
            self.resolve(selection.profile, selection.source)
                .map(|color| (Arc::new(color), selection.candidates)),
        )
    }
}
