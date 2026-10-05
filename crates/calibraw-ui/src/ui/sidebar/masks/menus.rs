use super::*;

impl Sidebar {
    pub(super) fn mask_kind_menu(ui: &mut Ui, unavailable_message: &str) -> Option<MaskKind> {
        let mut selected = None;
        for kind in [
            MaskKind::Fullscreen,
            MaskKind::Brush,
            MaskKind::Radial,
            MaskKind::Linear,
            MaskKind::Path,
            MaskKind::Subject,
            MaskKind::Background,
            MaskKind::Sky,
            MaskKind::Object,
            MaskKind::LuminanceRange,
            MaskKind::ColorRange,
            MaskKind::DepthRange,
        ] {
            let label = if kind.is_available() {
                kind.label().to_owned()
            } else {
                format!("{} · soon", kind.label())
            };
            let response = ui
                .add_enabled_ui(kind.is_available(), |ui| ui.selectable_label(false, label))
                .inner
                .on_disabled_hover_text(unavailable_message);
            if response.clicked() {
                selected = Some(kind);
                ui.close();
            }
        }
        selected
    }

    pub(super) fn submask_creation_menu(
        ui: &mut Ui,
        unavailable_message: &str,
    ) -> Option<(MaskKind, MaskCombineMode)> {
        let mut selected = None;
        ui.label(egui::RichText::new("Combine as").weak());
        for combine in [
            MaskCombineMode::Add,
            MaskCombineMode::Subtract,
            MaskCombineMode::Intersect,
        ] {
            moduwu_design::dropdown_submenu(ui, combine.label(), |ui| {
                if let Some(kind) = Self::mask_kind_menu(ui, unavailable_message) {
                    selected = Some((kind, combine));
                }
            });
        }
        selected
    }

    pub(super) fn mask_group_context_menu(
        ui: &mut Ui,
        mask: &LocalMask,
        can_add_group: bool,
        mask_index: usize,
        requests: &mut MaskStripRequests,
    ) {
        if moduwu_design::menu_item(ui, true, "Rename…").clicked() {
            Self::open_mask_rename_dialog(
                ui.ctx(),
                MaskRenameTarget::Group(mask_index),
                mask.name.clone(),
            );
            ui.close();
        }
        ui.separator();
        let mut enabled = mask.enabled;
        if moduwu_design::toggle(ui, &mut enabled, "Enabled").changed() {
            requests.edits.push(MaskStripEdit::SetGroupEnabled {
                mask_index,
                enabled,
            });
        }
        if moduwu_design::menu_item(ui, can_add_group, "Duplicate").clicked() {
            requests.duplicate_mask = Some((mask_index, false));
            ui.close();
        }
        if moduwu_design::toggle_button(ui, "Invert", mask.invert).clicked() {
            requests
                .edits
                .push(MaskStripEdit::ToggleGroupInvert(mask_index));
            ui.close();
        }
        if moduwu_design::menu_item(ui, can_add_group, "Duplicate & Invert").clicked() {
            requests.duplicate_mask = Some((mask_index, true));
            ui.close();
        }
        ui.separator();
        if moduwu_design::menu_item(ui, true, "Copy Mask Group").clicked() {
            ui.ctx().data_mut(|data| {
                data.insert_temp(Self::mask_group_clipboard_id(), mask.clone());
            });
            ui.close();
        }
        let can_paste = can_add_group && Self::copied_mask_group(ui.ctx()).is_some();
        if moduwu_design::menu_item(ui, can_paste, "Paste Mask Group")
            .on_disabled_hover_text("Copy a mask group first")
            .clicked()
        {
            requests.paste_mask = Some(mask_index);
            ui.close();
        }
        ui.separator();
        if moduwu_design::destructive_menu_item(
            ui,
            format!("{}  Delete mask group", egui_phosphor::regular::TRASH),
        )
        .clicked()
        {
            Self::open_mask_delete_dialog(
                ui.ctx(),
                MaskDeleteTarget::Group(mask_index),
                mask.name.clone(),
            );
            ui.close();
        }
    }

    pub(super) fn submask_context_menu(
        ui: &mut Ui,
        component: &MaskComponent,
        menu: SubmaskMenu,
        requests: &mut MaskStripRequests,
    ) {
        let SubmaskMenu {
            mask_index,
            component_index,
            can_delete,
            can_add_component,
        } = menu;
        if moduwu_design::menu_item(ui, true, "Rename…").clicked() {
            Self::open_mask_rename_dialog(
                ui.ctx(),
                MaskRenameTarget::Component {
                    mask_index,
                    component_index,
                },
                component.name.clone(),
            );
            ui.close();
        }
        ui.separator();
        let mut enabled = component.enabled;
        if moduwu_design::toggle(ui, &mut enabled, "Enabled").changed() {
            requests.edits.push(MaskStripEdit::SetComponentEnabled {
                mask_index,
                component_index,
                enabled,
            });
        }
        if moduwu_design::menu_item(ui, can_add_component, "Duplicate").clicked() {
            requests.duplicate_component = Some((mask_index, component_index, false));
            ui.close();
        }
        if moduwu_design::toggle_button(ui, "Invert", component.invert).clicked() {
            requests.edits.push(MaskStripEdit::ToggleComponentInvert {
                mask_index,
                component_index,
            });
            ui.close();
        }
        if moduwu_design::menu_item(ui, can_add_component, "Duplicate & Invert").clicked() {
            requests.duplicate_component = Some((mask_index, component_index, true));
            ui.close();
        }
        ui.separator();
        if moduwu_design::menu_item(ui, true, "Copy Component").clicked() {
            ui.ctx().data_mut(|data| {
                data.insert_temp(Self::mask_component_clipboard_id(), component.clone());
            });
            ui.close();
        }
        let can_paste = can_add_component && Self::copied_mask_component(ui.ctx()).is_some();
        if moduwu_design::menu_item(ui, can_paste, "Paste Component")
            .on_disabled_hover_text("Copy a component first")
            .clicked()
        {
            requests.paste_component = Some((mask_index, component_index));
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled_ui(can_delete, |ui| {
                moduwu_design::destructive_menu_item(
                    ui,
                    format!("{}  Delete sub-mask", egui_phosphor::regular::TRASH),
                )
            })
            .inner
            .on_disabled_hover_text("A mask group must contain at least one sub-mask")
            .clicked()
        {
            Self::open_mask_delete_dialog(
                ui.ctx(),
                MaskDeleteTarget::Component {
                    mask_index,
                    component_index,
                },
                component.name.clone(),
            );
            ui.close();
        }
    }

    fn mask_group_clipboard_id() -> egui::Id {
        egui::Id::new("mask-group-clipboard")
    }

    fn mask_component_clipboard_id() -> egui::Id {
        egui::Id::new("mask-component-clipboard")
    }

    fn mask_rename_dialog_id() -> egui::Id {
        egui::Id::new("mask-rename-dialog-state")
    }

    fn mask_delete_dialog_id() -> egui::Id {
        egui::Id::new("mask-delete-dialog-state")
    }

    pub(crate) fn mask_dialog_open(ctx: &egui::Context) -> bool {
        ctx.data(|data| {
            data.get_temp::<MaskRenameDialog>(Self::mask_rename_dialog_id())
                .is_some()
                || data
                    .get_temp::<MaskDeleteDialog>(Self::mask_delete_dialog_id())
                    .is_some()
        })
    }

    pub(super) fn copied_mask_group(ctx: &egui::Context) -> Option<LocalMask> {
        ctx.data(|data| data.get_temp::<LocalMask>(Self::mask_group_clipboard_id()))
    }

    pub(super) fn copied_mask_component(ctx: &egui::Context) -> Option<MaskComponent> {
        ctx.data(|data| data.get_temp::<MaskComponent>(Self::mask_component_clipboard_id()))
    }

    fn open_mask_rename_dialog(ctx: &egui::Context, target: MaskRenameTarget, name: String) {
        ctx.data_mut(|data| {
            data.insert_temp(
                Self::mask_rename_dialog_id(),
                MaskRenameDialog {
                    target,
                    name,
                    focus_requested: false,
                },
            );
        });
    }

    fn open_mask_delete_dialog(ctx: &egui::Context, target: MaskDeleteTarget, name: String) {
        ctx.data_mut(|data| {
            data.insert_temp(
                Self::mask_delete_dialog_id(),
                MaskDeleteDialog { target, name },
            );
        });
    }

    pub(super) fn show_mask_rename_dialog(ctx: &egui::Context, app: &mut CalibRawApp) {
        let Some(mut dialog) =
            ctx.data(|data| data.get_temp::<MaskRenameDialog>(Self::mask_rename_dialog_id()))
        else {
            return;
        };

        let title = match &dialog.target {
            MaskRenameTarget::Group(_) => "Rename mask group",
            MaskRenameTarget::Component { .. } => "Rename sub-mask",
        };
        let mut save = false;
        let mut cancel = false;
        moduwu_design::dialog_window(title, ctx, moduwu_design::DIALOG_WIDTH_NARROW)
            .id(egui::Id::new("mask-rename-dialog-window"))
            .show(ctx, |ui| {
                let response =
                    moduwu_design::dialog_text_field(ui, &mut dialog.name, "mask-rename-input", "");
                moduwu_design::request_initial_focus(&response, &mut dialog.focus_requested);
                let trimmed_is_empty = dialog.name.trim().is_empty();
                match moduwu_design::dialog_confirmation_buttons(
                    ui,
                    "Cancel",
                    "Rename",
                    !trimmed_is_empty,
                    false,
                    moduwu_design::DialogKeyboard::CONFIRM_ON_ENTER,
                ) {
                    moduwu_design::DialogAction::Cancel => cancel = true,
                    moduwu_design::DialogAction::Confirm => save = true,
                    moduwu_design::DialogAction::None => {}
                }
            });

        if save {
            let renamed = match dialog.target {
                MaskRenameTarget::Group(mask_index) => app
                    .masks
                    .stack
                    .masks
                    .get_mut(mask_index)
                    .is_some_and(|mask| mask.common.rename(dialog.name.trim())),
                MaskRenameTarget::Component {
                    mask_index,
                    component_index,
                } => app
                    .masks
                    .stack
                    .masks
                    .get_mut(mask_index)
                    .and_then(|mask| mask.components.get_mut(component_index))
                    .is_some_and(|component| component.common.rename(dialog.name.trim())),
            };
            if renamed {
                app.note_mask_edit_changed();
            }
            ctx.data_mut(|data| data.remove::<MaskRenameDialog>(Self::mask_rename_dialog_id()));
        } else if cancel {
            ctx.data_mut(|data| data.remove::<MaskRenameDialog>(Self::mask_rename_dialog_id()));
        } else {
            ctx.data_mut(|data| data.insert_temp(Self::mask_rename_dialog_id(), dialog));
        }
    }

    pub(super) fn show_mask_delete_dialog(ui: &mut Ui, app: &mut CalibRawApp) {
        let ctx = ui.ctx().clone();
        let Some(dialog) =
            ctx.data(|data| data.get_temp::<MaskDeleteDialog>(Self::mask_delete_dialog_id()))
        else {
            return;
        };

        let (title, message, confirm_label) = match &dialog.target {
            MaskDeleteTarget::Group(_) => (
                "Delete mask group?",
                format!("Delete the mask group “{}”?", dialog.name),
                "Delete Group",
            ),
            MaskDeleteTarget::Component { .. } => (
                "Delete sub-mask?",
                format!("Delete the sub-mask “{}”?", dialog.name),
                "Delete Sub-mask",
            ),
        };
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::dialog_window(title, &ctx, moduwu_design::DIALOG_WIDTH_NARROW)
            .id(egui::Id::new("mask-delete-dialog-window"))
            .show(&ctx, |ui| {
                ui.label(message);
                action = moduwu_design::dialog_confirmation_buttons(
                    ui,
                    "Cancel",
                    confirm_label,
                    true,
                    true,
                    moduwu_design::DialogKeyboard::CLOSE_ONLY,
                );
            });

        match action {
            moduwu_design::DialogAction::Cancel => {
                ctx.data_mut(|data| {
                    data.remove::<MaskDeleteDialog>(Self::mask_delete_dialog_id());
                });
            }
            moduwu_design::DialogAction::Confirm => {
                let changed = match dialog.target {
                    MaskDeleteTarget::Group(mask_index) => {
                        if app.masks.stack.delete_mask(mask_index) {
                            app.develop_ui.mask_point_color = Default::default();
                            app.develop_ui.mask_point_color_mask = None;
                            app.mark_all_mask_layers_dirty();
                            app.sync_selected_mask_tool();
                            true
                        } else {
                            false
                        }
                    }
                    MaskDeleteTarget::Component {
                        mask_index,
                        component_index,
                    } => {
                        if app
                            .masks
                            .stack
                            .delete_component(mask_index, component_index)
                        {
                            app.mark_mask_geometry_dirty(mask_index);
                            app.sync_selected_mask_tool();
                            true
                        } else {
                            false
                        }
                    }
                };
                ctx.data_mut(|data| {
                    data.remove::<MaskDeleteDialog>(Self::mask_delete_dialog_id());
                });
                if changed {
                    Self::refresh_mask_thumbnails(ui, app);
                }
            }
            moduwu_design::DialogAction::None => {}
        }
    }
}
