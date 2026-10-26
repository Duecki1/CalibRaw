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
    if moduwu_design::double_tapped(&response) {
        position = [spec.specs[0].default, spec.specs[1].default];
    } else if let Some(moved) = drag_to(ui, &response, id, placement) {
        position = spec.clamp(frames.position_at(spec.frame, moved));
    } else {
        let steps = arrow_steps(ui, &response);
        position = spec.clamp([position[0] + steps.x, position[1] + steps.y]);
    }
    if position != spec.position || moduwu_design::double_tapped(&response) {
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
