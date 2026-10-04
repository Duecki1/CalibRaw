use super::*;
use crate::app::MaskStripEdit;

pub(super) fn mask_component_badge(
    component_index: usize,
    combine: MaskCombineMode,
) -> &'static str {
    if component_index == 0 {
        "BASE"
    } else {
        match combine {
            MaskCombineMode::Add => egui_phosphor::regular::PLUS,
            MaskCombineMode::Subtract => egui_phosphor::regular::MINUS,
            MaskCombineMode::Intersect => egui_phosphor::regular::INTERSECT,
        }
    }
}

pub(super) fn mask_creation_icon() -> &'static str {
    egui_phosphor::regular::PLUS
}

fn mask_strip_scroll_source() -> egui::scroll_area::ScrollSource {
    if cfg!(target_os = "android") {
        egui::scroll_area::ScrollSource::ALL
    } else {
        egui::scroll_area::ScrollSource::default()
    }
}

#[derive(Clone, Debug)]
enum MaskRenameTarget {
    Group(usize),
    Component {
        mask_index: usize,
        component_index: usize,
    },
}

#[derive(Clone, Debug)]
struct MaskRenameDialog {
    target: MaskRenameTarget,
    name: String,
    focus_requested: bool,
}

#[derive(Clone, Debug)]
enum MaskDeleteTarget {
    Group(usize),
    Component {
        mask_index: usize,
        component_index: usize,
    },
}

#[derive(Clone, Debug)]
struct MaskDeleteDialog {
    target: MaskDeleteTarget,
    name: String,
}

#[derive(Clone)]
struct SubmaskDragState {
    source_mask: usize,
    source_component: usize,
    source_texture: Option<egui::TextureHandle>,
    source_name: String,
    source_badge: String,
    source_enabled: bool,
    hover_group: Option<(usize, std::time::Instant)>,
    drop_target: Option<(usize, usize)>,
    target_loss_started: Option<std::time::Instant>,
}

/// Requests collected while drawing the mask strip; the strip turns them
/// into `MaskStripActions` after the cards.
#[derive(Default)]
struct MaskStripRequests {
    edits: Vec<MaskStripEdit>,
    select_mask: Option<usize>,
    select_component: Option<usize>,
    new_mask: Option<MaskKind>,
    add_component: Option<(MaskKind, MaskCombineMode)>,
    duplicate_mask: Option<(usize, bool)>,
    paste_mask: Option<usize>,
    duplicate_component: Option<(usize, usize, bool)>,
    paste_component: Option<(usize, usize)>,
}

/// The sub-mask a context menu acts on and what it allows.
#[derive(Clone, Copy)]
struct SubmaskMenu {
    mask_index: usize,
    component_index: usize,
    can_delete: bool,
    can_add_component: bool,
}

mod adjustments;
mod details;
mod menus;
mod properties;
mod strip;
mod thumbnails;
