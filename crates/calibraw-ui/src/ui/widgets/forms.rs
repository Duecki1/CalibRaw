use crate::ui::layout::ResponsiveWidth;
use eframe::egui::{self, Align, InnerResponse, Layout, Margin, Response, RichText, Ui};

use super::super::theme::{CONTROL_HEIGHT, HELP_BUTTON_EDGE, SPACE_SM, SPACE_XS};

pub(crate) fn heading_with_help(ui: &mut Ui, title: impl Into<RichText>, help: &str) {
    ui.heading(title).on_hover_text(help);
}

pub(crate) fn strong_with_help(ui: &mut Ui, title: impl Into<RichText>, help: &str) {
    ui.label(title.into().strong()).on_hover_text(help);
}

pub(crate) fn checkbox_with_help(
    ui: &mut Ui,
    checked: &mut bool,
    label: impl Into<egui::WidgetText>,
    help: &str,
) -> Response {
    let width = ui.available_width().max(1.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, CONTROL_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| ui.checkbox(checked, label).on_hover_text(help),
    )
    .inner
}

pub(crate) fn property_row<R>(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    add_control: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let width = ui.available_width().max(1.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, CONTROL_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(label);
            ui.with_layout(Layout::right_to_left(Align::Center), add_control)
                .inner
        },
    )
}

pub(crate) fn form_row<R>(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    preferred_control_width: f32,
    add_control: impl FnOnce(&mut Ui, f32) -> R,
) -> InnerResponse<R> {
    if ResponsiveWidth::from_width(ui.available_width()).is_compact() {
        ui.vertical(|ui| {
            ui.label(label);
            let width = ui.available_width().max(1.0);
            add_control(ui, width)
        })
    } else {
        property_row(ui, label, |ui| {
            let width = preferred_control_width.min(ui.available_width().max(1.0));
            add_control(ui, width)
        })
    }
}

pub(crate) fn form_row_with_help<R>(
    ui: &mut Ui,
    label: &str,
    preferred_control_width: f32,
    help: &str,
    add_control: impl FnOnce(&mut Ui, f32) -> R,
) -> InnerResponse<R> {
    if ResponsiveWidth::from_width(ui.available_width()).is_compact() {
        ui.vertical(|ui| {
            let width = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(width, HELP_BUTTON_EDGE),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(label).on_hover_text(help);
                },
            );
            let width = ui.available_width().max(1.0);
            add_control(ui, width)
        })
    } else {
        let width = ui.available_width().max(1.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, CONTROL_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(label).on_hover_text(help);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let width = preferred_control_width.min(ui.available_width().max(1.0));
                    add_control(ui, width)
                })
                .inner
            },
        )
    }
}

pub(crate) fn singleline_text_edit<'a>(text: &'a mut dyn egui::TextBuffer) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .vertical_align(Align::Center)
        .margin(Margin::symmetric(SPACE_SM as i8, SPACE_XS as i8))
        .min_size(egui::vec2(0.0, CONTROL_HEIGHT))
}

pub(crate) fn form_combo(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    id_salt: impl egui::AsIdSalt,
    selected_text: impl Into<egui::WidgetText>,
    preferred_width: f32,
    add_contents: impl FnOnce(&mut Ui),
) {
    form_row(ui, label, preferred_width, |ui, width| {
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(selected_text)
            .width(width)
            .truncate()
            .show_ui(ui, add_contents);
    });
}

pub(crate) fn responsive_combo_box<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    selected_text: impl Into<egui::WidgetText>,
    width: f32,
    item_count: usize,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let popup_style = ui.ctx().global_style();
    let spacing = &popup_style.spacing;
    let item_count_f32 = item_count as f32;
    let item_spacing_count = item_count.saturating_sub(1) as f32;
    let popup_height = item_count_f32 * spacing.interact_size.y
        + item_spacing_count * spacing.item_spacing.y
        + spacing.menu_margin.sum().y
        + 2.0 * ui.visuals().window_stroke.width
        + 4.0;
    let content_height = ui.ctx().content_rect().height();
    let popup_fits_viewport = content_height >= popup_height;

    let context = ui.ctx().clone();
    let theme = context.theme();
    let original_style = context.style_of(theme);
    if original_style.spacing.default_area_size.y < popup_height {
        context.style_mut_of(theme, |style| {
            style.spacing.default_area_size.y = popup_height;
        });
    }

    let response = egui::ComboBox::from_id_salt((id_salt, popup_fits_viewport))
        .selected_text(selected_text)
        .width(width)
        .height(content_height)
        .truncate()
        .show_ui(ui, add_contents);

    context.set_style_of(theme, original_style);
    response
}

pub(crate) fn form_combo_with_help(
    ui: &mut Ui,
    label: &str,
    id_salt: impl egui::AsIdSalt,
    selected_text: impl Into<egui::WidgetText>,
    preferred_width: f32,
    help: &str,
    add_contents: impl FnOnce(&mut Ui),
) {
    form_row_with_help(ui, label, preferred_width, help, |ui, width| {
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(selected_text)
            .width(width)
            .truncate()
            .show_ui(ui, add_contents)
            .response
            .on_hover_text(help);
    });
}
