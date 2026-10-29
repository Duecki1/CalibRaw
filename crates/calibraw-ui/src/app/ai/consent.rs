use super::*;
use crate::i18n::{tr, tr_fmt};
use calibraw_ai::AiFeature;

const ARTIFACTS_URL: &str = "https://huggingface.co/Duecki/CalibRaw-Artifacts/tree/main/models";

/// What the consent dialog tells the user about a job's model.
struct AiJobDescription {
    /// Completes "Download … model?" and "Prepare …?".
    label: &'static str,
    /// Model name and download size.
    model: String,
    /// Completes "CalibRaw will … locally."
    purpose: &'static str,
    details_id: &'static str,
    artifact: (&'static str, String),
    details: String,
    /// A license with terms beyond attribution, which the dialog names
    /// outside the collapsed details: its name and where it is published.
    terms: Option<(&'static str, &'static str)>,
    /// Wording for re-running the model behind an edit that is already on.
    restore: Option<AiRestorePrompt>,
}

struct AiRestorePrompt {
    title: &'static str,
    explanation: &'static str,
    accept: &'static str,
    decline: &'static str,
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

impl CalibRawApp {
    fn describe_ai_job(&self, feature: AiFeature) -> AiJobDescription {
        match feature {
            AiFeature::Subject => {
                let checkpoint = self.subject_model();
                AiJobDescription {
                    label: tr("subject selection"),
                    model: tr_fmt("BiRefNet (~{size} MB)", &[("size", &format!("{:.0}", megabytes(checkpoint.download_bytes)))]),
                    purpose: tr("create subject and background masks"),
                    details_id: "subject-download-details",
                    artifact: (
                        tr("Subject model artifacts"),
                        format!("{ARTIFACTS_URL}/briefnet"),
                    ),
                    details: tr_fmt(
                        "{name} runs locally with a {width} × {height} input. License: MIT.",
                        &[
                            ("name", &checkpoint.name),
                            ("width", &checkpoint.input.edge()),
                            ("height", &checkpoint.input.edge()),
                        ],
                    ),
                    terms: None,
                    restore: None,
                }
            }
            AiFeature::Sky => AiJobDescription {
                label: tr("sky selection"),
                model: tr("SkySeg U2Net (~176 MB)").to_owned(),
                purpose: tr("create sky masks"),
                details_id: "sky-download-details",
                artifact: (tr("Sky model artifact"), format!("{ARTIFACTS_URL}/skyseg")),
                details: "SkySeg U2Net runs locally on a 320 × 320 image and produces a sky \
                          probability mask. License: MIT."
                    .to_owned(),
                terms: None,
                restore: None,
            },
            AiFeature::SceneDepth => {
                let depth_model = calibraw_ai::models::depth::MODEL;
                AiJobDescription {
                    label: tr("scene depth"),
                    model: tr_fmt(
                        "{name} (~{size} MB)",
                        &[
                            ("name", &depth_model.name),
                            ("size", &format!("{:.0}", megabytes(depth_model.download_bytes))),
                        ],
                    ),
                    purpose: tr("estimate scene depth for depth masks and effects"),
                    details_id: "depth-download-details",
                    artifact: (
                        tr("Depth model artifact"),
                        calibraw_ai::models::depth::ARTIFACT_PAGE_URL.to_owned(),
                    ),
                    details: format!(
                        "{name} runs locally on a {edge} × {edge} letterboxed image and \
                         produces relative depth. License: Apache-2.0.",
                        name = depth_model.name,
                        edge = depth_model.input.edge(),
                    ),
                    terms: None,
                    restore: None,
                }
            }
            AiFeature::Object => AiJobDescription {
                label: tr("object selection"),
                model: tr_fmt("SAM 2.1 (~{size} MB)", &[("size", &format!("{:.0}", megabytes(SAM21_MODEL_BYTES_ESTIMATE)))]),
                purpose: tr("create object masks"),
                details_id: "object-download-details",
                artifact: (tr("Object model artifacts"), format!("{ARTIFACTS_URL}/sam2")),
                details: "SAM 2.1 Hiera Tiny uses an encoder and decoder with local \
                          edge-aware cleanup. License: Apache-2.0."
                    .to_owned(),
                terms: None,
                restore: None,
            },
            AiFeature::People => {
                use calibraw_ai::models::people::{
                    foreground_model, PeopleModels, BODY_PARTS_LICENSE, BODY_PARTS_MODEL,
                    DETECTOR_MODEL,
                };
                let detector = tr_fmt(
                    "{name} finds the people in the photo (License: Apache-2.0).",
                    &[("name", &DETECTOR_MODEL.name)],
                );
                let (models, bytes, details, terms) = match self.people_models() {
                    PeopleModels::Outline { foreground, .. } => {
                        let foreground = foreground_model(foreground.precision);
                        (
                            tr("RF-DETR-Seg and BiRefNet"),
                            DETECTOR_MODEL.download_bytes + foreground.download_bytes,
                            tr_fmt(
                                "{detector} {model} then outlines each person you choose, on a \
                                 crop around them (License: MIT).",
                                &[("detector", &detector), ("model", &foreground.name)],
                            ),
                            None,
                        )
                    }
                    PeopleModels::BodyParts { foreground, .. } => {
                        let foreground = foreground.map(|model| foreground_model(model.precision));
                        let (models, foreground_details) = match foreground {
                            Some(model) => (
                                tr("RF-DETR-Seg, Sapiens2 and BiRefNet"),
                                tr_fmt(
                                    " For high quality, {model} first outlines each person more \
                                     finely (License: MIT).",
                                    &[("model", &model.name)],
                                ),
                            ),
                            None => (tr("RF-DETR-Seg and Sapiens2"), String::new()),
                        };
                        (
                            models,
                            DETECTOR_MODEL.download_bytes
                                + BODY_PARTS_MODEL.download_bytes
                                + foreground.map_or(0, |model| model.download_bytes),
                            tr_fmt(
                                "{detector} {model} then segments the body parts of each person \
                                 you choose, on a crop around them (License: Sapiens2 \
                                 License).{foreground_details}",
                                &[
                                    ("detector", &detector),
                                    ("model", &BODY_PARTS_MODEL.name),
                                    ("foreground_details", &foreground_details),
                                ],
                            ),
                            Some((tr("Sapiens2 License"), BODY_PARTS_LICENSE)),
                        )
                    }
                };
                AiJobDescription {
                    label: tr("people selection"),
                    model: tr_fmt("{models} (~{size} MB)", &[("models", &models), ("size", &format!("{:.0}", megabytes(bytes)))]),
                    purpose: tr("create people masks"),
                    details_id: "people-download-details",
                    artifact: (tr("People model artifacts"), ARTIFACTS_URL.to_owned()),
                    details,
                    terms,
                    restore: None,
                }
            }
            AiFeature::Remove => AiJobDescription {
                label: tr("Remove"),
                model: tr_fmt(
                    "Big-LaMa (~{size} MB)",
                    &[(
                        "size",
                        &format!(
                            "{:.0}",
                            megabytes(calibraw_ai::models::remove::MODEL.download_bytes)
                        ),
                    )],
                ),
                purpose: tr("remove unwanted content"),
                details_id: "remove-download-details",
                artifact: (tr("Big-LaMa model artifact"), format!("{ARTIFACTS_URL}/lama")),
                details: tr_fmt("Big-LaMa Places2 ONNX repairs a local context crop. License: {model_license}. \
                     Source: {model_provenance}. CalibRaw verifies its pinned size and SHA-256 ({sha}).", &[("model_license", &calibraw_ai::models::remove::MODEL_LICENSE), ("model_provenance", &calibraw_ai::models::remove::MODEL_PROVENANCE), ("sha", &&calibraw_ai::models::remove::MODEL.sha256[..12])]),
                terms: None,
                restore: None,
            },
            AiFeature::Denoise => AiJobDescription {
                label: tr("AI denoise"),
                model: tr_fmt(
                    "RawNIND (~{size} MB)",
                    &[(
                        "size",
                        &format!("{:.1}", megabytes(calibraw_ai::models::denoise::PACKAGE_BYTES)),
                    )],
                ),
                purpose: tr("apply AI denoise"),
                details_id: "denoise-download-details",
                artifact: (tr("RawNIND model artifact"), format!("{ARTIFACTS_URL}/rawnind")),
                details: "RawNIND handles Bayer denoise/demosaic and X-Trans images. The \
                          verified models are cached locally under GPL-3.0."
                    .to_owned(),
                terms: None,
                restore: Some(AiRestorePrompt {
                    title: tr("Re-apply AI denoise?"),
                    explanation: "AI denoise is turned on for this image, but there is no saved \
                                  AI-denoise result for it. Applying it runs RawNIND on this \
                                  device again, which can take a few minutes.",
                    accept: tr("Re-apply"),
                    decline: tr("Turn off AI denoise"),
                }),
            },
        }
    }

    /// The one consent dialog for every local-AI job.
    pub(in crate::app) fn show_ai_consent_dialog(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
    ) {
        let Some(AiConsent {
            feature,
            runtime_download_needed,
            origin,
        }) = self.ai.consent
        else {
            return;
        };
        let model_download_needed = self.model_download_needed(feature);
        let description = self.describe_ai_job(feature);
        let download_needed = model_download_needed || runtime_download_needed;
        let restore = description
            .restore
            .as_ref()
            .filter(|_| origin == AiJobOrigin::Restore);
        let title = match (restore, model_download_needed, runtime_download_needed) {
            (Some(restore), ..) => restore.title.to_owned(),
            (None, true, true) => tr_fmt(
                "Download {label} model and ONNX Runtime?",
                &[("label", &description.label)],
            ),
            (None, true, false) => {
                tr_fmt("Download {label} model?", &[("label", &description.label)])
            }
            (None, false, true) => tr("Download ONNX Runtime?").to_owned(),
            (None, false, false) => tr_fmt("Prepare {label}?", &[("label", &description.label)]),
        };
        let runtime_ready = self.ai_runtime_ready();
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("ai-consent", title)
            .size(moduwu_design::DialogSize::Wide)
            .show_with_footer(
                ctx,
                |ui| {
                    if let Some(restore) = restore {
                        ui.label(restore.explanation);
                    }
                    if restore.is_none() || download_needed {
                        Self::show_ai_download_summary(
                            ui,
                            &description.model,
                            description.purpose,
                            model_download_needed,
                            runtime_download_needed,
                        );
                    }
                    if let Some((license, url)) =
                        description.terms.filter(|_| model_download_needed)
                    {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(tr("Downloading accepts the model's"));
                            ui.hyperlink_to(license, url);
                            ui.label(tr("which prohibits some uses, such as surveillance."));
                        });
                    }
                    self.show_ai_download_details(
                        ui,
                        description.details_id,
                        model_download_needed,
                        runtime_download_needed,
                        &[(description.artifact.0, description.artifact.1.as_str())],
                        |ui| {
                            ui.label(&description.details);
                        },
                    );
                    self.show_manual_runtime_warning(ui);
                },
                |ui| {
                    let accept = match restore {
                        _ if download_needed => tr("Accept & download"),
                        Some(restore) => restore.accept,
                        None => tr("Continue"),
                    };
                    let decline = restore.map_or(tr("Cancel"), |restore| restore.decline);
                    action = Self::show_ai_consent_buttons(ui, decline, accept, runtime_ready);
                },
            );
        match action {
            moduwu_design::DialogAction::Confirm => {
                self.ai.consent = None;
                self.start_ai_job(feature, model_download_needed, frame);
            }
            moduwu_design::DialogAction::Cancel => self.abandon_ai_job(feature),
            moduwu_design::DialogAction::None => {}
        }
    }
}
