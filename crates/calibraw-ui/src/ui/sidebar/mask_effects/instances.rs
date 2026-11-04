//! The row of an effect card that lists its instances (Relight's lights, Light
//! Rays' sources): one chip per instance in its colour, then Add and Remove.
//! The card shows the controls of the selected instance below the row.

use super::{egui, Ui};
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::EffectInstances;

/// What the row calls the effect's instances.
pub(super) struct InstanceNames {
    /// One instance, with its `{number}`.
    pub(super) item: &'static str,
    pub(super) add: &'static str,
    pub(super) remove: &'static str,
    /// The Add button's help while the card or its effect list is full.
    pub(super) no_room: &'static str,
}

pub(super) const LIGHTS: InstanceNames = InstanceNames {
    item: "Light {number}",
    add: "Add light",
    remove: "Remove light",
    no_room: "No room for another light",
};

pub(super) const SOURCES: InstanceNames = InstanceNames {
    item: "Source {number}",
    add: "Add source",
    remove: "Remove source",
    no_room: "No room for another source",
};

/// Which instance the card shows, and whether its effect list has a place
/// for one more ([`crate::pipeline::effect_places`]).
pub(crate) struct InstanceSelection<'a> {
    pub(crate) selected: &'a mut usize,
    pub(crate) room: bool,
}

/// The instance row. A chip selects its instance; Add inserts `added` of the
/// selected instance after it and selects the copy; Remove deletes the
/// selected instance, unless it is the last. Returns whether the instances
/// changed.
pub(super) fn instance_row<T: Copy + Default>(
    ui: &mut Ui,
    instances: &mut EffectInstances<T>,
    selection: &mut InstanceSelection<'_>,
    names: &InstanceNames,
    color: impl Fn(&T) -> [f32; 3],
    added: impl Fn(&T) -> T,
) -> bool {
    let selected = &mut *selection.selected;
    *selected = (*selected).min(instances.count() - 1);
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for (index, instance) in instances.as_slice().iter().enumerate() {
            let number = index + 1;
            let name = tr_fmt(names.item, &[("number", &number)]);
            let response = moduwu_design::chip(
                ui,
                chip_label(ui, color(instance), index),
                index == *selected,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    ui.is_enabled(),
                    index == *selected,
                    &name,
                )
            });
            if response.on_hover_text(&name).clicked() {
                *selected = index;
            }
        }
        let size = egui::vec2(26.0, 26.0);
        let can_add = selection.room && !instances.is_full();
        let add = moduwu_design::icon_button_enabled(
            ui,
            can_add,
            egui_phosphor::regular::PLUS,
            size,
            tr(names.add),
        )
        .on_disabled_hover_text(tr(names.no_room));
        if add.clicked() {
            if let Some(copy) = instances.get(*selected).map(&added) {
                if instances.insert(*selected + 1, copy) {
                    *selected += 1;
                    changed = true;
                }
            }
        }
        let remove = moduwu_design::icon_button_enabled(
            ui,
            instances.count() > 1,
            egui_phosphor::regular::MINUS,
            size,
            tr(names.remove),
        );
        if remove.clicked() && instances.remove(*selected) {
            *selected = (*selected).min(instances.count() - 1);
            changed = true;
        }
    });
    changed
}

/// A dot in the instance's colour, then its number.
fn chip_label(ui: &Ui, color: [f32; 3], index: usize) -> egui::text::LayoutJob {
    let [r, g, b] = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        egui_phosphor::regular::CIRCLE,
        0.0,
        egui::TextFormat::simple(font.clone(), egui::Color32::from_rgb(r, g, b)),
    );
    job.append(
        &(index + 1).to_string(),
        4.0,
        egui::TextFormat::simple(font, ui.visuals().text_color()),
    );
    job
}
