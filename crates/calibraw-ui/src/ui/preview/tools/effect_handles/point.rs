//! Point handles: the Relight and Light Rays sources and the Radial Blur and
//! Vignette centers. Drag a handle to place it and double-click it to reset
//! its position; scroll over Relight's light to move it nearer or farther.
//! A focused handle moves one percent per arrow key, and Relight's light one
//! depth step per Page Up or Page Down; Shift makes the steps finer.

use super::*;
use crate::app::EffectHandleEdit;
use crate::pipeline::effect_params::{self, FloatParamSpec};

/// Depth ring radius for a light at the camera and at the farthest surface:
/// the nearer the light, the larger it looks.
const DEPTH_RING_RADIUS: [f32; 2] = [20.0, 9.0];
/// Ring radius of a light without depth and of a center.
const RING_RADIUS: f32 = 11.0;
const LIGHT_RADIUS: f32 = 6.0;
/// Light depth per point of vertical scrolling; Shift scrolls five times finer.
const DEPTH_PER_SCROLL_POINT: f32 = 0.1;

/// What a point handle edits and how it looks.
struct PointSpec {
    frame: Frame,
    position: [f32; 2],
    specs: [FloatParamSpec; 2],
    /// A light source shows its color; a center is white.
    light: Option<[f32; 3]>,
    /// Relight's light depth, edited by scrolling.
    depth: Option<f32>,
    name: &'static str,
}

impl PointSpec {
    fn of(handle: &EffectHandle) -> Option<Self> {
        use effect_params::{light_rays, radial_blur, relight, vignette};
        let settings = &handle.settings;
        Some(match handle.effect {
            MaskEffect::Relight => Self {
                frame: Frame::Source,
                position: settings.relight.source,
                specs: [relight::SOURCE_X, relight::SOURCE_Y],
                light: Some(settings.relight.color),
                depth: Some(settings.relight.depth),
                name: "Relight light",
            },
            MaskEffect::LightRays => Self {
                frame: Frame::Source,
                position: settings.light_rays.source,
                specs: [light_rays::SOURCE_X, light_rays::SOURCE_Y],
                light: Some(settings.light_rays.color),
                depth: None,
                name: "Light Rays source",
            },
            MaskEffect::RadialBlur => Self {
                frame: Frame::Source,
                position: settings.radial_blur.center,
                specs: [radial_blur::CENTER_X, radial_blur::CENTER_Y],
                light: None,
                depth: None,
                name: "Radial Blur center",
            },
            MaskEffect::Vignette => Self {
                frame: Frame::Output,
                position: settings.vignette.center,
                specs: [vignette::CENTER_X, vignette::CENTER_Y],
                light: None,
                depth: None,
                name: "Vignette center",
            },
            _ => return None,
        })
    }

    fn clamp(&self, position: [f32; 2]) -> [f32; 2] {
        [
            self.specs[0].clamp(position[0]),
            self.specs[1].clamp(position[1]),
        ]
    }
}

pub(super) fn show(
    ui: &Ui,
    painter: &egui::Painter,
    frames: Frames<'_>,
    handle: &EffectHandle,
) -> Vec<EffectHandleEdit> {
    let Some(spec) = PointSpec::of(handle) else {
        return Vec::new();
    };
    let mut edits = Vec::new();
    let placement = Placement::of(frames.screen_of(spec.frame, spec.position), frames.viewport);
    let id = handle_id(ui, handle.target, "point");
    let mut value = format!("{:.0}%, {:.0}%", spec.position[0], spec.position[1]);
    if let Some(depth) = spec.depth {
        value.push_str(&format!(", depth {}", format_signed(depth)));
    }
    let response = grab(ui, id, placement.drawn, spec.name, value);

    let mut position = spec.position;
    if response.double_clicked() {
        position = [spec.specs[0].default, spec.specs[1].default];
    } else if let Some(moved) = drag_to(ui, &response, id, placement) {
        position = spec.clamp(frames.position_at(spec.frame, moved));
    } else {
        let steps = arrow_steps(ui, &response);
        position = spec.clamp([position[0] + steps.x, position[1] + steps.y]);
    }
    if position != spec.position || response.double_clicked() {
        edits.push(EffectHandleEdit::Position(position));
    }

    let active = is_active(&response);
    let mut depth = spec.depth;
    if let Some(current) = spec.depth {
        let (scroll, fine) = if active {
            ui.input(|input| (input.smooth_scroll_delta.y, input.modifiers.shift))
        } else {
            (0.0, false)
        };
        // Scrolling up, like Page Up, pushes the light away from the camera.
        let rate = DEPTH_PER_SCROLL_POINT * if fine { FINE_STEP } else { 1.0 };
        let delta = scroll * rate + page_steps(ui, &response);
        if delta.abs() > 1e-4 {
            let moved = effect_params::relight::DEPTH.clamp(current + delta);
            if moved != current {
                edits.push(EffectHandleEdit::Depth(moved));
                depth = Some(moved);
            }
        }
    }
    set_grab_cursor(ui, &response);

    let placement = if position == spec.position {
        placement
    } else {
        Placement::of(frames.screen_of(spec.frame, position), frames.viewport)
    };
    paint(painter, placement, &spec, depth, active);
    response.on_hover_text(if spec.depth.is_some() {
        "Drag to move the light; arrow keys nudge it once selected. Scroll or press Page Up or Page Down to move it nearer or farther. Shift makes every step finer. Double-click to reset its position."
    } else {
        "Drag to move; arrow keys nudge it once selected, finely with Shift. Double-click to reset the position."
    });
    edits
}

fn paint(
    painter: &egui::Painter,
    placement: Placement,
    spec: &PointSpec,
    depth: Option<f32>,
    active: bool,
) {
    let center = placement.drawn;
    let ring = depth.map_or(RING_RADIUS, |depth| {
        let range = effect_params::relight::DEPTH;
        let nearness = ((range.max - depth) / (range.max - range.min)).clamp(0.0, 1.0);
        egui::lerp(DEPTH_RING_RADIUS[1]..=DEPTH_RING_RADIUS[0], nearness)
    });
    let ring_width = if active { 2.0 } else { 1.25 };
    handles::paint_ring(
        painter,
        center,
        ring,
        Stroke::new(ring_width, Color32::WHITE),
    );
    match spec.light {
        Some(color) => {
            let [r, g, b] = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            handles::paint_point(painter, center, LIGHT_RADIUS, Color32::from_rgb(r, g, b));
        }
        None => handles::paint_point(painter, center, handles::POINT_RADIUS, Color32::WHITE),
    }

    if !placement.in_view() {
        // Point toward a position beyond the view.
        let direction = (placement.anchor - center).normalized();
        let tip = center + direction * (ring + 9.0);
        let base = center + direction * (ring + 3.0);
        let side = direction.rot90() * 4.0;
        painter.add(Shape::convex_polygon(
            vec![tip, base + side, base - side],
            Color32::WHITE,
            Stroke::new(1.0, Color32::from_black_alpha(130)),
        ));
    }

    if active {
        let text = match depth {
            Some(depth) => format!("Depth {}", format_signed(depth)),
            None => spec.name.to_owned(),
        };
        handles::paint_value_label(painter, center, ring + 14.0, text);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    #[test]
    fn dragging_moves_a_point_by_the_pointer_without_a_jump() {
        // The light at 25%, 50% is drawn at (100, 150); grab it 5 points off
        // and move 40 points of a 400-point image: ten percent to the right.
        let input = input(MaskEffect::Relight, |settings| {
            settings.relight.source = [25.0, 50.0];
        });
        let edits =
            Canvas::new().drag(&input, Pos2::new(105.0, 150.0), Pos2::new(145.0, 150.0), 10);
        assert_position(&edits, [35.0, 50.0]);
    }

    #[test]
    fn scrolling_over_the_relight_light_sets_its_depth_instead_of_zooming() {
        let input = input(MaskEffect::Relight, |settings| {
            settings.relight.source = [25.0, 50.0];
            settings.relight.depth = -30.0;
        });
        let mut canvas = Canvas::new();
        let over = Pos2::new(100.0, 150.0);
        canvas.frame(&input, pointer(over, None));
        let mut depth = None;
        let mut canvas_saw_pointer = false;
        for _ in 0..30 {
            let (edits, canvas_hovered) = canvas.frame(&input, scroll(over, 50.0));
            canvas_saw_pointer |= canvas_hovered;
            for edit in edits {
                if let EffectHandleEdit::Depth(value) = edit {
                    depth = Some(value);
                }
            }
        }
        let depth = depth.expect("scroll sets depth");
        assert!(depth > -30.0, "scrolling up moves the light away: {depth}");
        assert!(
            !canvas_saw_pointer,
            "the canvas would zoom on the same scroll"
        );

        // Away from the light the scroll belongs to the canvas.
        let away = Pos2::new(300.0, 60.0);
        let mut edits = 0;
        let mut canvas_hovered = false;
        for _ in 0..5 {
            let (frame_edits, hovered) = canvas.frame(&input, scroll(away, 50.0));
            edits += frame_edits.len();
            canvas_hovered |= hovered;
        }
        assert_eq!(edits, 0);
        assert!(canvas_hovered);
    }

    #[test]
    fn centers_without_depth_ignore_scrolling() {
        let input = input(MaskEffect::RadialBlur, |settings| {
            settings.radial_blur.center = [50.0, 50.0];
        });
        let mut canvas = Canvas::new();
        let over = Pos2::new(200.0, 150.0);
        canvas.frame(&input, pointer(over, None));
        for _ in 0..5 {
            assert!(canvas.frame(&input, scroll(over, 50.0)).0.is_empty());
        }
    }

    #[test]
    fn a_point_beyond_the_view_is_pinned_to_its_edge_and_comes_to_the_pointer() {
        let placement = Placement::of(Pos2::new(-200.0, 150.0), VIEWPORT);
        assert!(!placement.in_view());
        assert_eq!(placement.drawn, Pos2::new(handles::POINT_REACH, 150.0));
        assert!(Placement::of(Pos2::new(100.0, 150.0), VIEWPORT).in_view());

        // Source -50% sits 200 points left of the 400-point image.
        let input = input(MaskEffect::LightRays, |settings| {
            settings.light_rays.source = [-50.0, 50.0];
        });
        let edits = Canvas::new().drag(&input, placement.drawn, Pos2::new(60.0, 120.0), 6);
        assert_position(&edits, [15.0, 40.0]);
    }

    #[test]
    fn double_clicking_resets_the_position() {
        let input = input(MaskEffect::Vignette, |settings| {
            settings.vignette.center = [25.0, 25.0];
        });
        let mut canvas = Canvas::new();
        let at = Pos2::new(100.0, 75.0);
        let mut reset = None;
        for _ in 0..2 {
            canvas.frame(&input, pointer(at, Some(true)));
            let (edits, _) = canvas.frame(&input, pointer(at, Some(false)));
            reset = reset.or(edits.into_iter().find_map(|edit| match edit {
                EffectHandleEdit::Position(position) => Some(position),
                _ => None,
            }));
        }
        let defaults = [
            effect_params::vignette::CENTER_X.default,
            effect_params::vignette::CENTER_Y.default,
        ];
        assert_eq!(reset, Some(defaults));
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn a_pressed_handle_takes_focus_and_steps_with_the_keyboard() {
        let input = input(MaskEffect::Relight, |settings| {
            settings.relight.source = [25.0, 50.0];
            settings.relight.depth = 10.0;
        });
        let mut canvas = Canvas::new();
        let at = Pos2::new(100.0, 150.0);
        canvas.frame(&input, pointer(at, None));
        canvas.frame(&input, pointer(at, Some(true)));
        canvas.frame(&input, pointer(at, Some(false)));

        let (edits, _) = canvas.frame(
            &input,
            vec![
                key(egui::Key::ArrowRight, egui::Modifiers::NONE),
                key(egui::Key::ArrowUp, egui::Modifiers::SHIFT),
                key(egui::Key::PageUp, egui::Modifiers::NONE),
            ],
        );
        assert_position(&edits, [26.0, 50.0 - FINE_STEP]);
        assert!(edits.contains(&EffectHandleEdit::Depth(11.0)), "{edits:?}");
    }

    #[test]
    fn arrow_keys_leave_an_unfocused_handle_alone() {
        let input = input(MaskEffect::Vignette, |settings| {
            settings.vignette.center = [25.0, 25.0];
        });
        let mut canvas = Canvas::new();
        let (edits, _) = canvas.frame(
            &input,
            vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
        );
        assert!(edits.is_empty(), "{edits:?}");
    }
}
