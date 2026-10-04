//! Edits from the mask strip (`ui::sidebar::masks::strip`): context-menu
//! toggles, group and sub-mask creation, selection, copies and sub-mask
//! moves. The strip returns `MaskStripActions` after drawing;
//! `apply_mask_strip_actions` applies them within the same frame.

use super::*;
use crate::pipeline::{LocalMask, MaskCombineMode, MaskComponent};

/// What the strip changed this frame, in the order it is applied.
#[derive(Default)]
pub(crate) struct MaskStripActions {
    /// Enabled and invert changes from the context menus.
    pub(crate) edits: Vec<MaskStripEdit>,
    /// A dragged sub-mask has hovered over this group long enough to open it.
    pub(crate) open_group: Option<usize>,
    /// At most one structural change per frame.
    pub(crate) command: Option<MaskStripCommand>,
}

pub(crate) enum MaskStripEdit {
    SetGroupEnabled {
        mask_index: usize,
        enabled: bool,
    },
    ToggleGroupInvert(usize),
    SetComponentEnabled {
        mask_index: usize,
        component_index: usize,
        enabled: bool,
    },
    ToggleComponentInvert {
        mask_index: usize,
        component_index: usize,
    },
}

pub(crate) enum MaskStripCommand {
    MoveComponent {
        source_mask: usize,
        source_component: usize,
        target_mask: usize,
        /// Insertion index in the target group, before the move.
        target_insert: usize,
    },
    DuplicateGroup {
        mask_index: usize,
        invert: bool,
    },
    /// Insert the copied group after `mask_index`.
    PasteGroup {
        mask_index: usize,
        mask: Box<LocalMask>,
    },
    DuplicateComponent {
        mask_index: usize,
        component_index: usize,
        invert: bool,
    },
    /// Insert the copied sub-mask after `component_index`.
    PasteComponent {
        mask_index: usize,
        component_index: usize,
        component: Box<MaskComponent>,
    },
    CreateGroup(MaskKind),
    AddComponent(MaskKind, MaskCombineMode),
    SelectGroup(usize),
    /// Select a sub-mask of the selected group.
    SelectComponent(usize),
}

/// Layers whose coverage or adjustments the menu edits changed.
#[derive(Default)]
struct MenuEditsDirty {
    adjustments: bool,
    group_geometry: Option<usize>,
    component_geometry: Option<usize>,
}

impl CalibRawApp {
    /// Apply the strip's actions. Returns true when the mask thumbnails
    /// must be refreshed.
    pub(crate) fn apply_mask_strip_actions(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        actions: MaskStripActions,
    ) -> bool {
        let mut dirty = MenuEditsDirty::default();
        for edit in actions.edits {
            self.apply_mask_strip_edit(edit, &mut dirty);
        }
        if let Some(mask_index) = actions.open_group {
            if self.masks.stack.select_mask(mask_index) {
                self.masks.thumbnail_component_mask = None;
                ctx.request_repaint();
            }
        }
        if dirty.adjustments {
            self.mark_mask_adjustments_dirty();
        }
        if let Some(mask_index) = dirty.group_geometry {
            self.mark_mask_geometry_dirty(mask_index);
        }
        if let Some(mask_index) = dirty.component_geometry {
            self.mark_mask_geometry_dirty(mask_index);
        }
        actions
            .command
            .is_some_and(|command| self.apply_mask_strip_command(frame, command))
    }

    fn apply_mask_strip_edit(&mut self, edit: MaskStripEdit, dirty: &mut MenuEditsDirty) {
        match edit {
            MaskStripEdit::SetGroupEnabled {
                mask_index,
                enabled,
            } => {
                if let Some(mask) = self.masks.stack.masks.get_mut(mask_index) {
                    dirty.adjustments |= mask.common.set_enabled(enabled);
                }
            }
            MaskStripEdit::ToggleGroupInvert(mask_index) => {
                if let Some(mask) = self.masks.stack.masks.get_mut(mask_index) {
                    mask.common.toggle_invert();
                    dirty.group_geometry = Some(mask_index);
                }
            }
            MaskStripEdit::SetComponentEnabled {
                mask_index,
                component_index,
                enabled,
            } => {
                if let Some(component) = self.mask_component_mut(mask_index, component_index) {
                    if component.common.set_enabled(enabled) {
                        dirty.component_geometry = Some(mask_index);
                    }
                }
            }
            MaskStripEdit::ToggleComponentInvert {
                mask_index,
                component_index,
            } => {
                if let Some(component) = self.mask_component_mut(mask_index, component_index) {
                    component.common.toggle_invert();
                    dirty.component_geometry = Some(mask_index);
                }
            }
        }
    }

    fn mask_component_mut(
        &mut self,
        mask_index: usize,
        component_index: usize,
    ) -> Option<&mut MaskComponent> {
        self.masks
            .stack
            .masks
            .get_mut(mask_index)?
            .components
            .get_mut(component_index)
    }

    /// Returns true when the mask thumbnails must be refreshed.
    fn apply_mask_strip_command(
        &mut self,
        frame: &eframe::Frame,
        command: MaskStripCommand,
    ) -> bool {
        match command {
            MaskStripCommand::MoveComponent {
                source_mask,
                source_component,
                target_mask,
                target_insert,
            } => {
                let moved = self
                    .masks
                    .stack
                    .move_submask_component(
                        source_mask,
                        source_component,
                        target_mask,
                        target_insert,
                    )
                    .is_some();
                if moved {
                    self.mark_all_mask_layers_dirty();
                    self.sync_selected_mask_tool();
                }
                moved
            }
            MaskStripCommand::DuplicateGroup { mask_index, invert } => {
                self.commit_mask_change("Mask-group copy", None, false, |stack| {
                    stack.duplicate_mask(mask_index, invert)
                })
            }
            MaskStripCommand::PasteGroup { mask_index, mask } => {
                self.commit_mask_change("Mask-group copy", None, false, |stack| {
                    stack.insert_mask_copy(mask_index, *mask, false)
                })
            }
            MaskStripCommand::DuplicateComponent {
                mask_index,
                component_index,
                invert,
            } => self.commit_mask_change("Sub-mask copy", Some(mask_index), true, |stack| {
                stack.duplicate_component(mask_index, component_index, invert)
            }),
            MaskStripCommand::PasteComponent {
                mask_index,
                component_index,
                component,
            } => self.commit_mask_change("Sub-mask copy", Some(mask_index), true, |stack| {
                stack.insert_component_copy(mask_index, component_index, *component, false)
            }),
            MaskStripCommand::CreateGroup(kind) => {
                let Some((mask_index, _)) = self.masks.stack.add_mask(kind) else {
                    return false;
                };
                self.start_new_mask_component(frame, kind, mask_index);
                self.blink_selected_mask();
                true
            }
            MaskStripCommand::AddComponent(kind, combine) => {
                let Some((mask_index, _)) = self.masks.stack.add_component(kind, combine) else {
                    return false;
                };
                self.start_new_mask_component(frame, kind, mask_index);
                self.blink_selected_component();
                true
            }
            MaskStripCommand::SelectGroup(mask_index) => {
                let selected = self.masks.stack.select_mask(mask_index);
                if selected {
                    self.sync_selected_mask_tool();
                    self.blink_selected_mask();
                }
                selected
            }
            MaskStripCommand::SelectComponent(component_index) => {
                if let Some(mask_index) = self.masks.stack.selected_mask {
                    if self
                        .masks
                        .stack
                        .select_component(mask_index, component_index)
                    {
                        self.sync_selected_mask_tool();
                        self.blink_selected_component();
                    }
                }
                false
            }
        }
    }

    /// Activate the tool of a newly added component and start what its
    /// content needs (an AI mask or a captured source).
    fn start_new_mask_component(
        &mut self,
        frame: &eframe::Frame,
        kind: MaskKind,
        mask_index: usize,
    ) {
        self.activate_mask_tool(kind);
        self.prepare_content_mask(frame, kind);
        self.mark_mask_geometry_dirty(mask_index);
        self.masks.thumbnail_component_mask = None;
    }

    fn prepare_content_mask(&mut self, frame: &eframe::Frame, kind: MaskKind) {
        match kind {
            MaskKind::Subject | MaskKind::Background => self.request_subject_mask(frame),
            MaskKind::Sky => self.request_sky_mask(frame),
            MaskKind::DepthRange => self.request_depth_mask(frame),
            MaskKind::Object => {
                if let Err(error) = self.capture_mask_source(frame) {
                    self.report_ai_mask_error(error);
                }
            }
            MaskKind::LuminanceRange | MaskKind::ColorRange => {
                if let Err(error) = self.capture_mask_source(frame) {
                    self.ui.status = error;
                    return;
                }
                let source = self.masks.source_cache.clone();
                if let Some(component) = self.masks.stack.selected_component_mut() {
                    match &mut component.geometry {
                        MaskGeometry::LuminanceRange { source: target, .. }
                        | MaskGeometry::ColorRange { source: target, .. } => *target = source,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Apply `edit` to a copy of the stack and keep it only if it changed
    /// the stack and the sidecar can still store it.
    fn commit_mask_change(
        &mut self,
        action: &str,
        dirty_layer: Option<usize>,
        component_selection: bool,
        edit: impl FnOnce(&mut MaskStack) -> bool,
    ) -> bool {
        let mut candidate = self.masks.stack.clone();
        if !edit(&mut candidate) {
            return false;
        }
        if let Err(error) = crate::sidecar::preflight_mask_change(&candidate) {
            self.report_mask_persistence_limit(action, &error);
            return false;
        }
        self.masks.stack = candidate;
        if let Some(mask_index) = dirty_layer {
            self.mark_mask_geometry_dirty(mask_index);
        } else {
            self.mark_all_mask_layers_dirty();
        }
        self.sync_selected_mask_tool();
        if component_selection {
            self.blink_selected_component();
        } else {
            self.blink_selected_mask();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_masks(ctx: &egui::Context) -> CalibRawApp {
        let mut app = CalibRawApp::empty(ctx);
        app.masks.stack.add_mask(MaskKind::Brush).unwrap();
        app.masks.stack.add_mask(MaskKind::Radial).unwrap();
        app.masks
            .stack
            .add_component(MaskKind::Linear, MaskCombineMode::Subtract)
            .unwrap();
        app.masks.dirty_layers = [false; MAX_LOCAL_MASKS];
        app
    }

    #[test]
    fn menu_edits_toggle_state_and_mark_their_layers() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_masks(&ctx);
        let refresh = app.apply_mask_strip_actions(
            &ctx,
            &frame,
            MaskStripActions {
                edits: vec![
                    MaskStripEdit::ToggleGroupInvert(0),
                    MaskStripEdit::SetComponentEnabled {
                        mask_index: 1,
                        component_index: 1,
                        enabled: false,
                    },
                ],
                ..Default::default()
            },
        );
        assert!(!refresh);
        assert!(app.masks.stack.masks[0].invert);
        assert!(!app.masks.stack.masks[1].components[1].enabled);
        assert!(app.masks.dirty_layers[0] && app.masks.dirty_layers[1]);
    }

    #[test]
    fn structural_commands_request_a_thumbnail_refresh() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_masks(&ctx);
        let command = |command| MaskStripActions {
            command: Some(command),
            ..Default::default()
        };

        let copy = Box::new(app.masks.stack.masks[0].clone());
        assert!(app.apply_mask_strip_actions(
            &ctx,
            &frame,
            command(MaskStripCommand::PasteGroup {
                mask_index: 0,
                mask: copy,
            }),
        ));
        assert_eq!(app.masks.stack.masks.len(), 3);

        assert!(app.apply_mask_strip_actions(
            &ctx,
            &frame,
            command(MaskStripCommand::SelectGroup(2))
        ));
        assert_eq!(app.masks.stack.selected_mask, Some(2));
        // Selecting a sub-mask keeps the group's thumbnails.
        assert!(!app.apply_mask_strip_actions(
            &ctx,
            &frame,
            command(MaskStripCommand::SelectComponent(1)),
        ));
        assert_eq!(app.masks.stack.selected_component, Some(1));
    }
}
