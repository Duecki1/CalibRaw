//! The open document's develop state and the develop view's interaction state.

use super::*;

pub(crate) struct DevelopReferenceState {
    pub(crate) path: Option<PathBuf>,
    pub(crate) label: Option<String>,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) texture_size: Option<[u32; 2]>,
    pub(crate) high_quality: bool,
    pub(crate) loading_path: Option<PathBuf>,
    pub(crate) preview_receiver: Option<mpsc::Receiver<(PathBuf, Result<RawThumbnail, String>)>>,
    pub(crate) error: Option<String>,
    pub(crate) split_ratio: f32,
}

impl Default for DevelopReferenceState {
    fn default() -> Self {
        Self {
            path: None,
            label: None,
            texture: None,
            texture_size: None,
            high_quality: false,
            loading_path: None,
            preview_receiver: None,
            error: None,
            split_ratio: 0.5,
        }
    }
}

impl DevelopReferenceState {
    pub(crate) fn clear(&mut self) {
        *self = Self {
            split_ratio: self.split_ratio,
            ..Self::default()
        };
    }
}

type DevelopThumbnailResult = (PathBuf, Result<Option<RawThumbnail>, String>);

#[derive(Default)]
pub(crate) struct DevelopLoadingThumbnailState {
    /// The folder photo the thumbnail shows.
    pub(crate) path: Option<PathBuf>,
    /// The media-library photo the thumbnail shows.
    pub(crate) source_uri: Option<String>,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) texture_size: Option<[u32; 2]>,
    pub(crate) receiver: Option<mpsc::Receiver<DevelopThumbnailResult>>,
}

impl DevelopLoadingThumbnailState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SidebarTab {
    #[default]
    Adjustments,
    Presets,
    Crop,
    Masks,
    Inpainting,
    Export,
    Info,
}

impl SidebarTab {
    /// Tabs in navigation order.
    pub(crate) const ALL: [Self; 7] = [
        Self::Adjustments,
        Self::Presets,
        Self::Crop,
        Self::Masks,
        Self::Inpainting,
        Self::Export,
        Self::Info,
    ];

    /// The sidebar header title.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Adjustments => "Edit",
            Self::Presets => "Presets",
            Self::Crop => "Crop",
            Self::Masks => "Masking",
            Self::Inpainting => "Inpaint",
            Self::Export => "Export",
            Self::Info => "Image Info",
        }
    }

    /// The label under the icon in compact navigation.
    pub(crate) const fn short_label(self) -> &'static str {
        match self {
            Self::Adjustments => "Edit",
            Self::Presets => "Presets",
            Self::Crop => "Crop",
            Self::Masks => "Mask",
            Self::Inpainting => "Remove",
            Self::Export => "Export",
            Self::Info => "Info",
        }
    }

    pub(crate) const fn tooltip(self) -> &'static str {
        match self {
            Self::Adjustments => "Edit adjustments",
            Self::Presets => "Presets",
            Self::Crop => "Crop",
            Self::Masks => "Masking",
            Self::Inpainting => "Remove unwanted objects",
            Self::Export => "Export",
            Self::Info => "Image information",
        }
    }

    pub(crate) const fn glyph(self) -> &'static str {
        use egui_phosphor::regular;
        match self {
            Self::Adjustments => regular::SLIDERS_HORIZONTAL,
            Self::Presets => regular::PALETTE,
            Self::Crop => regular::CROP,
            Self::Masks => regular::SELECTION,
            Self::Inpainting => regular::BANDAIDS,
            Self::Export => regular::EXPORT,
            Self::Info => regular::INFO,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InpaintTool {
    #[default]
    Remove,
    Clone,
    Heal,
}

impl InpaintTool {
    pub(crate) const ALL: [Self; 3] = [Self::Remove, Self::Clone, Self::Heal];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Remove => "Remove",
            Self::Clone => "Clone",
            Self::Heal => "Heal",
        }
    }

    pub(crate) const fn retouch(self) -> Option<RetouchTool> {
        match self {
            Self::Remove => None,
            Self::Clone => Some(RetouchTool::Clone),
            Self::Heal => Some(RetouchTool::Heal),
        }
    }

    pub(crate) const fn matches_stroke_tool(self, retouch: Option<RetouchTool>) -> bool {
        matches!(
            (self, retouch),
            (Self::Remove, None)
                | (Self::Clone, Some(RetouchTool::Clone))
                | (Self::Heal, Some(RetouchTool::Heal))
        )
    }
}

/// What a drag over the photo does in the Crop tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CropTool {
    /// Moves and resizes the crop.
    #[default]
    Crop,
    /// Draws a line along a horizon or vertical edge to level the photo.
    Straighten,
    /// Draws and reshapes perspective guides.
    Guides,
}

/// What a drag over the photo samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AreaPicker {
    /// A neutral area, for the white balance.
    WhiteBalance,
    /// Unexposed film, for the base of a film negative.
    FilmBase,
}

impl AreaPicker {
    /// The vertical layout's section that holds the picker's card.
    pub(crate) const fn section(self) -> AdjustmentSection {
        match self {
            Self::WhiteBalance => AdjustmentSection::Color,
            // The Film negative card leads the Light section.
            Self::FilmBase => AdjustmentSection::Light,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AdjustmentSection {
    #[default]
    Light,
    ToneCurve,
    /// Shown only when the LUT card is (`Sidebar::look_lut_card_shown`).
    Lut,
    Color,
    ColorGrading,
    Detail,
    Effects,
    ColorMixer,
    Optics,
}

impl AdjustmentSection {
    /// The section that shows `card` in the vertical layout, which orders its
    /// sections as the cards are ordered. The camera profile has none: the
    /// Light section shows it.
    pub(crate) const fn of_card(card: DevelopCard) -> Option<Self> {
        match card {
            DevelopCard::CameraProfile => None,
            DevelopCard::Light => Some(Self::Light),
            DevelopCard::ToneCurve => Some(Self::ToneCurve),
            DevelopCard::Lut => Some(Self::Lut),
            DevelopCard::Color => Some(Self::Color),
            DevelopCard::ColorGrading => Some(Self::ColorGrading),
            DevelopCard::Detail => Some(Self::Detail),
            DevelopCard::Effects => Some(Self::Effects),
            DevelopCard::ColorMixer => Some(Self::ColorMixer),
            DevelopCard::LensCorrections => Some(Self::Optics),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MaskSection {
    #[default]
    Properties,
    Light,
    ToneCurve,
    Color,
    ColorGrading,
    Effects,
    ColorMixer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ToneCurveTab {
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ColorGradeTab {
    Shadows,
    #[default]
    Midtones,
    Highlights,
    Global,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum HslMixerColor {
    #[default]
    Red,
    Orange,
    Yellow,
    Green,
    Aqua,
    Blue,
    Purple,
    Magenta,
}

impl HslMixerColor {
    pub(crate) const ALL: [Self; 8] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Aqua,
        Self::Blue,
        Self::Purple,
        Self::Magenta,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LensCorrectionState {
    pub enabled: bool,
    pub applied: bool,
    pub corrections: LensfunCorrections,
    pub catalog: LensfunCatalog,
    pub selected_maker: String,
    pub selected_model: String,
    /// Hand-set corrections used instead of the selected profile.
    pub manual: Option<ManualLensCorrection>,
}

/// The Settings choice for lens correction on photos with no saved edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutomaticLensCorrection {
    /// Apply the profile matched from the RAW metadata when a photo opens.
    pub enabled: bool,
    /// Which parts of that profile the automatic correction (and a card
    /// reset) uses.
    pub corrections: LensfunCorrections,
}

/// Matches the Settings defaults: geometry on, vignetting off.
impl Default for AutomaticLensCorrection {
    fn default() -> Self {
        Self {
            enabled: true,
            corrections: LensfunCorrections {
                geometry: true,
                vignetting: false,
            },
        }
    }
}

impl LensCorrectionState {
    /// The state of a photo with no saved lens edits: the auto-matched profile,
    /// applied only when `automatic` allows it. The match is still selected
    /// when it is not applied so the user can enable it from the card.
    pub(crate) fn automatic(catalog: LensfunCatalog, automatic: AutomaticLensCorrection) -> Self {
        let selected = catalog.auto_match.as_ref();
        Self {
            enabled: automatic.enabled && catalog.available && selected.is_some(),
            applied: false,
            corrections: automatic.corrections,
            selected_maker: selected
                .as_ref()
                .map(|lens| lens.maker.clone())
                .unwrap_or_default(),
            selected_model: selected
                .as_ref()
                .map(|lens| lens.model.clone())
                .unwrap_or_default(),
            catalog,
            manual: None,
        }
    }

    /// Switches to hand-set corrections, starting neutral and turned on, or
    /// back to the selected profile, which stays on only if one is selected.
    pub(crate) fn set_manual_mode(&mut self, manual: bool) {
        if manual {
            self.manual = Some(ManualLensCorrection::default());
            self.enabled = true;
        } else {
            self.manual = None;
            self.enabled &= self.catalog.available && self.selected_lens().is_some();
        }
    }

    /// The correction the photo should show; `None` shows the original
    /// geometry. Manual mode at its neutral values corrects nothing.
    pub(crate) fn requested_correction(&self) -> Option<LensCorrection> {
        if !self.enabled {
            return None;
        }
        match self.manual {
            Some(manual) => (!manual.is_identity()).then_some(LensCorrection::Manual(manual)),
            None => self.selected_lens().map(LensCorrection::Profile),
        }
    }

    pub(crate) fn selected_lens(&self) -> Option<LensfunLens> {
        (!self.selected_model.trim().is_empty()).then(|| LensfunLens {
            maker: self.selected_maker.clone(),
            model: self.selected_model.clone(),
            corrections: self.corrections,
        })
    }

    pub(crate) fn makers(&self) -> Vec<String> {
        let mut makers = self
            .catalog
            .lenses
            .iter()
            .map(|lens| lens.maker.clone())
            .collect::<Vec<_>>();
        makers.sort_by_key(|maker| maker.to_lowercase());
        makers.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        makers
    }

    pub(crate) fn models_for_maker(&self, maker: &str) -> Vec<String> {
        let mut models = self
            .catalog
            .lenses
            .iter()
            .filter(|lens| lens.maker.eq_ignore_ascii_case(maker))
            .map(|lens| lens.model.clone())
            .collect::<Vec<_>>();
        models.sort_by_key(|model| model.to_lowercase());
        models.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        models
    }
}

pub(crate) struct LoadFailure {
    pub(super) label: String,
    pub(super) message: String,
    pub(super) unsupported: bool,
}

pub(crate) enum LoadEvent {
    Finished(Result<LoadedPreview, LoadFailure>),
}

pub(super) struct PreparedLensCorrection {
    pub(super) full_raw: Arc<LoadedRaw>,
    pub(super) preview_raw: Arc<LoadedRaw>,
    pub(super) applied_label: Option<String>,
    pub(super) selection: Option<LensCorrection>,
    pub(super) preview_quality: PreviewQuality,
}

pub(super) enum LensCorrectionEvent {
    Progress(String),
    Finished(Result<PreparedLensCorrection, String>),
}

pub(crate) const MAX_DESKTOP_RAW_CACHE_FILES: usize = 8;
pub(crate) const MAX_ANDROID_RAW_CACHE_FILES: usize = 3;

/// Desktop keeps the open photo and both prefetched neighbours.
pub(crate) const fn default_raw_cache_limit() -> usize {
    if PlatformCaps::CURRENT.constrained_memory() {
        1
    } else {
        3
    }
}

pub(crate) const fn maximum_raw_cache_limit() -> usize {
    if PlatformCaps::CURRENT.constrained_memory() {
        MAX_ANDROID_RAW_CACHE_FILES
    } else {
        MAX_DESKTOP_RAW_CACHE_FILES
    }
}

pub(crate) struct DevelopState {
    pub(crate) current_path: Option<PathBuf>,
    pub(crate) review: crate::sidecar::PhotoReview,
    pub(crate) original_raw: Option<Arc<LoadedRaw>>,
    pub(crate) loaded_raw: Option<Arc<LoadedRaw>>,
    pub(crate) preview_raw: Option<Arc<LoadedRaw>>,
    pub(crate) exposure: ExposureParams,
    pub(crate) target_exposure: ExposureParams,
    pub(crate) geometry: GeometryTransform,
    pub(crate) geometry_revision: u64,
    pub(crate) lens_correction: LensCorrectionState,
    pub(crate) lens_correction_dirty: bool,
    pub(crate) selected_camera_profile: Option<PathBuf>,
    /// The edit's LUT.
    pub(crate) look_lut: Option<crate::luts::LookLut>,
    /// The table of `look_lut` once read; `None` while it is read or when the
    /// LUT library does not hold it (`luts.rs`).
    pub(crate) look_lut_table: Option<Arc<crate::luts::CubeLut>>,
    /// The LUT the previews render, kept like `target_exposure`.
    pub(crate) target_look: Option<crate::luts::ResolvedLook>,
    /// Another camera profile's colour the shown previews render instead of
    /// the photo's own: a previewed profile.
    pub(crate) target_profile_color: Option<Arc<crate::pipeline::ProfileColor>>,
    pub(crate) profile_preview: crate::app::lifecycle::CameraProfilePreview,
    pub(crate) load_receiver: Option<mpsc::Receiver<LoadEvent>>,
    pub(crate) loading_label: Option<String>,
    /// The view the loading document opens at; `None` fits it.
    pub(crate) loading_view: Option<crate::app::KeptView>,
    pub(crate) image_status: String,
    pub(crate) current_label: Option<String>,
    pub(crate) decoded_raws: DecodedRawCache,
    pub(crate) neighbour_prefetch: crate::app::lifecycle::NeighbourPrefetch,
}

pub(crate) struct DevelopUiState {
    pub(crate) histogram_open: bool,
    pub(crate) reference: DevelopReferenceState,
    pub(crate) loading_thumbnail: DevelopLoadingThumbnailState,
    pub(crate) filmstrip_open: bool,
    pub(crate) filmstrip_centered_path: Option<PathBuf>,
    /// Whether the Develop tool surface is shown. Selecting the active sidebar
    /// tab again toggles it; selecting another tab opens it.
    pub(crate) sidebar_open: bool,
    pub(crate) crop_constraint_reference: Option<[f32; 4]>,
    pub(crate) crop_drag: Option<CropDragState>,
    pub(crate) crop_tool: CropTool,
    pub(crate) straighten_drag: Option<StraightenDragState>,
    /// The guided perspective correction's lines on the open photo. They are not saved, and are
    /// kept until another photo opens or the transform is reset.
    pub(crate) perspective_guides: Vec<PerspectiveGuide>,
    pub(crate) guide_drag: Option<GuideDrag>,
    /// The value a drag over the photo samples, while one is being picked.
    pub(crate) area_picker: Option<AreaPicker>,
    /// The area dragged so far, in normalized source coordinates.
    pub(crate) area_picker_drag: Option<[[f32; 2]; 2]>,
    pub(crate) adjustment_section: AdjustmentSection,
    /// Unfold and scroll to the Lens Corrections card when the sidebar next
    /// draws it.
    pub(crate) reveal_lens_card: bool,
    pub(crate) manual_lens_dialog: Option<ManualLensDialog>,
    pub(crate) mask_section: MaskSection,
    pub(crate) effect_component: Option<crate::pipeline::MaskEffect>,
    pub(crate) mask_effect_component: Option<crate::pipeline::MaskEffect>,
    pub(crate) mask_effect_mask: Option<usize>,
    pub(crate) tone_curve_tab: ToneCurveTab,
    pub(crate) color_grade_tab: ColorGradeTab,
    pub(crate) hsl_mixer_color: HslMixerColor,
    pub(crate) point_color: crate::ui::components::point_color::PointColorUiState,
    pub(crate) point_color_tab: bool,
    pub(crate) mask_point_color: crate::ui::components::point_color::PointColorUiState,
    pub(crate) mask_point_color_tab: bool,
    pub(crate) mask_point_color_mask: Option<usize>,
}

/// The open manual lens-correction dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ManualLensDialog {
    /// The values being adjusted; nothing is corrected until they are applied.
    pub(crate) draft: ManualLensCorrection,
    /// The manual correction already in the preview pixels. The live preview
    /// shows the draft relative to it.
    pub(crate) shown: ManualLensCorrection,
}

impl DevelopUiState {
    /// Ends any drag of the crop, the straighten line or a guide.
    pub(crate) fn cancel_crop_drags(&mut self) {
        self.crop_drag = None;
        self.straighten_drag = None;
        self.guide_drag = None;
    }

    /// Switches the Crop tab's preview to `tool`, ending any drag.
    pub(crate) fn set_crop_tool(&mut self, tool: CropTool) {
        self.crop_tool = tool;
        self.cancel_crop_drags();
    }

    pub(crate) fn cancel_area_picker(&mut self) {
        self.area_picker = None;
        self.area_picker_drag = None;
    }

    /// Starts or stops picking `picker`. Starting one ends any other.
    pub(crate) fn set_area_picker(&mut self, picker: AreaPicker, active: bool) {
        if active && self.area_picker != Some(picker) {
            self.area_picker = Some(picker);
            self.area_picker_drag = None;
        } else if !active && self.area_picker == Some(picker) {
            self.cancel_area_picker();
        }
    }

    /// Stops picking global point colors and hides their range overlay.
    pub(crate) fn cancel_point_color_preview(&mut self) {
        self.point_color.picker_active = false;
        self.point_color.visualize_range = false;
    }
}
