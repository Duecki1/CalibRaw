//! Drive the inpaint tool with pointer frames and inspect the brush state.
#![cfg(not(target_os = "android"))]

use super::*;
use crate::app::InpaintTool;
use crate::pipeline::LoadedRaw;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
const SOURCE: Pos2 = Pos2::new(150.0, 150.0);
const STROKE: Pos2 = Pos2::new(300.0, 200.0);

fn image_rect() -> Rect {
    Rect::from_min_size(
        egui::pos2(50.0, 50.0),
        egui::vec2(WIDTH as f32, HEIGHT as f32),
    )
}

fn screen_rect() -> Rect {
    Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 400.0))
}

struct Harness {
    context: egui::Context,
    app: CalibRawApp,
    frame: eframe::Frame,
}

impl Harness {
    fn new(tool: InpaintTool) -> Self {
        let context = egui::Context::default();
        let mut app = CalibRawApp::empty(&context);
        let raw = LoadedRaw::from_scene_linear_rec2020(
            WIDTH,
            HEIGHT,
            vec![0.2; (WIDTH * HEIGHT * 3) as usize],
        )
        .unwrap();
        app.develop.loaded_raw = Some(Arc::new(raw));
        app.ui.sidebar_tab = SidebarTab::Inpainting;
        app.inpaint.tool = tool;
        let mut harness = Self {
            context,
            app,
            frame: eframe::Frame::_new_kittest(),
        };
        // Register the interaction rectangle before sending its first press.
        harness.run(vec![], egui::Modifiers::NONE);
        harness.run(vec![], egui::Modifiers::NONE);
        harness
    }

    fn run(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        let (app, frame) = (&mut self.app, &self.frame);
        let _ = self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(screen_rect()),
                events,
                modifiers,
                ..Default::default()
            },
            |ui| {
                let response = ui.allocate_rect(screen_rect(), Sense::click_and_drag());
                let layout = PreviewLayout {
                    image_rect: image_rect(),
                    visible_rect: image_rect(),
                    viewport_rect: screen_rect(),
                    source_width: WIDTH,
                    source_height: HEIGHT,
                };
                Preview::handle_inpaint_interaction(ui, app, frame, layout, &response);
            },
        );
    }

    fn press(&mut self, position: Pos2, modifiers: egui::Modifiers) {
        self.run(
            vec![
                egui::Event::PointerMoved(position),
                button(position, true, modifiers),
            ],
            modifiers,
        );
    }

    fn move_to(&mut self, position: Pos2) {
        self.run(
            vec![egui::Event::PointerMoved(position)],
            egui::Modifiers::NONE,
        );
    }

    fn release(&mut self, position: Pos2) {
        self.run(
            vec![button(position, false, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
    }

    fn assert_source_placed_without_stroke(&self) {
        let expected = [
            SOURCE.x - image_rect().left(),
            SOURCE.y - image_rect().top(),
        ];
        assert_eq!(self.app.inpaint.source_point, Some(expected));
        assert!(self.app.inpaint.active_points.is_empty());
        assert!(self.app.inpaint.pending_brush.is_none());
        // Starting a retouch without a GPU reports this; no stroke must start.
        assert!(self.app.ui.notice.is_none());
    }
}

fn button(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

#[test]
fn picked_source_press_does_not_continue_as_a_stroke() {
    for tool in [InpaintTool::Clone, InpaintTool::Heal] {
        let mut harness = Harness::new(tool);
        harness.app.inpaint.source_pick_active = true;

        harness.press(SOURCE, egui::Modifiers::NONE);
        harness.move_to(STROKE);
        harness.release(STROKE);
        harness.move_to(STROKE);

        assert!(!harness.app.inpaint.source_pick_active);
        harness.assert_source_placed_without_stroke();

        // The next press paints normally.
        harness.press(STROKE, egui::Modifiers::NONE);
        harness.move_to(STROKE + egui::vec2(20.0, 0.0));
        assert!(!harness.app.inpaint.active_points.is_empty());
    }
}

#[test]
fn releasing_the_source_modifier_before_the_button_does_not_paint() {
    let mut harness = Harness::new(InpaintTool::Clone);

    harness.press(SOURCE, egui::Modifiers::COMMAND);
    harness.move_to(STROKE);
    harness.release(STROKE);
    harness.move_to(STROKE);

    harness.assert_source_placed_without_stroke();
}
