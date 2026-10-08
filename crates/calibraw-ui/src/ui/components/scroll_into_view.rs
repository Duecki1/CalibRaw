//! Scrolling a widget into view as soon as it is drawn.
//!
//! The action that creates or reveals a widget usually runs before the widget
//! is laid out, and the widget may first appear a frame later. The action
//! calls [`request`] with a target id for the widget; the widget calls
//! [`on_draw`] with the same id and its rect, and the enclosing scroll areas
//! bring it into view. One request is pending at a time; a newer one replaces it.

use eframe::egui::{Align, Context, Id, Rect, Ui};

/// Passes a request waits for its widget. A widget that is not drawn by then
/// (another tab, a folded card) never scrolls into view out of nowhere later.
const MAX_WAIT_PASSES: u64 = 4;

#[derive(Clone, Copy, Debug)]
struct Request {
    target: Id,
    align: Option<Align>,
    pass: u64,
}

fn request_id() -> Id {
    Id::new("calibraw-scroll-into-view")
}

/// Scrolls the widget drawn with `target` into view. `align` places it in the
/// viewport; `None` scrolls the least distance that shows it, its top first
/// when it is taller than the viewport.
pub(crate) fn request(ctx: &Context, target: Id, align: Option<Align>) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        data.insert_temp(
            request_id(),
            Request {
                target,
                align,
                pass,
            },
        );
    });
}

/// Scrolls `rect` into view if it was requested for `target`. Call it after
/// drawing the widget, inside the scroll areas that hold it.
pub(crate) fn on_draw(ui: &Ui, target: Id, rect: Rect) {
    let Some(pending) = ui.data(|data| data.get_temp::<Request>(request_id())) else {
        return;
    };
    let lapsed = ui.ctx().cumulative_pass_nr() > pending.pass + MAX_WAIT_PASSES;
    if pending.target == target && !lapsed {
        ui.scroll_to_rect(rect, pending.align);
    }
    if pending.target == target || lapsed {
        ui.data_mut(|data| data.remove::<Request>(request_id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn run(ctx: &Context, draw_target: bool) -> f32 {
        let mut offset = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(200.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                offset = egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        ui.allocate_space(egui::vec2(100.0, 1_000.0));
                        if draw_target {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(100.0, 40.0), egui::Sense::hover());
                            on_draw(ui, Id::new("target"), rect);
                        }
                        ui.allocate_space(egui::vec2(100.0, 1_000.0));
                    })
                    .state
                    .offset
                    .y;
            },
        );
        offset
    }

    #[test]
    fn a_requested_widget_scrolls_into_view_once_drawn() {
        let ctx = Context::default();
        // Land in one pass instead of animating over several frames.
        ctx.all_styles_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
        run(&ctx, true);
        request(&ctx, Id::new("target"), None);
        // Not drawn yet: the request waits.
        assert_eq!(run(&ctx, false), 0.0);
        run(&ctx, true);
        // The scroll area applies the scroll as its pass ends.
        let offset = run(&ctx, true);
        // The widget spans 1000..1040 in a 200-point viewport.
        assert!((840.0..=1_000.0).contains(&offset), "offset {offset}");
        // The request is used up.
        assert!(ctx
            .data(|data| data.get_temp::<Request>(request_id()))
            .is_none());
    }

    #[test]
    fn a_request_lapses_when_its_widget_is_not_drawn_soon() {
        let ctx = Context::default();
        request(&ctx, Id::new("target"), None);
        for _ in 0..=MAX_WAIT_PASSES + 1 {
            run(&ctx, false);
        }
        assert_eq!(run(&ctx, true), 0.0);
        assert_eq!(run(&ctx, true), 0.0);
    }
}
