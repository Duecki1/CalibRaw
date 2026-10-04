//! Exercise the preview entry points with pointer frames and inspect their output.
#![cfg(not(target_os = "android"))]

use super::*;
use crate::pipeline::LoadedRaw;
use std::time::{Duration, Instant};

const WIDTH: u32 = 960;
const HEIGHT: u32 = 720;
const CENTER: [f32; 2] = [0.38, 0.43];
const RADIUS: [f32; 2] = [0.22, 0.18];
const ROTATION: f32 = 0.31;
const START: [f32; 2] = [0.24, 0.29];
const END: [f32; 2] = [0.72, 0.63];

fn image_rect() -> Rect {
    Rect::from_min_size(
        egui::pos2(100.0, 100.0),
        egui::vec2(WIDTH as f32, HEIGHT as f32),
    )
}

fn screen_rect() -> Rect {
    Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 1000.0))
}

fn display_geometry() -> GeometryTransform {
    GeometryTransform {
        crop: [0.08, 0.12, 0.92, 0.90],
        quarter_turns: 1,
        rotation_degrees: 17.0,
        ..Default::default()
    }
}

fn nonlinear_lens() -> LensGeometryMap {
    let grid = 33;
    let mut coordinates = Vec::new();
    for y in 0..grid {
        for x in 0..grid {
            let dx = x as f32 / (grid - 1) as f32 - 0.5;
            let dy = y as f32 / (grid - 1) as f32 - 0.5;
            let scale = 0.72 + 0.56 * (dx * dx + dy * dy);
            coordinates.push([
                (0.5 + dx * scale) * (WIDTH - 1) as f32,
                (0.5 + dy * scale) * (HEIGHT - 1) as f32,
            ]);
        }
    }
    LensGeometryMap::new(WIDTH, HEIGHT, grid, grid, coordinates).unwrap()
}

#[derive(Clone, Copy)]
enum Paint {
    None,
    Guides,
    Coverage,
}

struct Harness {
    context: egui::Context,
    app: CalibRawApp,
}

impl Harness {
    fn new(kind: MaskKind) -> Self {
        let context = egui::Context::default();
        let mut app = CalibRawApp::empty(&context);
        let mut raw = LoadedRaw::from_scene_linear_rec2020(
            WIDTH,
            HEIGHT,
            vec![0.2; (WIDTH * HEIGHT * 3) as usize],
        )
        .unwrap();
        raw.lens_geometry = Some(Arc::new(nonlinear_lens()));
        app.develop.loaded_raw = Some(Arc::new(raw));
        app.develop.geometry = display_geometry();
        app.preview.visible_uv = crate::app::PreviewUvRect {
            min: [0.0, 0.0],
            max: [1.0, 1.0],
        };
        app.masks.stack.add_mask(kind).unwrap();
        app.masks.stack.masks[0].components[0].geometry = match kind {
            MaskKind::Radial => MaskGeometry::Radial {
                center: CENTER,
                radius: RADIUS,
                rotation: ROTATION,
                feather: 0.55,
                initialized: true,
            },
            MaskKind::Linear => MaskGeometry::Linear {
                start: START,
                end: END,
                feather: 0.65,
                initialized: true,
            },
            _ => unreachable!(),
        };
        let mut harness = Self { context, app };
        // Register the interaction rectangle before sending its first press.
        harness.frame(vec![], Paint::None);
        harness.frame(vec![], Paint::None);
        harness
    }

    fn frame(&mut self, events: Vec<egui::Event>, paint: Paint) -> egui::FullOutput {
        let app = &mut self.app;
        self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(screen_rect()),
                events,
                ..Default::default()
            },
            |ui| {
                let response = ui.allocate_rect(screen_rect(), Sense::drag());
                let layout = PreviewLayout {
                    image_rect: image_rect(),
                    visible_rect: image_rect(),
                    viewport_rect: screen_rect(),
                    source_width: WIDTH,
                    source_height: HEIGHT,
                };
                let actions = Preview::mask_tool_actions(
                    ui,
                    &tools::MaskToolInput::of(app),
                    layout,
                    &response,
                );
                app.apply_mask_tool_actions(ui.ctx(), actions);
                match paint {
                    Paint::None => {}
                    Paint::Guides => Preview::paint_mask_overlay(ui, app, layout),
                    Paint::Coverage => Preview::paint_coverage_texture(ui, app, layout, 0, Some(0)),
                }
            },
        )
    }

    fn press(&mut self, position: Pos2, paint: Paint) -> egui::FullOutput {
        self.frame(
            vec![egui::Event::PointerMoved(position), button(position, true)],
            paint,
        )
    }

    fn move_to(&mut self, position: Pos2, paint: Paint) -> egui::FullOutput {
        self.frame(vec![egui::Event::PointerMoved(position)], paint)
    }

    fn release(&mut self, position: Pos2, paint: Paint) -> egui::FullOutput {
        self.frame(vec![button(position, false)], paint)
    }

    fn geometry(&self) -> &MaskGeometry {
        &self.app.masks.stack.masks[0].components[0].geometry
    }
}

fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn screen(uv: [f32; 2]) -> Pos2 {
    final_geometry_source_to_screen(image_rect(), display_geometry(), WIDTH, HEIGHT, uv)
}

fn corrected_uv(position: Pos2) -> [f32; 2] {
    final_geometry_screen_to_source(image_rect(), display_geometry(), WIDTH, HEIGHT, position)
}

fn rotate_uv(point: [f32; 2], center: [f32; 2], angle: f32) -> [f32; 2] {
    let x = (point[0] - center[0]) * WIDTH as f32;
    let y = (point[1] - center[1]) * HEIGHT as f32;
    [
        center[0] + (x * angle.cos() - y * angle.sin()) / WIDTH as f32,
        center[1] + (x * angle.sin() + y * angle.cos()) / HEIGHT as f32,
    ]
}

fn assert_uv(actual: [f32; 2], expected: [f32; 2]) {
    for axis in 0..2 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 2e-5,
            "corrected coordinates: actual={actual:?}, expected={expected:?}"
        );
    }
}

fn rotation_handle(kind: MaskKind) -> Pos2 {
    match kind {
        MaskKind::Radial => {
            SourceProjection::new(image_rect(), display_geometry(), None, WIDTH, HEIGHT)
                .radial_rotation_handle(CENTER, RADIUS, ROTATION)
        }
        MaskKind::Linear => {
            SourceProjection::new(image_rect(), display_geometry(), None, WIDTH, HEIGHT)
                .linear_rotation_handle(START, END)
                .1
        }
        _ => unreachable!(),
    }
}

#[test]
fn radial_handle_drags_use_corrected_coordinates_with_lens_rotation_and_crop() {
    let handles = SourceProjection::new(image_rect(), display_geometry(), None, WIDTH, HEIGHT)
        .radial_handles(CENTER, RADIUS, ROTATION);
    let rotation = rotation_handle(MaskKind::Radial);
    let resized_radius = [RADIUS[0], 0.25];
    let resized_uv = radial_source_uv_at(
        CENTER,
        resized_radius,
        ROTATION,
        std::f32::consts::FRAC_PI_2,
        WIDTH,
        HEIGHT,
    );
    for (case, from, to) in [
        ("move", screen(CENTER), screen([0.47, 0.51])),
        ("resize", handles[2], screen(resized_uv)),
        (
            "rotate",
            rotation,
            screen(rotate_uv(corrected_uv(rotation), CENTER, 0.4)),
        ),
    ] {
        let mut h = Harness::new(MaskKind::Radial);
        h.press(from, Paint::None);
        assert!(
            matches!(
                (case, h.app.masks.drag),
                ("move", Some(MaskDragState::MoveRadial { .. }))
                    | ("resize", Some(MaskDragState::ResizeRadial { axis: 1 }))
                    | ("rotate", Some(MaskDragState::RotateRadial { .. }))
            ),
            "wrong radial handle selected: {case}"
        );
        h.move_to(to, Paint::None);
        let MaskGeometry::Radial {
            center,
            radius,
            rotation,
            ..
        } = h.geometry()
        else {
            panic!("expected radial geometry");
        };
        assert_uv(*center, if case == "move" { [0.47, 0.51] } else { CENTER });
        assert_uv(
            *radius,
            if case == "resize" {
                resized_radius
            } else {
                RADIUS
            },
        );
        let expected_rotation = ROTATION + if case == "rotate" { 0.4 } else { 0.0 };
        assert!(
            (rotation - expected_rotation).abs() < 2e-5,
            "{case}: {rotation}"
        );
        let edited = h.geometry().clone();
        h.release(to, Paint::None);
        assert_eq!(h.geometry(), &edited);
        assert!(h.app.masks.drag.is_none());
    }
}

#[test]
fn linear_handle_drags_use_corrected_coordinates_with_lens_rotation_and_crop() {
    let midpoint = [(START[0] + END[0]) * 0.5, (START[1] + END[1]) * 0.5];
    let rotation = rotation_handle(MaskKind::Linear);
    for (case, from, to, expected_start, expected_end) in [
        (
            "start",
            screen(START),
            screen([0.30, 0.35]),
            [0.30, 0.35],
            END,
        ),
        (
            "end",
            screen(END),
            screen([0.66, 0.59]),
            START,
            [0.66, 0.59],
        ),
        (
            "move",
            screen(midpoint),
            screen([midpoint[0] + 0.06, midpoint[1] + 0.04]),
            [START[0] + 0.06, START[1] + 0.04],
            [END[0] + 0.06, END[1] + 0.04],
        ),
        (
            "rotate",
            rotation,
            screen(rotate_uv(corrected_uv(rotation), midpoint, 0.4)),
            rotate_uv(START, midpoint, 0.4),
            rotate_uv(END, midpoint, 0.4),
        ),
    ] {
        let mut h = Harness::new(MaskKind::Linear);
        h.press(from, Paint::None);
        assert!(
            matches!(
                (case, h.app.masks.drag),
                ("start", Some(MaskDragState::LinearStart))
                    | ("end", Some(MaskDragState::LinearEnd))
                    | ("move", Some(MaskDragState::MoveLinear { .. }))
                    | ("rotate", Some(MaskDragState::RotateLinear { .. }))
            ),
            "wrong linear handle selected: {case}"
        );
        h.move_to(to, Paint::None);
        let MaskGeometry::Linear { start, end, .. } = h.geometry() else {
            panic!("expected linear geometry");
        };
        assert_uv(*start, expected_start);
        assert_uv(*end, expected_end);
        let edited = h.geometry().clone();
        h.release(to, Paint::None);
        assert_eq!(h.geometry(), &edited);
        assert!(h.app.masks.drag.is_none());
    }
}

fn paths(output: &egui::FullOutput) -> Vec<&egui::epaint::PathShape> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Path(path) if path.points.len() > 2 => Some(path),
            _ => None,
        })
        .collect()
}

fn max_line_deviation(points: &[Pos2]) -> f32 {
    let origin = points[0];
    let direction = *points.last().unwrap() - origin;
    assert!(
        direction.length() > 10.0,
        "guide must have a measurable length"
    );
    points
        .iter()
        .map(|point| {
            let offset = *point - origin;
            (offset.x * direction.y - offset.y * direction.x).abs() / direction.length()
        })
        .fold(0.0, f32::max)
}

#[test]
fn painted_linear_axis_and_feather_guides_stay_straight_under_lens_rotation_and_crop() {
    let mut h = Harness::new(MaskKind::Linear);
    // Ensure this synthetic lens would actually bend the old guide path.
    let distorted = SourceProjection::new(
        image_rect(),
        display_geometry(),
        loaded_lens_geometry(&h.app).map(Arc::as_ref),
        WIDTH,
        HEIGHT,
    )
    .linear_axis(START, END, 48);
    assert!(max_line_deviation(&distorted) > 1.0);
    let output = h.frame(vec![], Paint::Guides);
    let guides = paths(&output);
    assert_eq!(guides.len(), 4, "axis, center, and two feather boundaries");
    for guide in &guides {
        assert!(
            max_line_deviation(&guide.points) < 0.002,
            "painted guide was bent"
        );
    }
    let axis = guides
        .iter()
        .find(|guide| guide.points.len() == 49)
        .unwrap();
    assert!(axis.points[0].distance(screen(START)) < 0.002);
    assert!(axis.points.last().unwrap().distance(screen(END)) < 0.002);
}

#[test]
fn painted_radial_outlines_follow_corrected_ellipses_under_lens_rotation_and_crop() {
    let mut h = Harness::new(MaskKind::Radial);
    let output = h.frame(vec![], Paint::Guides);
    let outlines = paths(&output);
    assert_eq!(outlines.len(), 2, "outer and feather ellipses");
    for (outline, scale) in outlines.iter().zip([1.0, 1.0 - 0.55 * 0.98]) {
        assert_eq!(outline.points.len(), 73);
        for point in &outline.points {
            let uv = corrected_uv(*point);
            let dx = (uv[0] - CENTER[0]) * WIDTH as f32;
            let dy = (uv[1] - CENTER[1]) * HEIGHT as f32;
            let x =
                (ROTATION.cos() * dx + ROTATION.sin() * dy) / (RADIUS[0] * scale * WIDTH as f32);
            let y =
                (-ROTATION.sin() * dx + ROTATION.cos() * dy) / (RADIUS[1] * scale * HEIGHT as f32);
            assert!(
                (x * x + y * y - 1.0).abs() < 2e-4,
                "painted ellipse was lens warped"
            );
        }
    }
}

fn texture_image(output: &egui::FullOutput, id: egui::TextureId) -> &egui::ColorImage {
    let delta = &output
        .textures_delta
        .set
        .iter()
        .find(|(texture_id, _)| *texture_id == id)
        .expect("coverage texture must be uploaded in this frame")
        .1;
    assert!(
        delta.pos.is_none(),
        "coverage upload should replace the full texture"
    );
    let egui::ImageData::Color(image) = &delta.image;
    image
}

fn assert_pending_coverage_updates_and_release_refines(kind: MaskKind, rotate: bool) {
    let mut h = Harness::new(kind);
    assert!(h.app.preview.gpu_pipeline.is_none());
    let midpoint = if kind == MaskKind::Radial {
        CENTER
    } else {
        [(START[0] + END[0]) * 0.5, (START[1] + END[1]) * 0.5]
    };
    let from = if rotate {
        rotation_handle(kind)
    } else {
        screen(midpoint)
    };
    let initial = h.frame(vec![], Paint::Coverage);
    let id = h.app.masks.overlay_texture.as_ref().unwrap().id();
    let full_size = texture_image(&initial, id).size;
    assert!(full_size[0].max(full_size[1]) > 512);
    let pressed = h.press(from, Paint::Coverage);
    assert!(matches!(
        (kind, rotate, h.app.masks.drag),
        (
            MaskKind::Radial,
            false,
            Some(MaskDragState::MoveRadial { .. })
        ) | (
            MaskKind::Radial,
            true,
            Some(MaskDragState::RotateRadial { .. })
        ) | (
            MaskKind::Linear,
            false,
            Some(MaskDragState::MoveLinear { .. })
        ) | (
            MaskKind::Linear,
            true,
            Some(MaskDragState::RotateLinear { .. })
        )
    ));
    let mut previous = texture_image(&pressed, id).clone();
    assert!(previous.size[0].max(previous.size[1]) <= 512);
    let mut last_pointer = from;
    for amount in [0.18, 0.36] {
        // A future upload timestamp forces the pending branch without sleeping
        // or depending on how long rasterization takes on the test machine.
        let blocked_until = Instant::now() + Duration::from_secs(3600);
        h.app.masks.interaction_last_upload = Some(blocked_until);
        h.app.masks.dirty_layers.fill(false);
        h.app.masks.detail_dirty_layers.fill(false);
        h.app.masks.navigation_dirty_layers.fill(false);
        let revision = h.app.masks.overlay_revision;
        last_pointer = if rotate {
            screen(rotate_uv(corrected_uv(from), midpoint, amount))
        } else {
            screen([midpoint[0] + amount * 0.3, midpoint[1] + amount * 0.2])
        };
        let moved = h.move_to(last_pointer, Paint::Coverage);
        assert_eq!(h.app.masks.overlay_revision, revision.wrapping_add(1));
        assert_eq!(h.app.masks.interaction_last_upload, Some(blocked_until));
        assert!(h.app.masks.interaction_has_uncommitted_change);
        assert!(h.app.masks.dirty_layers.iter().all(|dirty| !dirty));
        assert!(h.app.masks.detail_dirty_layers.iter().all(|dirty| !dirty));
        assert!(h
            .app
            .masks
            .navigation_dirty_layers
            .iter()
            .all(|dirty| !dirty));
        let updated = texture_image(&moved, id);
        assert_eq!(updated.size, previous.size);
        assert!(
            updated.pixels != previous.pixels,
            "pending edit must change coverage before release"
        );
        previous = updated.clone();
    }
    let edited = h.geometry().clone();
    let drag_region = h.app.masks.overlay_texture_key.unwrap().3;
    let released = h.release(last_pointer, Paint::Coverage);
    assert_eq!(
        h.geometry(),
        &edited,
        "release preserves the last pointer edit"
    );
    assert!(h.app.masks.drag.is_none());
    assert!(h.app.masks.interaction_dirty_layer.is_none());
    assert!(h.app.masks.interaction_last_upload.is_none());
    assert!(!h.app.masks.interaction_has_uncommitted_change);
    assert!(h.app.masks.dirty_layers[0]);
    let refined = texture_image(&released, id);
    assert_eq!(refined.size, full_size);
    assert!(refined.size[0].max(refined.size[1]) > previous.size[0].max(previous.size[1]));
    let region = h.app.masks.overlay_texture_key.unwrap().3;
    assert_eq!(
        [
            region.source_x,
            region.source_y,
            region.source_width,
            region.source_height
        ],
        [
            drag_region.source_x,
            drag_region.source_y,
            drag_region.source_width,
            drag_region.source_height
        ],
        "quality refinement must keep the source region"
    );
    // Parametric-only overlays are rasterized and painted directly in corrected
    // space, avoiding lens inversion on each pointer frame.
    let expected_image = |lens| {
        let coverage = h.app.masks.stack.rasterize_component_region(
            0,
            0,
            [region.texture_width, region.texture_height],
            [
                region.source_x,
                region.source_y,
                region.source_width,
                region.source_height,
            ],
            [WIDTH, HEIGHT],
            lens,
        );
        egui::ColorImage::from_rgba_unmultiplied(
            refined.size,
            &coverage_rgba(coverage, mask_component_color(0)),
        )
    };
    assert!(refined.pixels == expected_image(None).pixels);
    assert!(
        refined.pixels != expected_image(loaded_lens_geometry(&h.app).map(Arc::as_ref)).pixels,
        "fixture must distinguish native and corrected coverage"
    );
    let settled = h.frame(vec![], Paint::Coverage);
    assert!(
        settled
            .textures_delta
            .set
            .iter()
            .all(|(texture_id, _)| *texture_id != id),
        "unchanged settled coverage should reuse the texture"
    );
}

#[test]
fn radial_move_repaints_pending_coverage_and_release_refines() {
    assert_pending_coverage_updates_and_release_refines(MaskKind::Radial, false);
}

#[test]
fn radial_rotation_repaints_pending_coverage_and_release_refines() {
    assert_pending_coverage_updates_and_release_refines(MaskKind::Radial, true);
}

#[test]
fn linear_move_repaints_pending_coverage_and_release_refines() {
    assert_pending_coverage_updates_and_release_refines(MaskKind::Linear, false);
}

#[test]
fn linear_rotation_repaints_pending_coverage_and_release_refines() {
    assert_pending_coverage_updates_and_release_refines(MaskKind::Linear, true);
}
