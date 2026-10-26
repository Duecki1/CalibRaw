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
