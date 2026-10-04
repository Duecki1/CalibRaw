use super::*;
use crate::app::{MaskStripActions, MaskStripCommand};

impl Sidebar {
    fn submask_drag_id() -> egui::Id {
        egui::Id::new("submask-component-drag")
    }

    pub(in crate::ui::sidebar) fn show_masks(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        layout: ScreenLayout,
        frame: &eframe::Frame,
    ) -> Option<egui::Rect> {
        Self::show_ai_update_card(ui, app, frame);

        if app.masks.stack.masks.is_empty() {
            moduwu_design::section_card(ui, "No masks yet", |_| {});
            return None;
        }

        match layout {
            ScreenLayout::Vertical => {
                Self::show_masks_vertical_details(ui, app, frame);
                None
            }
            ScreenLayout::Horizontal => Self::show_masks_horizontal_details(ui, app, frame),
        }
    }

    pub(crate) fn show_vertical_mask_strip(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
    ) {
        Self::show_mask_strip(ui, app, frame, MaskStripOrientation::Horizontal);
    }

    pub(crate) fn show_horizontal_mask_strip(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
    ) {
        Self::show_mask_strip(ui, app, frame, MaskStripOrientation::Vertical);
    }

    fn show_mask_strip(
        ui: &mut Ui,
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        orientation: MaskStripOrientation,
    ) {
        Self::requesting_new_content(app, frame, |app| {
            ui.spacing_mut().item_spacing =
                egui::vec2(moduwu_design::SPACE_XS, moduwu_design::SPACE_XXS);
            app.masks.stack.ensure_selection();
            Self::refresh_mask_thumbnails(ui, app);
            let actions = Self::show_mask_strip_contents(ui, &MaskStripInput::of(app), orientation);
            if app.apply_mask_strip_actions(ui.ctx(), frame, actions) {
                Self::refresh_mask_thumbnails(ui, app);
            }
            Self::show_mask_rename_dialog(ui.ctx(), app);
            Self::show_mask_delete_dialog(ui, app);
        });
    }

    /// Draw the group and sub-mask cards and return what the user changed.
    fn show_mask_strip_contents(
        ui: &mut Ui,
        input: &MaskStripInput<'_>,
        orientation: MaskStripOrientation,
    ) -> MaskStripActions {
        let pointer = ui.input(|input| StripPointer {
            position: input.pointer.interact_pos(),
            down: input.pointer.primary_down(),
            released: input.pointer.primary_released(),
        });
        let mut drag = StripDrag::load(ui.ctx());
        let mut requests = MaskStripRequests::default();
        let mut show_cards = |ui: &mut Ui| {
            Self::show_mask_cards(ui, input, orientation, pointer, &mut drag, &mut requests);
        };
        match orientation {
            MaskStripOrientation::Horizontal => {
                egui::ScrollArea::horizontal()
                    .id_salt("vertical-mask-card-strip")
                    .scroll_source(mask_strip_scroll_source())
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| show_cards(ui));
                    });
            }
            MaskStripOrientation::Vertical => {
                egui::ScrollArea::vertical()
                    .id_salt("horizontal-mask-card-strip")
                    .scroll_source(mask_strip_scroll_source())
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| show_cards(ui));
                    });
            }
        }

        let component_drop = drag.finish(ui, pointer);
        MaskStripActions {
            command: requests.command(ui.ctx(), component_drop),
            edits: requests.edits,
            open_group: drag.open_group,
        }
    }

    fn show_mask_cards(
        ui: &mut Ui,
        input: &MaskStripInput<'_>,
        orientation: MaskStripOrientation,
        pointer: StripPointer,
        drag: &mut StripDrag,
        requests: &mut MaskStripRequests,
    ) {
        ui.add_enabled_ui(input.can_add_group(), |ui| {
            Self::create_mask_group_card(ui, &mut requests.new_mask, orientation);
        });
        ui.add_space(moduwu_design::SPACE_XXS);

        for index in (0..input.masks.len()).rev() {
            Self::show_mask_group_card(ui, input, index, pointer, drag, requests);
            if input.selected_mask == Some(index) {
                ui.add_space(1.0);
                Self::show_submask_cards(ui, input, index, orientation, pointer, drag, requests);
                Self::create_submask_card(ui, &mut requests.add_component, orientation);
                ui.add_space(moduwu_design::SPACE_XXS);
            }
        }
    }

    fn show_mask_group_card(
        ui: &mut Ui,
        input: &MaskStripInput<'_>,
        index: usize,
        pointer: StripPointer,
        drag: &mut StripDrag,
        requests: &mut MaskStripRequests,
    ) {
        let mask = &input.masks[index];
        let badge = mask.components.len().to_string();
        let response = Self::mask_thumbnail_card(
            ui,
            input.group_textures.get(index),
            &mask.name,
            input.selected_mask == Some(index),
            Some(&badge),
            mask.enabled,
            MaskCardSize::Group,
        );
        let can_add_group = input.can_add_group();
        #[cfg(target_os = "android")]
        let overflow_clicked = {
            let menu_id = ui.make_persistent_id(("android-mask-group-overflow", index));
            crate::ui::android_overflow_menu(ui, response.rect, menu_id, 22.0, |ui| {
                Self::mask_group_context_menu(ui, mask, can_add_group, index, requests);
            })
            .clicked()
        };
        #[cfg(not(target_os = "android"))]
        let overflow_clicked = false;
        if response.clicked() && !overflow_clicked {
            requests.select_mask = Some(index);
        }
        if let (Some(state), Some(position)) = (&mut drag.state, pointer.position) {
            if response.rect.contains(position) {
                paint_drop_highlight(ui, response.rect);
                drag.hovered_group = Some(index);
                state.drop_target = Some((index, mask.components.len()));
                match state.hover_group {
                    Some((hovered, started)) if hovered == index => {
                        if started.elapsed() >= std::time::Duration::from_millis(650) {
                            drag.open_group = Some(index);
                        }
                    }
                    _ => {
                        state.hover_group = Some((index, std::time::Instant::now()));
                    }
                }
            }
        }
        moduwu_design::context_menu(&response, |ui| {
            Self::mask_group_context_menu(ui, mask, can_add_group, index, requests);
        });
    }

    fn show_submask_cards(
        ui: &mut Ui,
        input: &MaskStripInput<'_>,
        mask_index: usize,
        orientation: MaskStripOrientation,
        pointer: StripPointer,
        drag: &mut StripDrag,
        requests: &mut MaskStripRequests,
    ) {
        let component_count = input.masks[mask_index].components.len();
        for component_index in 0..component_count {
            if drag.displayed_drop_target == Some((mask_index, component_index)) {
                Self::show_submask_drop_placeholder(ui, pointer, drag);
            }
            let source_is_dragging = drag.state.as_ref().is_some_and(|state| {
                state.source_mask == mask_index && state.source_component == component_index
            });
            if source_is_dragging {
                continue;
            }
            Self::show_submask_card(
                ui,
                input,
                (mask_index, component_index),
                orientation,
                pointer,
                drag,
                requests,
            );
        }
        if drag
            .displayed_drop_target
            .is_some_and(|(mask, insert)| mask == mask_index && insert >= component_count)
        {
            Self::show_submask_drop_placeholder(ui, pointer, drag);
        }
    }

    /// Keep last frame's drop target while the pointer is over its placeholder.
    fn show_submask_drop_placeholder(ui: &mut Ui, pointer: StripPointer, drag: &mut StripDrag) {
        let placeholder = Self::submask_drop_placeholder(ui);
        if pointer
            .position
            .is_some_and(|position| placeholder.rect.contains(position))
        {
            if let Some(state) = &mut drag.state {
                state.drop_target = drag.displayed_drop_target;
                state.hover_group = None;
            }
        }
    }

    fn show_submask_card(
        ui: &mut Ui,
        input: &MaskStripInput<'_>,
        (mask_index, component_index): (usize, usize),
        orientation: MaskStripOrientation,
        pointer: StripPointer,
        drag: &mut StripDrag,
        requests: &mut MaskStripRequests,
    ) {
        let component_count = input.masks[mask_index].components.len();
        let component = &input.masks[mask_index].components[component_index];
        let component_badge = mask_component_badge(component_index, component.combine);
        let response = Self::mask_thumbnail_card(
            ui,
            input.component_textures.get(component_index),
            &component.name,
            input.selected_component == Some(component_index),
            Some(component_badge),
            component.enabled,
            MaskCardSize::Submask,
        );
        let component_can_drag = component_count > 1;
        if component_can_drag && response.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let menu = SubmaskMenu {
            mask_index,
            component_index,
            can_delete: component_count > 1,
            can_add_component: component_count < MAX_MASK_COMPONENTS,
        };
        #[cfg(target_os = "android")]
        let overflow_clicked = {
            let menu_id =
                ui.make_persistent_id(("android-submask-overflow", mask_index, component_index));
            crate::ui::android_overflow_menu(ui, response.rect, menu_id, 20.0, |ui| {
                Self::submask_context_menu(ui, component, menu, requests);
            })
            .clicked()
        };
        #[cfg(not(target_os = "android"))]
        let overflow_clicked = false;
        if response.clicked() && !overflow_clicked {
            requests.select_component = Some(component_index);
        }
        if response.drag_started() && component_can_drag {
            drag.state = Some(SubmaskDragState {
                source_mask: mask_index,
                source_component: component_index,
                source_texture: input.component_textures.get(component_index).cloned(),
                source_name: component.name.clone(),
                source_badge: component_badge.to_owned(),
                source_enabled: component.enabled,
                hover_group: None,
                drop_target: Some((mask_index, component_index)),
                target_loss_started: None,
            });
        }
        if let (Some(state), Some(position)) = (&mut drag.state, pointer.position) {
            if response.rect.contains(position) {
                paint_drop_highlight(ui, response.rect);
                let before = match orientation {
                    MaskStripOrientation::Horizontal => position.x < response.rect.center().x,
                    MaskStripOrientation::Vertical => position.y < response.rect.center().y,
                };
                state.drop_target = Some((mask_index, component_index + usize::from(!before)));
                state.hover_group = None;
            }
        }
        moduwu_design::context_menu(&response, |ui| {
            Self::submask_context_menu(ui, component, menu, requests);
        });
    }
}

/// The mask stack and thumbnails the strip shows.
struct MaskStripInput<'a> {
    masks: &'a [LocalMask],
    selected_mask: Option<usize>,
    selected_component: Option<usize>,
    group_textures: &'a [egui::TextureHandle],
    /// Thumbnails of the selected group's sub-masks.
    component_textures: &'a [egui::TextureHandle],
}

impl<'a> MaskStripInput<'a> {
    fn of(app: &'a CalibRawApp) -> Self {
        Self {
            masks: &app.masks.stack.masks,
            selected_mask: app.masks.stack.selected_mask,
            selected_component: app.masks.stack.selected_component,
            group_textures: &app.masks.thumbnail_group_textures,
            component_textures: &app.masks.thumbnail_component_textures,
        }
    }

    fn can_add_group(&self) -> bool {
        self.masks.len() < MAX_LOCAL_MASKS
    }
}

#[derive(Clone, Copy)]
struct StripPointer {
    position: Option<egui::Pos2>,
    down: bool,
    released: bool,
}

/// A sub-mask drag across frames, kept in egui memory.
struct StripDrag {
    state: Option<SubmaskDragState>,
    /// Where last frame showed the drop placeholder.
    displayed_drop_target: Option<(usize, usize)>,
    hovered_group: Option<usize>,
    open_group: Option<usize>,
}

impl StripDrag {
    fn load(ctx: &egui::Context) -> Self {
        let mut state =
            ctx.data(|data| data.get_temp::<SubmaskDragState>(Sidebar::submask_drag_id()));
        let displayed_drop_target = state.as_ref().and_then(|drag| drag.drop_target);
        if let Some(drag) = &mut state {
            drag.drop_target = None;
        }
        Self {
            state,
            displayed_drop_target,
            hovered_group: None,
            open_group: None,
        }
    }

    /// Keep a briefly lost drop target, paint the floating card, and store
    /// the drag. Returns the drag and its target when the pointer is released.
    fn finish(
        &mut self,
        ui: &Ui,
        pointer: StripPointer,
    ) -> Option<(SubmaskDragState, (usize, usize))> {
        if let Some(drag) = &mut self.state {
            if drag.drop_target.is_some() {
                drag.target_loss_started = None;
            } else if let Some(previous_target) = self.displayed_drop_target {
                let lost_at = drag
                    .target_loss_started
                    .get_or_insert_with(std::time::Instant::now);
                if lost_at.elapsed() < std::time::Duration::from_millis(120) {
                    drag.drop_target = Some(previous_target);
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(16));
                }
            }
            if self.hovered_group.is_none() && drag.drop_target.is_none() {
                drag.hover_group = None;
            }
            if drag.drop_target.is_none() {
                if let Some(position) = pointer.position {
                    Sidebar::paint_floating_submask(ui, drag, position);
                }
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }

        let component_drop = if pointer.released {
            self.state
                .take()
                .and_then(|drag| drag.drop_target.map(|target| (drag, target)))
        } else {
            None
        };
        if !pointer.down && !pointer.released {
            self.state = None;
        }
        ui.ctx().data_mut(|data| {
            if let Some(drag) = self.state.clone() {
                data.insert_temp(Sidebar::submask_drag_id(), drag);
            } else {
                data.remove::<SubmaskDragState>(Sidebar::submask_drag_id());
            }
        });
        component_drop
    }
}

impl MaskStripRequests {
    /// The one structural change of the frame, by priority: a drop, then
    /// copies, then creation, then selection.
    fn command(
        &self,
        ctx: &egui::Context,
        component_drop: Option<(SubmaskDragState, (usize, usize))>,
    ) -> Option<MaskStripCommand> {
        if let Some((drag, (target_mask, target_insert))) = component_drop {
            return Some(MaskStripCommand::MoveComponent {
                source_mask: drag.source_mask,
                source_component: drag.source_component,
                target_mask,
                target_insert,
            });
        }
        if let Some((mask_index, invert)) = self.duplicate_mask {
            return Some(MaskStripCommand::DuplicateGroup { mask_index, invert });
        }
        if let Some(mask_index) = self.paste_mask {
            return Sidebar::copied_mask_group(ctx).map(|mask| MaskStripCommand::PasteGroup {
                mask_index,
                mask: Box::new(mask),
            });
        }
        if let Some((mask_index, component_index, invert)) = self.duplicate_component {
            return Some(MaskStripCommand::DuplicateComponent {
                mask_index,
                component_index,
                invert,
            });
        }
        if let Some((mask_index, component_index)) = self.paste_component {
            return Sidebar::copied_mask_component(ctx).map(|component| {
                MaskStripCommand::PasteComponent {
                    mask_index,
                    component_index,
                    component: Box::new(component),
                }
            });
        }
        if let Some(kind) = self.new_mask {
            return Some(MaskStripCommand::CreateGroup(kind));
        }
        if let Some((kind, combine)) = self.add_component {
            return Some(MaskStripCommand::AddComponent(kind, combine));
        }
        if let Some(mask_index) = self.select_mask {
            return Some(MaskStripCommand::SelectGroup(mask_index));
        }
        self.select_component.map(MaskStripCommand::SelectComponent)
    }
}

fn paint_drop_highlight(ui: &Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        5.0,
        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
        egui::StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag_state() -> SubmaskDragState {
        SubmaskDragState {
            source_mask: 1,
            source_component: 2,
            source_texture: None,
            source_name: String::new(),
            source_badge: String::new(),
            source_enabled: true,
            hover_group: None,
            drop_target: None,
            target_loss_started: None,
        }
    }

    #[test]
    fn a_drop_wins_over_copies_and_copies_over_selection() {
        let ctx = egui::Context::default();
        let requests = MaskStripRequests {
            duplicate_mask: Some((0, true)),
            select_mask: Some(1),
            ..Default::default()
        };
        assert!(matches!(
            requests.command(&ctx, Some((drag_state(), (0, 1)))),
            Some(MaskStripCommand::MoveComponent {
                source_mask: 1,
                source_component: 2,
                target_mask: 0,
                target_insert: 1,
            })
        ));
        assert!(matches!(
            requests.command(&ctx, None),
            Some(MaskStripCommand::DuplicateGroup {
                mask_index: 0,
                invert: true,
            })
        ));
        // A paste with an empty clipboard does nothing, and nothing after it runs.
        let paste = MaskStripRequests {
            paste_mask: Some(0),
            select_mask: Some(1),
            ..Default::default()
        };
        assert!(paste.command(&ctx, None).is_none());
    }
}
