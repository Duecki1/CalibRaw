use super::*;

/// Rows with enough images shrink to fit the viewport. Sparse rows retain the
/// selected thumbnail height rather than enlarging their images to fill space.
const THUMBNAIL_IMAGE_HORIZONTAL_INSET: f32 = 0.0;
const THUMBNAIL_IMAGE_VERTICAL_CHROME: f32 = 0.0;
const THUMBNAIL_CARD_RADIUS: f32 = moduwu_design::CARD_RADIUS;

pub(super) fn send_scan_failure(
    sender: &mpsc::SyncSender<ScanEvent>,
    generation: u64,
    error: String,
    repaint: &egui::Context,
) {
    let _ = sender.send(ScanEvent::Failed { generation, error });
    repaint.request_repaint();
}

pub(super) fn catalog_status(warning_count: usize, truncated: bool) -> String {
    let mut notices = Vec::new();
    if truncated {
        notices.push(format!("Newest {MAX_LIBRARY_FILES} photos shown"));
    }
    if warning_count > 0 {
        notices.push(format!(
            "{warning_count} unreadable {}",
            if warning_count == 1 { "item" } else { "items" }
        ));
    }
    notices.join(" · ")
}

pub(super) fn responsive_thumbnail_target_height(
    available_width: f32,
    available_height: f32,
    pixels_per_point: f32,
    android: bool,
) -> f32 {
    if android {
        return 120.0;
    }

    const REFERENCE_WIDTH: f32 = 1280.0;
    const REFERENCE_HEIGHT: f32 = 720.0;
    const BASE_HEIGHT: f32 = 140.0;

    let width = if available_width.is_finite() {
        available_width.max(1.0)
    } else {
        REFERENCE_WIDTH
    };
    let height = if available_height.is_finite() {
        available_height.max(1.0)
    } else {
        REFERENCE_HEIGHT
    };
    let viewport_scale = ((width * height) / (REFERENCE_WIDTH * REFERENCE_HEIGHT))
        .sqrt()
        .clamp(0.90, 1.70);

    let density_scale = if pixels_per_point.is_finite() {
        pixels_per_point.max(1.0).sqrt().clamp(1.0, 1.20)
    } else {
        1.0
    };

    (BASE_HEIGHT * viewport_scale * density_scale).clamp(126.0, 270.0)
}

/// Groups images in their display order. A row becomes complete only once its
/// natural width at the selected thumbnail height reaches the viewport width.
/// This leaves a short final row unscaled instead of spreading it across the
/// viewport.
pub(super) fn justified_thumbnail_row_ranges(
    aspects: &[f32],
    available_width: f32,
    target_image_height: f32,
    gap: f32,
) -> Vec<(usize, usize)> {
    if aspects.is_empty() {
        return Vec::new();
    }

    let available_width = available_width.max(1.0);
    let target_image_height = target_image_height.max(1.0);
    let gap = gap.max(0.0);
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut row_width = 0.0;

    for (index, aspect) in aspects.iter().copied().enumerate() {
        if index > start {
            row_width += gap;
        }
        row_width +=
            aspect.max(f32::EPSILON) * target_image_height + THUMBNAIL_IMAGE_HORIZONTAL_INSET;
        if row_width >= available_width {
            ranges.push((start, index + 1));
            start = index + 1;
            row_width = 0.0;
        }
    }

    if start < aspects.len() {
        ranges.push((start, aspects.len()));
    }

    ranges
}

/// Thumbnail rectangles for the entries at `indices`, in their order, and the
/// grid's height.
pub(super) fn justified_thumbnail_layout(
    entries: &[LibraryEntry],
    indices: &[usize],
    available_width: f32,
    target_height: f32,
    gap: f32,
) -> (Vec<egui::Rect>, f32) {
    justified_thumbnail_layout_from_aspects(
        indices
            .iter()
            .map(|index| library_entry_aspect(&entries[*index]))
            .collect(),
        available_width,
        target_height,
        gap,
    )
}

fn library_entry_aspect(entry: &LibraryEntry) -> f32 {
    entry
        .layout_size
        .or(entry.thumbnail_size)
        .and_then(|[width, source_height]| {
            (width > 0 && source_height > 0).then_some(width as f32 / source_height as f32)
        })
        .filter(|aspect| aspect.is_finite() && *aspect > 0.0)
        .unwrap_or(1.5)
}

fn justified_thumbnail_layout_from_aspects(
    aspects: Vec<f32>,
    available_width: f32,
    target_height: f32,
    gap: f32,
) -> (Vec<egui::Rect>, f32) {
    let available_width = available_width.max(1.0);
    let target_height = target_height.max(1.0);
    let gap = gap.max(0.0);

    let target_image_height = (target_height - THUMBNAIL_IMAGE_VERTICAL_CHROME).max(1.0);
    let row_ranges =
        justified_thumbnail_row_ranges(&aspects, available_width, target_image_height, gap);
    let mut placements = Vec::with_capacity(aspects.len());
    let mut y = 0.0;

    for (row_start, row_end) in row_ranges {
        let row_aspects = &aspects[row_start..row_end];
        let item_count = row_aspects.len();
        let aspect_sum = row_aspects.iter().sum::<f32>();
        let gaps_width = gap * item_count.saturating_sub(1) as f32;
        let image_chrome_width = THUMBNAIL_IMAGE_HORIZONTAL_INSET * item_count as f32;
        let justified_image_height = ((available_width - gaps_width - image_chrome_width).max(1.0)
            / aspect_sum.max(f32::EPSILON))
        .max(1.0);
        let row_is_justified = justified_image_height <= target_image_height;
        let image_height = if row_is_justified {
            justified_image_height
        } else {
            target_image_height
        };
        let row_height = image_height + THUMBNAIL_IMAGE_VERTICAL_CHROME;
        let mut x = 0.0;

        for (row_offset, aspect) in row_aspects.iter().copied().enumerate() {
            let width = if row_is_justified && row_offset + 1 == item_count {
                (available_width - x).max(1.0)
            } else {
                image_height * aspect + THUMBNAIL_IMAGE_HORIZONTAL_INSET
            };
            placements.push(egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(width, row_height),
            ));
            x += width + gap;
        }

        y += row_height + gap;
    }

    let total_height = if placements.is_empty() {
        0.0
    } else {
        (y - gap).max(0.0)
    };
    (placements, total_height)
}

/// A keyboard move through the thumbnail grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GridStep {
    Previous,
    Next,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
}

/// Rows share their top edge; this absorbs float noise between them.
const GRID_ROW_EPSILON: f32 = 0.5;

/// The position a step from `from` lands on; with no position to start
/// from, the first photo (or the last for End).
pub(super) fn grid_step_target(
    placements: &[egui::Rect],
    from: Option<usize>,
    step: GridStep,
    page_height: f32,
) -> Option<usize> {
    let last = placements.len().checked_sub(1)?;
    let Some(from) = from.filter(|from| *from <= last) else {
        return Some(if step == GridStep::Last { last } else { 0 });
    };
    Some(match step {
        GridStep::Previous => from.saturating_sub(1),
        GridStep::Next => (from + 1).min(last),
        GridStep::First => 0,
        GridStep::Last => last,
        GridStep::Up => grid_row_target(placements, from, false, 0.0).unwrap_or(from),
        GridStep::Down => grid_row_target(placements, from, true, 0.0).unwrap_or(from),
        GridStep::PageUp => grid_row_target(placements, from, false, page_height).unwrap_or(0),
        GridStep::PageDown => grid_row_target(placements, from, true, page_height).unwrap_or(last),
    })
}

/// The photo in the row `distance` points above or below `from`'s row
/// (the adjacent row for zero) whose centre is nearest horizontally.
fn grid_row_target(
    placements: &[egui::Rect],
    from: usize,
    below: bool,
    distance: f32,
) -> Option<usize> {
    let current = placements[from];
    let target_top = if below {
        current.top() + distance
    } else {
        current.top() - distance
    };
    let row_top = placements
        .iter()
        .map(|rect| rect.top())
        .filter(|top| {
            if below {
                *top > current.top() + GRID_ROW_EPSILON
            } else {
                *top < current.top() - GRID_ROW_EPSILON
            }
        })
        .min_by(|left, right| {
            (left - target_top)
                .abs()
                .total_cmp(&(right - target_top).abs())
        })?;
    let x = current.center().x;
    placements
        .iter()
        .enumerate()
        .filter(|(_, rect)| (rect.top() - row_top).abs() <= GRID_ROW_EPSILON)
        .min_by(|(_, left), (_, right)| {
            (left.center().x - x)
                .abs()
                .total_cmp(&(right.center().x - x).abs())
        })
        .map(|(position, _)| position)
}

pub(super) fn thumbnail_cover_uv(
    source_size: Option<[u32; 2]>,
    target_size: egui::Vec2,
) -> egui::Rect {
    let Some([width, height]) = source_size.filter(|[width, height]| *width > 0 && *height > 0)
    else {
        return egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    };
    if target_size.x <= 0.0 || target_size.y <= 0.0 {
        return egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    }

    let source_aspect = width as f32 / height as f32;
    let target_aspect = target_size.x / target_size.y;
    if !source_aspect.is_finite() || !target_aspect.is_finite() {
        return egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    }

    if source_aspect > target_aspect {
        let visible = (target_aspect / source_aspect).clamp(0.0, 1.0);
        let inset = (1.0 - visible) * 0.5;
        egui::Rect::from_min_max(egui::pos2(inset, 0.0), egui::pos2(1.0 - inset, 1.0))
    } else if source_aspect < target_aspect {
        let visible = (source_aspect / target_aspect).clamp(0.0, 1.0);
        let inset = (1.0 - visible) * 0.5;
        egui::Rect::from_min_max(egui::pos2(0.0, inset), egui::pos2(1.0, 1.0 - inset))
    } else {
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
    }
}

/// Paints a thumbnail and its badges. The grid handles the keyboard, so
/// tiles take clicks but not keyboard focus.
pub(super) fn thumbnail_tile(
    ui: &mut Ui,
    entry: &LibraryEntry,
    rect: egui::Rect,
    selected: Option<bool>,
) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("library-thumbnail-tile", entry.asset.display_path.as_str())),
        Sense::CLICK,
    );
    response.widget_info(|| match selected {
        Some(selected) => egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            &entry.asset.display_name,
        ),
        None => {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &entry.asset.display_name)
        }
    });
    let tile_rect = rect;
    let image_rect = tile_rect;
    let painter = ui.painter_at(tile_rect);
    if let Some(texture) = &entry.texture {
        let uv = thumbnail_cover_uv(entry.thumbnail_size, image_rect.size());
        painter.add(
            egui::epaint::RectShape::filled(image_rect, THUMBNAIL_CARD_RADIUS, Color32::WHITE)
                .with_texture(texture.id(), uv),
        );
    } else {
        painter.rect_filled(
            image_rect,
            THUMBNAIL_CARD_RADIUS,
            ui.visuals().extreme_bg_color,
        );
        crate::ui::components::loading_wave::paint_loading_wave(
            ui,
            image_rect,
            THUMBNAIL_CARD_RADIUS,
        );
        painter.text(
            image_rect.center(),
            Align2::CENTER_CENTER,
            if entry.thumbnail_error.is_some() {
                "Retrying preview…"
            } else if entry.thumbnail_queued {
                "Loading preview…"
            } else {
                rendered_format_label(&entry.asset.display_name).unwrap_or("RAW")
            },
            FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
    }

    let mut corner_inset = 0.0;
    if entry.developed_thumbnail_pending {
        let badge_edge = 25.0_f32.min(rect.width() * 0.32).min(rect.height() * 0.32);
        let center = egui::pos2(
            rect.right() - badge_edge * 0.5 - 6.0,
            rect.top() + badge_edge * 0.5 + 6.0,
        );
        crate::ui::components::pending_indicator(
            &painter,
            center,
            badge_edge * 0.5,
            (badge_edge * 0.72).max(12.0),
        );
        corner_inset = badge_edge + 4.0;
    }
    if entry.texture.is_some() {
        if let Some(label) = rendered_format_label(&entry.asset.display_name) {
            paint_format_badge(ui, tile_rect, label, corner_inset);
        }
    }

    response
}

/// Outlines a selected thumbnail and tints it, and rings the active one.
/// Painted over the hover details so both stay visible.
pub(super) fn paint_thumbnail_selection(ui: &Ui, rect: egui::Rect, selected: bool, active: bool) {
    let painter = ui.painter_at(rect);
    let accent = ui.visuals().selection.bg_fill;
    if selected {
        painter.rect_filled(rect, THUMBNAIL_CARD_RADIUS, accent.gamma_multiply(0.28));
        painter.rect_stroke(
            rect,
            THUMBNAIL_CARD_RADIUS,
            Stroke::new(3.0, accent),
            StrokeKind::Inside,
        );
    }
    if active {
        let inset = if selected { 3.0 } else { 0.0 };
        painter.rect_stroke(
            rect.shrink(inset),
            THUMBNAIL_CARD_RADIUS,
            Stroke::new(2.0, ui.visuals().strong_text_color()),
            StrokeKind::Inside,
        );
    }
}

pub(super) fn thumbnail_hover_details(
    asset: &LibraryAsset,
    stacked_formats: &[crate::pipeline::RenderedImageFormat],
) -> String {
    let mut format = Path::new(&asset.display_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(str::to_uppercase)
        .unwrap_or_else(|| "RAW".to_owned());
    for stacked in stacked_formats {
        format.push_str(" + ");
        format.push_str(stacked.label());
    }
    asset
        .metadata
        .dimensions_hint
        .map_or(format.clone(), |[width, height]| {
            format!("{format}  ·  {width} × {height}")
        })
}

pub(super) fn thumbnail_capture_details(asset: &LibraryAsset) -> String {
    let metadata = &asset.metadata;
    format!(
        "{}  ·  {}  ·  {}  ·  {}",
        format_thumbnail_iso(metadata.iso_speed),
        format_thumbnail_shutter(metadata.shutter_seconds),
        format_thumbnail_aperture(metadata.aperture),
        format_thumbnail_focal_length(metadata.focal_length),
    )
}

fn format_thumbnail_aperture(value: f32) -> String {
    if value.is_finite() && value > 0.0 {
        format!("f/{value:.1}")
    } else {
        "f/—".to_owned()
    }
}

fn format_thumbnail_iso(value: f32) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "ISO —".to_owned();
    }
    if (value - value.round()).abs() < 0.05 {
        format!("ISO {value:.0}")
    } else {
        format!("ISO {value:.1}")
    }
}

fn format_thumbnail_shutter(seconds: f32) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "— s".to_owned();
    }
    if seconds < 0.5 {
        let denominator = (1.0 / seconds).round().max(1.0);
        format!("1/{denominator:.0} s")
    } else if (seconds - seconds.round()).abs() < 0.05 {
        format!("{seconds:.0} s")
    } else {
        format!("{seconds:.1} s")
    }
}

fn format_thumbnail_focal_length(value: f32) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "— mm".to_owned();
    }
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0} mm")
    } else {
        format!("{value:.1} mm")
    }
}

/// The round check in a thumbnail's corner that adds it to the selection
/// or removes it, without opening the photo.
pub(super) fn thumbnail_check_circle(
    ui: &mut Ui,
    entry: &LibraryEntry,
    thumbnail_rect: egui::Rect,
    selected: bool,
) -> egui::Response {
    const HIT_EDGE: f32 = 42.0;
    const CIRCLE_EDGE: f32 = 24.0;
    const CIRCLE_INSET: f32 = 7.0;

    let hit_rect = egui::Rect::from_min_size(
        thumbnail_rect.min,
        egui::vec2(
            HIT_EDGE.min(thumbnail_rect.width()),
            HIT_EDGE.min(thumbnail_rect.height()),
        ),
    );
    let response = ui
        .interact(
            hit_rect,
            ui.make_persistent_id((
                "library-thumbnail-check-circle",
                entry.asset.display_path.as_str(),
            )),
            Sense::CLICK,
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            selected,
            format!("Select {}", entry.asset.display_name),
        )
    });
    let radius = CIRCLE_EDGE * 0.5;
    let center = thumbnail_rect.min + egui::Vec2::splat(CIRCLE_INSET + radius);
    let painter = ui.painter_at(thumbnail_rect);
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else if response.hovered() {
        Color32::from_black_alpha(200)
    } else {
        Color32::from_black_alpha(130)
    };
    painter.circle(center, radius, fill, Stroke::new(1.5, Color32::WHITE));
    if selected || response.hovered() {
        let tick = [
            center + egui::vec2(-5.0, 0.5),
            center + egui::vec2(-1.5, 4.0),
            center + egui::vec2(5.0, -3.5),
        ];
        let color = if selected {
            Color32::WHITE
        } else {
            Color32::from_white_alpha(170)
        };
        painter.add(egui::Shape::line(tick.to_vec(), Stroke::new(2.2, color)));
    }

    response.on_hover_text(if selected {
        "Deselect photo"
    } else {
        "Select photo"
    })
}
