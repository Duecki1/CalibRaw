//! The picker for the settings an edit transfer moves: one chip per Edit card
//! and one per additional category. Shared by the preset editor and the
//! "Choose adjustments to copy" dialog.
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::{AdjustmentGroup, AdjustmentGroupSet};
use crate::sidecar::{EditSelection, EditState};
use eframe::egui::{self, Ui};

/// What the chosen settings are for. Presets leave out photo-specific
/// settings that a copy keeps, so some choices and their help differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionPurpose {
    /// Saved in a new preset.
    Preset,
    /// Copied for Paste adjustments.
    Copy,
}

/// The selection chips for `selection`, with the groups `edits` changes shown
/// in bold, and the number of settings chosen.
pub(crate) fn show_selection_controls(
    ui: &mut Ui,
    selection: &mut EditSelection,
    edits: &EditState,
    purpose: SelectionPurpose,
) {
    moduwu_design::strong_with_help(
        ui,
        tr("Include settings"),
        match purpose {
            SelectionPurpose::Preset => tr("Settings that are not selected keep the destination photo’s values. Selecting an unchanged group also saves its default values."),
            SelectionPurpose::Copy => tr("Settings that are not selected keep the destination photo’s values."),
        },
    );
    ui.add_space(moduwu_design::SPACE_XS);
    // Each column retains a readable checkbox width; small viewports stack the
    // same cards inside the dialog's bounded scroll area.
    if ui.available_width() >= 440.0 {
        ui.columns(2, |columns| {
            show_adjustment_selection(&mut columns[0], selection, edits);
            show_additional_selection(&mut columns[1], selection, purpose);
        });
    } else {
        show_adjustment_selection(ui, selection, edits);
        moduwu_design::card_gap(ui);
        show_additional_selection(ui, selection, purpose);
    }

    let selected_count = selection.adjustment_groups.iter().count()
        + [
            selection.raw_processing,
            selection.camera_profile,
            selection.lut,
            selection.masks,
            selection.ai_masks,
            selection.geometry,
            selection.lens_correction,
        ]
        .into_iter()
        .filter(|included| *included)
        .count();
    ui.add_space(moduwu_design::SPACE_XS);
    ui.add(
        egui::Label::new(
            egui::RichText::new(if selected_count == 0 {
                match purpose {
                    SelectionPurpose::Preset => {
                        tr("Choose at least one setting to save this preset.")
                    }
                    SelectionPurpose::Copy => tr("Choose at least one setting to copy."),
                }
                .to_owned()
            } else if selected_count == 1 {
                tr_fmt("{count} setting selected", &[("count", &selected_count)])
            } else {
                tr_fmt("{count} settings selected", &[("count", &selected_count)])
            })
            .small()
            .weak(),
        )
        .wrap(),
    );

    let has_photo_specific_masks = edits
        .masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(|component| !crate::presets::is_portable_mask_kind(component.kind));
    if purpose == SelectionPurpose::Preset
        && has_photo_specific_masks
        && (selection.masks || selection.ai_masks)
    {
        ui.add(egui::Label::new(
            egui::RichText::new(
                tr("Brush, path and object masks follow this photo's content and are not saved in presets."),
            )
            .small()
            .weak(),
        ).wrap());
    }
}

fn show_adjustment_selection(ui: &mut Ui, selection: &mut EditSelection, edits: &EditState) {
    moduwu_design::content_card(ui, |ui| {
        moduwu_design::strong_with_help(
            ui,
            tr("Adjustments"),
            tr("Each group matches a card in the Edit tab. Groups that are not selected keep the destination photo's settings. Edited groups are shown in bold."),
        );
        moduwu_design::action_row(ui, |ui| {
            let all = selection.adjustment_groups == AdjustmentGroupSet::ALL;
            if moduwu_design::chip(ui, tr("All"), all)
                .on_hover_text(tr("Include every adjustment group, including unchanged values. Click again to clear them."))
                .clicked()
            {
                selection.adjustment_groups = if all {
                    AdjustmentGroupSet::EMPTY
                } else {
                    AdjustmentGroupSet::ALL
                };
            }
            for group in AdjustmentGroup::ALL {
                let included = selection.adjustment_groups.contains(group);
                let edited = crate::sidecar::adjustment_group_is_edited(edits, group);
                let label = egui::RichText::new(group.label());
                let label = if edited { label.strong() } else { label };
                let help = if edited {
                    tr("Edited on this photo. Include this group’s current settings.")
                } else {
                    tr("Unchanged on this photo. Including this group replaces the destination’s settings with these defaults.")
                };
                if moduwu_design::chip(ui, label, included)
                    .on_hover_text(help)
                    .clicked()
                {
                    selection.adjustment_groups.set(group, !included);
                }
            }
        });
    });
}

fn show_additional_selection(
    ui: &mut Ui,
    selection: &mut EditSelection,
    purpose: SelectionPurpose,
) {
    let (masks_help, ai_masks_help) = match purpose {
        SelectionPurpose::Preset => (
            tr("Fullscreen, linear and radial masks with their adjustments. They are added next to each photo's own masks."),
            tr("Subject, background, sky, depth and range masks. They are regenerated for each photo the preset is applied to."),
        ),
        SelectionPurpose::Copy => (
            tr("Brush, linear, radial, and fullscreen mask components with their local edits."),
            tr("Subject, background, sky, object, luminance-range, and color-range components. They are regenerated for the destination image when needed."),
        ),
    };
    moduwu_design::content_card(ui, |ui| {
        ui.strong(tr("Also include"));
        ui.add_space(moduwu_design::SPACE_XS);
        moduwu_design::action_row(ui, |ui| {
            // Presets leave RAW processing out as photo-specific
            // (`presets::suggested_selection`); a copy shares it, for example
            // across a roll of film scans.
            if purpose == SelectionPurpose::Copy
                && moduwu_design::chip(ui, tr("RAW processing"), selection.raw_processing)
                    .on_hover_text(tr("Black point, tone mapper, demosaic, highlight reconstruction and the film negative."))
                    .clicked()
            {
                selection.raw_processing = !selection.raw_processing;
            }
            for (included, label, help) in [
                (
                    &mut selection.camera_profile,
                    tr("Camera profile"),
                    tr("The DCP profile chosen for this photo. Photos from other camera models keep their automatic profile."),
                ),
                (
                    &mut selection.lut,
                    tr("LUT"),
                    tr("The LUT and its amount. Photos whose LUT library lacks it are shown without it."),
                ),
                (&mut selection.masks, tr("Masks"), masks_help),
                (&mut selection.ai_masks, tr("AI masks"), ai_masks_help),
                (
                    &mut selection.geometry,
                    tr("Crop & geometry"),
                    tr("Crop, rotation, straighten, perspective and flips."),
                ),
                (
                    &mut selection.lens_correction,
                    tr("Lens correction"),
                    tr("Lens correction state and the selected lens profile."),
                ),
            ] {
                if moduwu_design::chip(ui, label, *included)
                    .on_hover_text(help)
                    .clicked()
                {
                    *included = !*included;
                }
            }
        });
    });
}
