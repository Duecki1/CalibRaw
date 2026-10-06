use super::LoadedRaw;
#[cfg(lensfun_available)]
use super::{CompactPixelMap, LensGeometryMap};
use anyhow::{anyhow, Result};

/// Which parts of a Lensfun profile an application request uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LensfunCorrections {
    /// Distortion, auto-scale and lateral chromatic aberration.
    pub geometry: bool,
    pub vignetting: bool,
}

impl Default for LensfunCorrections {
    fn default() -> Self {
        Self {
            geometry: true,
            vignetting: true,
        }
    }
}

/// A Lensfun profile identity. `corrections` only matters for
/// `apply_lensfun_correction` requests; catalog entries keep the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LensfunLens {
    pub maker: String,
    pub model: String,
    pub corrections: LensfunCorrections,
}

impl LensfunLens {
    pub fn label(&self) -> String {
        match (self.maker.trim(), self.model.trim()) {
            ("", model) => model.to_owned(),
            (maker, "") => maker.to_owned(),
            (maker, model) => format!("{maker} {model}"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LensfunCatalog {
    pub available: bool,
    pub camera_label: String,
    pub lenses: Vec<LensfunLens>,
    pub auto_match: Option<LensfunLens>,
    pub status: String,
}

pub fn lensfun_catalog(raw: &LoadedRaw) -> LensfunCatalog {
    backend::catalog(raw)
}

pub fn apply_lensfun_correction(raw: &LoadedRaw, selection: &LensfunLens) -> Result<LoadedRaw> {
    backend::apply(raw, selection)
}

/// Stand-in used when the build has no Lensfun library.
#[cfg(not(lensfun_available))]
mod backend {
    use super::*;

    pub(super) fn catalog(_raw: &LoadedRaw) -> LensfunCatalog {
        LensfunCatalog {
            available: false,
            status: "Lensfun is not available in this build. Install the Lensfun development package and rebuild CalibRaw.".to_owned(),
            ..LensfunCatalog::default()
        }
    }

    pub(super) fn apply(_raw: &LoadedRaw, _selection: &LensfunLens) -> Result<LoadedRaw> {
        Err(anyhow!(
            "this build was compiled without Lensfun; lens correction is unavailable"
        ))
    }
}

#[cfg(lensfun_available)]
mod backend;
