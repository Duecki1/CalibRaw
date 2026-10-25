//! Grows one element of the portrait Develop panel, such as the LUT list, so
//! the panel's content ends at the bottom of its visible area: the card shows
//! whole and the element scrolls inside it instead of the panel.
//!
//! An element's height comes from the frame before: the panel reports how far
//! its content overflowed ([`Sidebar::settle_portrait_fill`]) and the element
//! changes by that much. The overflow does not depend on the panel's scroll
//! offset, so the height settles one frame after the content changes.

use super::*;

/// The element [`Sidebar::portrait_fill`] drew this frame, for the panel to
/// settle once its content is measured.
#[derive(Clone, Copy)]
struct ShownFill {
    id: egui::Id,
    /// The height the element was given.
    target: f32,
    /// The height it took; less than `target` when its content is shorter.
    rendered: f32,
    min_height: f32,
}

fn shown_fill_id() -> egui::Id {
    egui::Id::new("develop-portrait-fill")
}

impl Sidebar {
    /// Draws `add` with the height, at least `min_height`, that makes the
    /// portrait panel's content end at the bottom of its visible area.
    pub(super) fn portrait_fill<R>(
        ui: &mut Ui,
        id_salt: impl egui::AsIdSalt,
        min_height: f32,
        add: impl FnOnce(&mut Ui, f32) -> R,
    ) -> R {
        let id = ui.make_persistent_id(id_salt);
        let target = ui
            .data(|data| data.get_temp::<f32>(id))
            .unwrap_or(min_height)
            .max(min_height);
        let inner = ui.scope(|ui| add(ui, target));
        let shown = ShownFill {
            id,
            target,
            rendered: inner.response.rect.height(),
            min_height,
        };
        ui.data_mut(|data| data.insert_temp(shown_fill_id(), shown));
        inner.inner
    }

    /// Sizes the element [`Self::portrait_fill`] drew this frame for the next
    /// one, now that the panel's content overflowed its visible area by
    /// `overflow` points; negative when it ended above the bottom.
    pub(super) fn settle_portrait_fill(ctx: &egui::Context, overflow: f32) {
        let Some(shown) = ctx.data_mut(|data| {
            let shown = data.get_temp::<ShownFill>(shown_fill_id());
            data.remove::<ShownFill>(shown_fill_id());
            shown
        }) else {
            return;
        };
        let next = (shown.rendered - overflow).max(shown.min_height);
        if (next - shown.target).abs() > 0.5 {
            ctx.data_mut(|data| data.insert_temp(shown.id, next));
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::preview_list::{preview_list, ListHeight, PreviewListItem};

    const MIN_HEIGHT: f32 = 60.0;

    /// Draws one frame of a panel `viewport_height` tall that scrolls fixed
    /// content around a filling list of `rows` rows, scrolled by `offset`.
    /// Returns the content's overflow and the list's height.
    fn frame(ctx: &egui::Context, viewport_height: f32, rows: usize, offset: f32) -> (f32, f32) {
        let mut measured = (0.0, 0.0);
        let viewport = egui::vec2(320.0, viewport_height);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewport)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let output = egui::ScrollArea::vertical()
                .vertical_scroll_offset(offset)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(120.0);
                    let top = ui.cursor().top();
                    Sidebar::portrait_fill(ui, "fill", MIN_HEIGHT, |ui, height| {
                        let items = (0..rows)
                            .map(|row| PreviewListItem::choice(row, format!("Row {row}"), false));
                        preview_list(ui, "list", ListHeight::Points(height), items);
                    });
                    let list = ui.cursor().top() - top;
                    ui.add_space(90.0);
                    list
                });
            let overflow = output.content_size.y - output.inner_rect.height();
            Sidebar::settle_portrait_fill(ui.ctx(), overflow);
            measured = (overflow, output.inner);
        });
        measured
    }

    #[test]
    fn a_long_list_grows_until_the_content_ends_at_the_bottom() {
        let ctx = egui::Context::default();
        frame(&ctx, 400.0, 40, 0.0);
        let (overflow, list) = frame(&ctx, 400.0, 40, 0.0);
        assert!(overflow.abs() < 1.0, "{overflow}");
        assert!(list > MIN_HEIGHT);

        // Scrolling the panel leaves the list as it is.
        let (scrolled_overflow, scrolled_list) = frame(&ctx, 400.0, 40, 50.0);
        assert!(scrolled_overflow.abs() < 1.0, "{scrolled_overflow}");
        assert!((scrolled_list - list).abs() < 1.0);
    }

    #[test]
    fn a_short_list_keeps_its_rows_and_settles() {
        let ctx = egui::Context::default();
        frame(&ctx, 400.0, 2, 0.0);
        let (overflow, list) = frame(&ctx, 400.0, 2, 0.0);
        assert!(overflow < 0.0);
        assert!(list < MIN_HEIGHT * 2.0);
        assert_eq!(frame(&ctx, 400.0, 2, 0.0), (overflow, list));
    }

    #[test]
    fn a_panel_too_short_for_the_card_keeps_the_minimum() {
        let ctx = egui::Context::default();
        frame(&ctx, 200.0, 40, 0.0);
        let (overflow, list) = frame(&ctx, 200.0, 40, 0.0);
        assert!(overflow > 0.0);
        assert!((list - MIN_HEIGHT).abs() < 8.0, "{list}");
    }

    #[test]
    fn the_list_follows_a_resized_panel() {
        let ctx = egui::Context::default();
        frame(&ctx, 400.0, 40, 0.0);
        let (_, tall) = frame(&ctx, 400.0, 40, 0.0);
        frame(&ctx, 320.0, 40, 0.0);
        let (overflow, short) = frame(&ctx, 320.0, 40, 0.0);
        assert!(overflow.abs() < 1.0, "{overflow}");
        assert!((tall - short - 80.0).abs() < 1.0, "{tall} then {short}");
    }
}
