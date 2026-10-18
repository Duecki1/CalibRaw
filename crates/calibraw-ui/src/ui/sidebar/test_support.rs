//! Drives sidebar controls in tests: draws them, clicks them and finds their
//! text.

use super::{egui, MaskEffect, Ui};

/// The label of the Add effect buttons.
pub(super) fn add_effect_label() -> String {
    format!("{}  Add effect", egui_phosphor::regular::PLUS)
}

/// Where each text shape reading exactly `text` is drawn, in drawing order.
pub(super) fn text_rects(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<egui::Rect> {
    fn find(shape: &egui::Shape, text: &str, rects: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => {
                rects.push(egui::Rect::from_min_size(shape.pos, shape.galley.size()));
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| find(shape, text, rects)),
            _ => {}
        }
    }
    let mut rects = Vec::new();
    for shape in shapes {
        find(&shape.shape, text, &mut rects);
    }
    rects
}

/// Where the first text shape reading exactly `text` is drawn.
pub(super) fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Rect> {
    text_rects(shapes, text).into_iter().next()
}

/// A sidebar column drawing `contents`, which returns what it changed. A
/// window wider than tall gets the desktop cards, with headers.
pub(super) struct SidebarHarness<T> {
    ctx: egui::Context,
    time: f64,
    screen: egui::Vec2,
    width: f32,
    contents: Box<dyn FnMut(&mut Ui) -> T>,
}

impl<T: Default> SidebarHarness<T> {
    /// A `width` wide column in a `screen` sized window.
    pub(super) fn new(
        screen: egui::Vec2,
        width: f32,
        contents: impl FnMut(&mut Ui) -> T + 'static,
    ) -> Self {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        Self {
            ctx,
            time: 0.0,
            screen,
            width,
            contents: Box::new(contents),
        }
    }

    pub(super) fn render(
        &mut self,
        events: Vec<egui::Event>,
    ) -> (Vec<egui::epaint::ClippedShape>, T) {
        self.time += 0.25;
        let mut result = T::default();
        let output = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(self.width);
                result = (self.contents)(ui);
            },
        );
        (output.shapes, result)
    }

    /// Clicks at `position` and returns what the release changed.
    pub(super) fn click(&mut self, position: egui::Pos2) -> T {
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

    /// Clicks the last Add effect button and returns where its picker shows
    /// `effect`'s name, with the button's label rect.
    pub(super) fn open_picker(&mut self, effect: MaskEffect) -> Option<(egui::Rect, egui::Rect)> {
        let (shapes, _) = self.render(Vec::new());
        let button = *text_rects(&shapes, &add_effect_label())
            .last()
            .expect("Add effect button");
        self.click(button.center());
        // A new popup lays itself out once before it is drawn.
        (0..3)
            .find_map(|_| text_rect(&self.render(Vec::new()).0, effect.label()))
            .map(|tile| (tile, button))
    }

    /// Adds `effect` with the last Add effect button and returns what the
    /// click changed.
    pub(super) fn pick(&mut self, effect: MaskEffect) -> T {
        let (tile, _) = self
            .open_picker(effect)
            .unwrap_or_else(|| panic!("the picker shows {}", effect.label()));
        self.click(tile.center())
    }
}
