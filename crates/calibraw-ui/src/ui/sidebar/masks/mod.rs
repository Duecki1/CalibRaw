use super::*;
use crate::app::MaskStripEdit;
use crate::i18n::tr;

/// What a strip card's thumbnail badge shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardBadge {
    /// A group card: how many sub-masks the group has.
    Count(usize),
    /// The first sub-mask, which the others combine with.
    Base,
    /// A later sub-mask: how it combines with the ones before it.
    Combine(MaskCombineMode),
}

impl CardBadge {
    fn of_component(component_index: usize, combine: MaskCombineMode) -> Self {
        if component_index == 0 {
            Self::Base
        } else {
            Self::Combine(combine)
        }
    }
}

/// Paint a badge at the bottom-right corner of a card's thumbnail. A
/// combine badge is a white Venn glyph on the mode's colour, matching the
/// buttons that create sub-masks.
fn paint_card_badge(
    painter: &egui::Painter,
    visuals: &egui::Visuals,
    image_rect: egui::Rect,
    badge: CardBadge,
    card_size: MaskCardSize,
) {
    let (font_size, badge_height, horizontal_padding) = card_size.badge_metrics();
    let text = match badge {
        CardBadge::Count(count) => count.to_string(),
        CardBadge::Base => tr("BASE").to_owned(),
        CardBadge::Combine(combine) => {
            let size = egui::vec2(badge_height + 6.0, badge_height);
            let rect = egui::Rect::from_min_size(image_rect.right_bottom() - size, size);
            let fill = crate::ui::theme::mask_combine_color(combine, visuals.dark_mode);
            painter.rect_filled(rect, 4.0, fill);
            paint_combine_glyph(
                painter,
                rect.center(),
                badge_height - 4.0,
                combine,
                egui::Color32::WHITE,
                fill,
            );
            return;
        }
    };
    let size = egui::vec2(
        (text.chars().count() as f32 * font_size * 0.62 + horizontal_padding)
            .max(badge_height + 2.0),
        badge_height,
    );
    let rect = egui::Rect::from_min_size(image_rect.right_bottom() - size, size);
    painter.rect_filled(
        rect,
        4.0,
        visuals.widgets.active.bg_fill.gamma_multiply(0.92),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(font_size),
        visuals.widgets.active.fg_stroke.color,
    );
}

/// Paint a combine mode as two overlapping squares in a `width`-point box,
/// as image editors draw selection modes: the upper-left square is the mask
/// so far, the lower-right one the new sub-mask, and the area the result
/// keeps is filled with `color`. Squares stay crisp at badge size.
///
/// `background` must be the opaque colour behind the glyph, because Subtract
/// cuts the new square out of the old one by painting it over.
fn paint_combine_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    width: f32,
    combine: MaskCombineMode,
    color: egui::Color32,
    background: egui::Color32,
) {
    // Squares offset by half a side span one and a half sides.
    let side = (width / 1.5).round();
    let offset = (side * 0.5).round();
    let origin = (center - egui::Vec2::splat((side + offset) * 0.5)).round();
    let old = egui::Rect::from_min_size(origin, egui::Vec2::splat(side));
    let new = old.translate(egui::Vec2::splat(offset));
    let corner = (side * 0.18).round();
    let outline = egui::Stroke::new((width / 14.0).clamp(1.0, 1.5), color);
    match combine {
        MaskCombineMode::Add => {
            painter.rect_filled(old, corner, color);
            painter.rect_filled(new, corner, color);
        }
        MaskCombineMode::Subtract => {
            painter.rect_filled(old, corner, color);
            painter.rect_filled(new, corner, background);
            painter.rect_stroke(new, corner, outline, egui::StrokeKind::Inside);
        }
        MaskCombineMode::Intersect => {
            painter.rect_stroke(old, corner, outline, egui::StrokeKind::Inside);
            painter.rect_stroke(new, corner, outline, egui::StrokeKind::Inside);
            painter.rect_filled(old.intersect(new), 0.0, color);
        }
    }
}

pub(super) fn mask_creation_icon() -> &'static str {
    egui_phosphor::regular::PLUS
}

/// Touch drags scroll the strip on Android, except while they carry a card:
/// then the drag moves the card and the strip scrolls at its ends.
fn mask_strip_scroll_source(dragging_card: bool) -> egui::scroll_area::ScrollSource {
    let source = if crate::platform::PlatformCaps::CURRENT.is_mobile() {
        egui::scroll_area::ScrollSource::ALL
    } else {
        egui::scroll_area::ScrollSource::default()
    };
    if dragging_card {
        egui::scroll_area::ScrollSource {
            drag: egui::scroll_area::DragScroll::Never,
            ..source
        }
    } else {
        source
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

/// The card a strip drag carries. A drag moves either a group or a sub-mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StripDragSource {
    Group(usize),
    Component {
        mask_index: usize,
        component_index: usize,
    },
}

/// Where a released drag lands, as insertion indices before the move. A
/// group drag only targets groups and a sub-mask drag only sub-masks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StripDropTarget {
    /// Insertion index in the stack.
    Group(usize),
    Component {
        mask_index: usize,
        insert: usize,
    },
}

#[derive(Clone)]
struct StripDragState {
    source: StripDragSource,
    source_texture: Option<egui::TextureHandle>,
    source_name: String,
    source_badge: CardBadge,
    source_enabled: bool,
    /// The group a dragged sub-mask rests on, to open it after a delay.
    hover_group: Option<(usize, std::time::Instant)>,
    drop_target: Option<StripDropTarget>,
    target_loss_started: Option<std::time::Instant>,
}

impl StripDragState {
    fn card_size(&self) -> MaskCardSize {
        match self.source {
            StripDragSource::Group(_) => MaskCardSize::Group,
            StripDragSource::Component { .. } => MaskCardSize::Submask,
        }
    }
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
mod kind_picker;
mod menus;
mod properties;
mod strip;
mod thumbnails;
