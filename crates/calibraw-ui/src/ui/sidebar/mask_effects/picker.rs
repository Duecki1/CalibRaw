//! Adding effects: the Add effect button and the picker it opens, which shows
//! every effect as an icon tile under its category.

use super::{add_effect_buttons, effect_description, egui, MaskEffect, Ui};
use crate::pipeline::{EffectComponent, MaskEffectCategory, MAX_EFFECT_COMPONENTS};

const TILES_PER_ROW: usize = 3;
/// Scales with the platform's control height, so tiles stay touch-sized.
const TILE_HEIGHT: f32 = moduwu_design::CONTROL_HEIGHT * 1.6;
const TILE_ICON_SIZE: f32 = 20.0;
const TILE_LABEL_SIZE: f32 = 11.5;
/// The picker's width when its button is narrower, as the touch Add tab is.
const PICKER_MIN_WIDTH: f32 = 264.0;

/// The add controls of an Effects card: a quick-add button per effect in
/// `featured`, then [`add_effect_button`]. Returns the effect to add to
/// `components`.
pub(in crate::ui::sidebar) fn add_effect_controls(
    ui: &mut Ui,
    featured: &[MaskEffect],
    components: &[EffectComponent],
) -> Option<MaskEffect> {
    let featured_added = add_effect_buttons(ui, featured, components);
    ui.add_space(moduwu_design::SPACE_XS);
    let picked = add_effect_button(ui, components);
    featured_added.or(picked)
}

/// A full-width Add effect button that opens [`effect_picker_popup`].
/// Disabled once `components` is full. Returns the picked effect.
pub(in crate::ui::sidebar) fn add_effect_button(
    ui: &mut Ui,
    components: &[EffectComponent],
) -> Option<MaskEffect> {
    let full = components.len() >= MAX_EFFECT_COMPONENTS;
    let response = ui
        .add_enabled_ui(!full, |ui| {
            moduwu_design::full_width_button(
                ui,
                format!("{}  Add effect", egui_phosphor::regular::PLUS),
            )
        })
        .inner
        .on_disabled_hover_text(format!(
            "At most {MAX_EFFECT_COMPONENTS} effects can be added"
        ));
    effect_picker_popup(&response, components)
}

/// Opens [`effect_picker`] when `button` is clicked, as wide as the button
/// and at least [`PICKER_MIN_WIDTH`]. It opens on the side of the button with
/// more room, and scrolls if it is taller than that room. Returns the picked
/// effect.
pub(in crate::ui::sidebar) fn effect_picker_popup(
    button: &egui::Response,
    components: &[EffectComponent],
) -> Option<MaskEffect> {
    let screen = button.ctx.content_rect();
    let gap = moduwu_design::SPACE_XS;
    let room_above = button.rect.top() - screen.top();
    let room_below = screen.bottom() - button.rect.bottom();
    // Chosen here rather than by egui, which learns the popup's height only
    // after showing it once and would move it on the second frame.
    let align = if room_below >= room_above {
        egui::RectAlign::BOTTOM_START
    } else {
        egui::RectAlign::TOP_START
    };
    let size = egui::vec2(
        button.rect.width().max(PICKER_MIN_WIDTH),
        room_above.max(room_below) - 2.0 * gap,
    );
    // Not a menu: menus draw idle buttons flat, and the tiles keep their frame.
    egui::Popup::from_toggle_button_response(button)
        .align(align)
        .align_alternatives(&[])
        .gap(gap)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| effect_picker(ui, components, size))
        .and_then(|response| response.inner)
}

/// Every effect as a tile under its category's heading, filling a popup of
/// at most `size`, to add one to `components`. An effect already in
/// `components` shows a check mark and is disabled; once `components` is
/// full, every effect is disabled. Returns the clicked effect and closes the
/// popup.
fn effect_picker(
    ui: &mut Ui,
    components: &[EffectComponent],
    size: egui::Vec2,
) -> Option<MaskEffect> {
    let full = components.len() >= MAX_EFFECT_COMPONENTS;
    // The popup frame's margin and border lie outside the contents.
    let size = size
        - ui.spacing().menu_margin.sum()
        - egui::Vec2::splat(2.0 * ui.visuals().window_stroke.width);
    ui.set_width(size.x);
    // The sidebars' solid scrollbar, so a picker taller than its room shows
    // that more effects follow.
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 7.0;
    scroll.bar_inner_margin = 7.0;
    ui.spacing_mut().scroll = scroll;
    let gap = moduwu_design::SPACE_XS;
    let mut picked = None;
    // A popup offers its contents only the height it had the frame before,
    // which would keep the scroll area at its first height. The minimum lets
    // it grow to its contents, up to `size.y`.
    egui::ScrollArea::vertical()
        .max_height(size.y)
        .min_scrolled_height(size.y)
        .show(ui, |ui| {
            // Narrower than the popup while the scrollbar shows.
            let tile_size = egui::vec2(
                (ui.available_width() - gap * (TILES_PER_ROW - 1) as f32) / TILES_PER_ROW as f32,
                TILE_HEIGHT,
            );
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for (index, category) in MaskEffectCategory::ALL.into_iter().enumerate() {
                if index > 0 {
                    ui.add_space(moduwu_design::SPACE_SM);
                }
                category_heading(ui, category.label());
                let effects: Vec<_> = MaskEffect::ALL
                    .into_iter()
                    .filter(|effect| effect.category() == Some(category))
                    .collect();
                for row in effects.chunks(TILES_PER_ROW) {
                    ui.horizontal(|ui| {
                        for &effect in row {
                            let added = components
                                .iter()
                                .any(|component| component.effect == effect);
                            let mut response =
                                effect_tile(ui, effect, tile_size, added, !added && !full)
                                    .on_disabled_hover_text(if added {
                                        format!("{} is already added", effect.label())
                                    } else {
                                        format!(
                                            "At most {MAX_EFFECT_COMPONENTS} effects can be added"
                                        )
                                    });
                            if let Some(help) = effect_description(effect) {
                                response = response.on_hover_text(help);
                            }
                            if response.clicked() {
                                picked = Some(effect);
                            }
                        }
                    });
                }
            }
        });
    if picked.is_some() {
        ui.close();
    }
    picked
}

/// A category's name followed by a hairline to the right edge. Painted into
/// one rect of the text's height: a row of widgets lays out at another height
/// in the popup's sizing pass, which would misplace a popup opening upwards.
fn category_heading(ui: &mut Ui, label: &str) {
    let galley = egui::WidgetText::from(egui::RichText::new(label).small()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    let line_start = rect.left() + galley.size().x + ui.spacing().item_spacing.x;
    let painter = ui.painter();
    painter.hline(
        line_start..=rect.right(),
        rect.center().y,
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    painter.galley(rect.left_top(), galley, ui.visuals().weak_text_color());
}

/// A button showing `effect`'s icon above its name, framed like the
/// quick-add buttons. An `added` effect shows a check mark and reports itself
/// selected to assistive technology.
fn effect_tile(
    ui: &mut Ui,
    effect: MaskEffect,
    size: egui::Vec2,
    added: bool,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                ui.is_enabled(),
                added,
                effect.label(),
            )
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let visuals = moduwu_design::interaction_visuals(ui, &response, false);
        let painter = ui.painter();
        painter.rect(
            rect,
            moduwu_design::CARD_RADIUS,
            visuals.weak_fill,
            visuals.stroke,
            egui::StrokeKind::Inside,
        );

        let padding = ui.spacing().button_padding.x;
        let label =
            egui::WidgetText::from(egui::RichText::new(effect.label()).size(TILE_LABEL_SIZE))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    rect.width() - 2.0 * padding,
                    egui::TextStyle::Button,
                );
        // The icon and the name are centred as one block.
        let icon_gap = moduwu_design::SPACE_XXS;
        let top = rect.center().y - (TILE_ICON_SIZE + icon_gap + label.size().y) * 0.5;
        painter.text(
            egui::pos2(rect.center().x, top + TILE_ICON_SIZE * 0.5),
            egui::Align2::CENTER_CENTER,
            effect_icon(effect),
            egui::FontId::proportional(TILE_ICON_SIZE),
            visuals.foreground,
        );
        painter.galley(
            egui::pos2(
                rect.center().x - label.size().x * 0.5,
                top + TILE_ICON_SIZE + icon_gap,
            ),
            label,
            visuals.foreground,
        );
        if added {
            painter.text(
                rect.right_top() + egui::vec2(-padding, padding),
                egui::Align2::RIGHT_TOP,
                egui_phosphor::regular::CHECK,
                egui::FontId::proportional(TILE_LABEL_SIZE),
                visuals.foreground,
            );
        }
        response
    })
    .inner
}

fn effect_icon(effect: MaskEffect) -> &'static str {
    use egui_phosphor::regular;

    match effect {
        MaskEffect::Adjustment => regular::SLIDERS_HORIZONTAL,
        MaskEffect::Blur => regular::DROP_HALF,
        MaskEffect::LensBlur => regular::APERTURE,
        MaskEffect::MotionBlur => regular::ARROW_FAT_LINES_RIGHT,
        MaskEffect::RadialBlur => regular::SPIRAL,
        MaskEffect::Sharpen => regular::TRIANGLE,
        MaskEffect::TiltShift => regular::ARROWS_IN_LINE_VERTICAL,
        MaskEffect::Glow => regular::SUN,
        MaskEffect::LightRays => regular::SUN_HORIZON,
        MaskEffect::Neon => regular::LIGHTNING,
        MaskEffect::Relight => regular::LIGHTBULB,
        MaskEffect::EdgeGlow => regular::SCRIBBLE,
        MaskEffect::Pixelate => regular::CHECKERBOARD,
        MaskEffect::Fog => regular::CLOUD_FOG,
        MaskEffect::Smoke => regular::WIND,
        MaskEffect::Grain => regular::DOTS_NINE,
        MaskEffect::Halation => regular::CIRCLE_DASHED,
        MaskEffect::Vignette => regular::VIGNETTE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => {
                    Some(egui::Rect::from_min_size(shape.pos, shape.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
                _ => None,
            }
        }
        shapes.iter().find_map(|shape| find(&shape.shape, text))
    }

    /// A 280 px sidebar column drawing `contents`, which returns what it
    /// changed.
    struct Column<T> {
        ctx: egui::Context,
        time: f64,
        contents: Box<dyn FnMut(&mut Ui) -> T>,
    }

    impl<T: Default> Column<T> {
        fn new(contents: impl FnMut(&mut Ui) -> T + 'static) -> Self {
            let ctx = egui::Context::default();
            crate::ui::theme::install(&ctx);
            Self {
                ctx,
                time: 0.0,
                contents: Box::new(contents),
            }
        }

        fn render(&mut self, events: Vec<egui::Event>) -> (Vec<egui::epaint::ClippedShape>, T) {
            self.time += 0.25;
            let mut result = T::default();
            let output = self.ctx.run_ui(
                egui::RawInput {
                    time: Some(self.time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(320.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(280.0);
                    result = (self.contents)(ui);
                },
            );
            (output.shapes, result)
        }

        /// Clicks at `position` and returns what the release changed.
        fn click(&mut self, position: egui::Pos2) -> T {
            let events = |pressed| {
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            self.render(events(true));
            self.render(events(false)).1
        }

        /// Clicks the Add effect button and returns where the picker shows
        /// `effect`'s name, with the button's label rect.
        fn open_picker(&mut self, effect: MaskEffect) -> Option<(egui::Rect, egui::Rect)> {
            let add_label = format!("{}  Add effect", egui_phosphor::regular::PLUS);
            let (shapes, _) = self.render(Vec::new());
            let button = text_rect(&shapes, &add_label).expect("Add effect button");
            self.click(button.center());
            // A new popup lays itself out once before it is drawn.
            (0..3)
                .find_map(|_| text_rect(&self.render(Vec::new()).0, effect.label()))
                .map(|tile| (tile, button))
        }

        fn pick(&mut self, effect: MaskEffect) -> T {
            let (tile, _) = self
                .open_picker(effect)
                .unwrap_or_else(|| panic!("the picker shows {}", effect.label()));
            self.click(tile.center())
        }
    }

    fn controls(components: Vec<EffectComponent>) -> Column<Option<MaskEffect>> {
        Column::new(move |ui| add_effect_controls(ui, &[MaskEffect::Grain], &components))
    }

    #[test]
    fn add_effect_shows_every_effect_and_adds_only_missing_ones() {
        let mut column = controls(vec![EffectComponent::new(MaskEffect::Glow)]);
        for effect in MaskEffect::ALL
            .into_iter()
            .filter(|effect| effect.category().is_some() && *effect != MaskEffect::Glow)
        {
            assert_eq!(column.pick(effect), Some(effect), "{}", effect.label());
        }
        // Glow is already added.
        assert_eq!(column.pick(MaskEffect::Glow), None);
    }

    #[test]
    fn picker_opens_below_its_button_and_spans_it() {
        let mut column = controls(Vec::new());
        // Blur starts the first row, Motion Blur ends it.
        let (first, button) = column.open_picker(MaskEffect::Blur).expect("picker");
        let last = text_rect(&column.render(Vec::new()).0, MaskEffect::MotionBlur.label())
            .expect("Motion Blur tile");
        assert!(first.top() > button.bottom());
        // `button` is the label's rect; the button spans the 280 px column.
        assert!(first.left() < 280.0 / TILES_PER_ROW as f32);
        assert!(last.right() > 280.0 * 2.0 / TILES_PER_ROW as f32);
        assert!(last.right() < 280.0);
    }

    #[test]
    fn picker_opens_above_a_button_near_the_window_bottom() {
        let mut column = Column::new(|ui| {
            ui.add_space(760.0);
            add_effect_button(ui, &[])
        });
        let (tile, button) = column.open_picker(MaskEffect::Vignette).expect("picker");
        assert!(tile.bottom() < button.top());
        // Every row fits above, so the picker does not scroll.
        assert_eq!(column.click(tile.center()), Some(MaskEffect::Vignette));
    }

    #[test]
    fn add_effect_is_disabled_once_the_list_is_full() {
        let mut column = controls(vec![
            EffectComponent::new(MaskEffect::Blur);
            MAX_EFFECT_COMPONENTS
        ]);
        assert!(column.open_picker(MaskEffect::LensBlur).is_none());
    }

    #[test]
    fn the_add_effect_button_below_the_effect_cards_adds_to_them() {
        use std::{cell::RefCell, rc::Rc};

        let components = Rc::new(RefCell::new(vec![EffectComponent::new(MaskEffect::Fog)]));
        let shown = Rc::clone(&components);
        let frame = super::super::EffectFrame::uncropped(3000, 2000);
        let mut column = Column::new(move |ui| {
            super::super::super::Sidebar::show_effect_components(
                ui,
                &mut shown.borrow_mut(),
                true,
                &frame,
            )
        });
        assert!(column.pick(MaskEffect::Blur));
        let effects: Vec<_> = components
            .borrow()
            .iter()
            .map(|component| component.effect)
            .collect();
        assert_eq!(effects, [MaskEffect::Fog, MaskEffect::Blur]);
    }
}
