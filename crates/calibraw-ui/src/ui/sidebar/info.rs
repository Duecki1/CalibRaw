use super::*;
use crate::pipeline::CfaKind;
use crate::ui::capture_format::{
    format_aperture, format_focal_length, format_iso, format_shutter_speed,
};

impl Sidebar {
    pub(super) fn show_info(ui: &mut Ui, app: &CalibRawApp) {
        let Some(raw) = app
            .develop
            .original_raw
            .as_deref()
            .or(app.develop.loaded_raw.as_deref())
        else {
            moduwu_design::content_card(ui, |ui| {
                ui.label("Open a photo to view its metadata.");
            });
            return;
        };

        let file_name = display_file_name(
            app.develop.current_label.as_deref(),
            app.develop.current_path.as_deref(),
        );

        moduwu_design::section_card(ui, "File", |ui| {
            metadata_row(ui, "Name", &file_name);
            metadata_row(
                ui,
                "Format",
                &source_format_description(&file_name, app.current_rendered_format(), raw),
            );
            metadata_row(
                ui,
                "Editing time",
                &crate::app::format_usage_duration(app.raw_edit_duration()),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
            // A media-library photo is open from a temporary copy.
            if let Some(path) = app
                .develop
                .current_path
                .as_deref()
                .filter(|_| app.caps().has_folders())
            {
                if let Some(parent) = path.parent() {
                    metadata_row(ui, "Location", &parent.display().to_string());
                }
                if let Some(bytes) = app.library.desktop_asset_bytes(path) {
                    metadata_row(ui, "File size", &format_file_size(bytes));
                }
            }
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Capture", |ui| {
            optional_metadata_row(ui, "ISO", format_iso(raw.capture_metadata.iso_speed));
            optional_metadata_row(
                ui,
                "Shutter speed",
                format_shutter_speed(raw.capture_metadata.shutter_seconds),
            );
            optional_metadata_row(ui, "Aperture", format_aperture(raw.aperture));
            optional_metadata_row(ui, "Focal length", format_focal_length(raw.focal_length));
            optional_metadata_row(
                ui,
                "Focus distance",
                format_focus_distance(raw.focus_distance),
            );
            optional_metadata_row(ui, "Flash", raw.capture_metadata.flash.map(format_flash));
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Equipment", |ui| {
            optional_metadata_row(
                ui,
                "Camera",
                equipment_name(&raw.camera_make, &raw.camera_model),
            );
            optional_metadata_row(ui, "Lens", equipment_name(&raw.lens_make, &raw.lens_model));
        });

        moduwu_design::card_gap(ui);
        moduwu_design::section_card(ui, "Image", |ui| {
            metadata_row(
                ui,
                "Dimensions",
                &format!("{} × {} px", raw.width, raw.height),
            );
            let megapixels = f64::from(raw.width) * f64::from(raw.height) / 1_000_000.0;
            metadata_row(ui, "Resolution", &format!("{megapixels:.1} MP"));
            if !raw.is_display_referred_raster() {
                metadata_row(
                    ui,
                    "Sensor pattern",
                    match raw.cfa_kind {
                        // A linear DNG was demosaiced before it was saved.
                        _ if raw.is_camera_linear_raster() => "Demosaiced in camera",
                        CfaKind::Bayer => "Bayer",
                        CfaKind::XTrans => "X-Trans",
                    },
                );
            }

            let cropped = app
                .develop
                .geometry
                .crop_pixel_dimensions(raw.width, raw.height);
            if cropped != (raw.width, raw.height) {
                metadata_row(
                    ui,
                    "Cropped size",
                    &format!("{} × {} px", cropped.0, cropped.1),
                );
            }
        });

        if !raw.capture_metadata.artist.trim().is_empty()
            || !raw.capture_metadata.description.trim().is_empty()
        {
            moduwu_design::card_gap(ui);
            moduwu_design::section_card(ui, "Description", |ui| {
                if !raw.capture_metadata.artist.trim().is_empty() {
                    metadata_row(ui, "Creator", raw.capture_metadata.artist.trim());
                }
                if !raw.capture_metadata.description.trim().is_empty() {
                    metadata_row(ui, "Caption", raw.capture_metadata.description.trim());
                }
            });
        }
    }
}

fn metadata_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(92.0, 18.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_width(92.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(label)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    )
                    .wrap(),
                );
            },
        );
        ui.add(egui::Label::new(value).wrap());
    });
}

fn optional_metadata_row(ui: &mut Ui, label: &str, value: Option<String>) {
    if let Some(value) = value {
        metadata_row(ui, label, &value);
    } else {
        metadata_row(ui, label, "Not recorded");
    }
}

fn finite_positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

/// "JPEG · rendered photo", "TIFF · rendered image", "DNG · linear RAW" (a DNG
/// the camera demosaiced before saving, as phones do) or "NEF · camera RAW".
fn source_format_description(
    file_name: &str,
    rendered: Option<crate::pipeline::RenderedImageFormat>,
    raw: &crate::pipeline::LoadedRaw,
) -> String {
    if let Some(format) = rendered {
        return format!("{} · rendered photo", format.label());
    }
    let extension = std::path::Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_uppercase)
        .unwrap_or_else(|| "RAW".to_owned());
    if raw.is_display_referred_raster() {
        format!("{extension} · rendered image")
    } else if raw.is_camera_linear_raster() {
        format!("{extension} · linear RAW")
    } else {
        format!("{extension} · camera RAW")
    }
}

fn display_file_name(label: Option<&str>, path: Option<&std::path::Path>) -> String {
    label
        .filter(|label| !label.trim().is_empty())
        .and_then(|label| std::path::Path::new(label).file_name())
        .or_else(|| path.and_then(std::path::Path::file_name))
        .and_then(std::ffi::OsStr::to_str)
        .or_else(|| label.filter(|label| !label.trim().is_empty()))
        .unwrap_or("Not available")
        .to_owned()
}

fn format_flash(value: u16) -> String {
    let mut details = Vec::with_capacity(4);
    if value & 0x20 != 0 {
        details.push("No flash function");
    } else if value & 0x01 != 0 {
        details.push("Fired");
    } else {
        details.push("Did not fire");
    }

    match value & 0x18 {
        0x08 => details.push("compulsory mode"),
        0x10 => details.push("suppressed"),
        0x18 => details.push("auto mode"),
        _ => {}
    }
    match value & 0x06 {
        0x02 => details.push("return not detected"),
        0x06 => details.push("return detected"),
        _ => {}
    }
    if value & 0x40 != 0 {
        details.push("red-eye reduction");
    }
    details.join(", ")
}

fn format_focus_distance(value: f32) -> Option<String> {
    finite_positive(value).map(|value| format!("{value:.2} m"))
}

fn equipment_name(make: &str, model: &str) -> Option<String> {
    let make = make.trim();
    let model = model.trim();
    match (make.is_empty(), model.is_empty()) {
        (true, true) => None,
        (false, true) => Some(make.to_owned()),
        (true, false) => Some(model.to_owned()),
        (false, false)
            if model
                .to_ascii_lowercase()
                .starts_with(&make.to_ascii_lowercase()) =>
        {
            Some(model.to_owned())
        }
        (false, false) => Some(format!("{make} {model}")),
    }
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
