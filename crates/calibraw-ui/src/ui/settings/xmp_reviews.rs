//! The rows that choose how ratings use XMP files; folder storage only.

use super::Settings;
use crate::app::CalibRawApp;
use crate::i18n::tr;
use crate::sidecar::xmp_review::{XmpReviewSync, XmpSidecarName};
use eframe::egui::Ui;

const FIELD_WIDTH: f32 = 220.0;

impl Settings {
    pub(super) fn show_xmp_reviews(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::strong_with_help(
            ui,
            tr("Ratings in XMP files"),
            tr("Shares star ratings and pick and reject flags with apps such as Lightroom, darktable and digiKam through the XMP files beside your photos."),
        );
        let mut settings = app.preferences.xmp_review;
        moduwu_design::form_combo_with_help(
            ui,
            tr("Use XMP"),
            "settings-xmp-review-sync",
            sync_label(settings.sync),
            FIELD_WIDTH,
            sync_description(settings.sync),
            |ui| {
                for option in [
                    XmpReviewSync::Off,
                    XmpReviewSync::Read,
                    XmpReviewSync::ReadWrite,
                ] {
                    ui.selectable_value(&mut settings.sync, option, sync_label(option))
                        .on_hover_text(sync_description(option));
                }
            },
        );
        if settings.sync != XmpReviewSync::Off {
            moduwu_design::form_combo_with_help(
                ui,
                tr("XMP file name"),
                "settings-xmp-sidecar-name",
                name_example(settings.sidecar_name),
                FIELD_WIDTH,
                tr("The name looked for first and given to new XMP files. darktable and digiKam keep the photo's extension; Lightroom and Capture One drop it, so a RAW and a JPEG of the same name share one file."),
                |ui| {
                    for option in [XmpSidecarName::WithExtension, XmpSidecarName::Basename] {
                        ui.selectable_value(
                            &mut settings.sidecar_name,
                            option,
                            name_example(option),
                        );
                    }
                },
            );
        }
        app.set_xmp_review_settings(settings);
    }
}

fn sync_label(sync: XmpReviewSync) -> &'static str {
    match sync {
        XmpReviewSync::Off => tr("Off"),
        XmpReviewSync::Read => tr("Read"),
        XmpReviewSync::ReadWrite => tr("Read and write"),
    }
}

fn sync_description(sync: XmpReviewSync) -> &'static str {
    match sync {
        XmpReviewSync::Off => tr("Ratings are kept in CalibRaw's own sidecar files only."),
        XmpReviewSync::Read => tr("Photos without CalibRaw edits or ratings show the rating their XMP file records. Changes are saved only to CalibRaw's own files."),
        XmpReviewSync::ReadWrite => tr("The rating in a photo's XMP file wins, and every change is saved to it too. Everything else in the file is kept."),
    }
}

/// The name of the sidecar of an example photo, the same in every language.
fn name_example(name: XmpSidecarName) -> &'static str {
    match name {
        XmpSidecarName::WithExtension => "IMG_0001.CR3.xmp",
        XmpSidecarName::Basename => "IMG_0001.xmp",
    }
}
