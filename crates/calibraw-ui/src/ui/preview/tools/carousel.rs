//! The carousel stencil over the preview, shown only while the Export panel is open. Its slide
//! count, shape and size come from the panel's card; here it is dragged into place and drawn
//! where the export cuts the output.

use super::super::*;
use crate::app::CarouselAction;
use crate::i18n::tr;
use crate::pipeline::CarouselLayout;

const OUTSIDE_SHADE: Color32 = Color32::from_black_alpha(150);
/// Screen distance of a slide's number from its corner.
const NUMBER_INSET: f32 = 16.0;

/// The stencil on screen.
fn carousel_screen_rect(stencil: &CarouselLayout, layout: PreviewLayout) -> Rect {
    let output = layout.image_rect;
    let [left, top, right, bottom] = stencil.frame_uv();
    Rect::from_min_max(
        output.lerp_inside(egui::vec2(left, top)),
        output.lerp_inside(egui::vec2(right, bottom)),
    )
}

/// Each slide on screen, left to right.
pub(in crate::ui::preview) fn carousel_slide_rects(
    stencil: &CarouselLayout,
    layout: PreviewLayout,
) -> impl Iterator<Item = Rect> {
    let frame = carousel_screen_rect(stencil, layout);
    let slides = stencil.slides();
    let width = frame.width() / f32::from(slides);
    (0..slides).map(move |slide| {
        let left = frame.left() + width * f32::from(slide);
        Rect::from_min_max(
            Pos2::new(left, frame.top()),
            Pos2::new(left + width, frame.bottom()),
        )
    })
}

impl Preview {
    /// Dragging the stencil. `grab` is the drag in progress.
    pub(in crate::ui::preview) fn carousel_tool_action(
        ui: &Ui,
        stencil: &CarouselLayout,
        grab: Option<[f32; 2]>,
        layout: PreviewLayout,
        gesture_free: bool,
    ) -> Option<CarouselAction> {
        let output = layout.image_rect;
        let frame = carousel_screen_rect(stencil, layout);
        let (pointer, pressed, down) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        let output_uv = |point: Pos2| {
            let offset = point - output.min;
            [
                offset.x / output.width().max(1.0),
                offset.y / output.height().max(1.0),
            ]
        };

        if let Some(grab) = grab {
            if !gesture_free || !down {
                return Some(CarouselAction::Release);
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let [u, v] = output_uv(pointer?);
            return Some(CarouselAction::MoveTo([u - grab[0], v - grab[1]]));
        }

        let pointer = pointer.filter(|point| {
            gesture_free
                && frame.contains(*point)
                && layout.viewport_rect.contains(*point)
                && ui.ctx().layer_id_at(*point) == Some(ui.layer_id())
        })?;
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        pressed.then(|| {
            let [u, v] = output_uv(pointer);
            let [left, top, right, bottom] = stencil.frame_uv();
            CarouselAction::Grab([u - (left + right) * 0.5, v - (top + bottom) * 0.5])
        })
    }

    pub(in crate::ui::preview) fn paint_carousel(
        ui: &Ui,
        stencil: &CarouselLayout,
        layout: PreviewLayout,
    ) {
        let frame = carousel_screen_rect(stencil, layout);
        // Custom-painted, so assistive technology is told what it is.
        ui.interact(
            frame.intersect(layout.viewport_rect),
            ui.id().with("carousel-stencil"),
            egui::Sense::hover(),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, tr("Carousel frame"))
        });

        let painter = ui.painter_at(layout.viewport_rect);
        let visible = layout.visible_rect;
        for outside in [
            Rect::from_min_max(visible.min, Pos2::new(visible.right(), frame.top())),
            Rect::from_min_max(Pos2::new(visible.left(), frame.bottom()), visible.max),
            Rect::from_min_max(
                Pos2::new(visible.left(), frame.top()),
                Pos2::new(frame.left(), frame.bottom()),
            ),
            Rect::from_min_max(
                Pos2::new(frame.right(), frame.top()),
                Pos2::new(visible.right(), frame.bottom()),
            ),
        ] {
            let outside = outside.intersect(visible);
            if outside.is_positive() {
                painter.rect_filled(outside, 0.0, OUTSIDE_SHADE);
            }
        }

        handles::paint_guide(
            &painter,
            vec![
                frame.left_top(),
                frame.right_top(),
                frame.right_bottom(),
                frame.left_bottom(),
                frame.left_top(),
            ],
            Stroke::new(2.0, Color32::WHITE),
        );
        for (index, slide) in carousel_slide_rects(stencil, layout).enumerate() {
            if index > 0 {
                handles::paint_guide(
                    &painter,
                    vec![slide.left_top(), slide.left_bottom()],
                    Stroke::new(1.5, Color32::from_white_alpha(220)),
                );
            }
            handles::paint_value_label(
                &painter,
                slide.left_top() + egui::Vec2::splat(NUMBER_INSET),
                0.0,
                (index + 1).to_string(),
            );
        }
    }
}
