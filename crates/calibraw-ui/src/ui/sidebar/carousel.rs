//! The Export panel's carousel card: the switch, the slide count and shape, and the stencil's
//! size. The stencil is dragged into place on the preview (`ui::preview::tools::carousel`).

use crate::app::{CarouselAction, CarouselView};
use crate::i18n::tr;
use crate::pipeline::{Carousel, CarouselRatio};
use crate::ui::components::adjustment_slider::AdjustmentSlider;
use eframe::egui::{self, Ui};

/// The values of [`Carousel::SLIDES`].
const SLIDE_COUNTS: [(u8, &str); 5] = [(2, "2"), (3, "3"), (4, "4"), (5, "5"), (6, "6")];
const SLIDE_COUNT_WIDTH: f32 = 220.0;
/// Ratios are numbers and stay as they are.
const PRESET_RATIOS: [(CarouselRatio, &str); 3] = [
    (CarouselRatio::FourFive, "4:5"),
    (CarouselRatio::ThreeFour, "3:4"),
    (CarouselRatio::Square, "1:1"),
];

pub(super) fn carousel_card(ui: &mut Ui, view: CarouselView) -> Option<CarouselAction> {
    let mut action = None;
    moduwu_design::section_card_with_help(
        ui,
        tr("Carousel"),
        tr("Drag the frame on the photo to place it."),
        |ui| {
            let mut enabled = view.enabled;
            if moduwu_design::toggle_row(ui, &mut enabled, tr("Split into slides")).changed() {
                action = Some(CarouselAction::SetEnabled(enabled));
            }
            if !view.enabled {
                return;
            }

            let mut slides = view.carousel.slides();
            moduwu_design::form_row(ui, tr("Slides"), SLIDE_COUNT_WIDTH, |ui, width| {
                moduwu_design::segmented_control(ui, &mut slides, &SLIDE_COUNTS, width);
            });
            let ratio = ratio_picker(ui, view.carousel.ratio());
            let mut size = view.carousel.size();
            // A slider's id comes from its label, which the watermark's size slider shares.
            ui.push_id("carousel-size", |ui| {
                AdjustmentSlider::new(tr("Size (%)"), &mut size, Carousel::SIZE_RANGE)
                    .decimals(0)
                    .step(1.0)
                    .reset_to(Carousel::LARGEST_SIZE)
                    .show(ui);
            });
            let edited = view
                .carousel
                .with_slides(slides)
                .with_ratio(ratio)
                .with_size(size);
            if edited != view.carousel {
                action = Some(CarouselAction::Configure(edited));
            }

            let status = match view.layout {
                Some(layout) => {
                    let (width, height) = layout.slide_size();
                    format!("{} × {width} × {height} px", layout.slides())
                }
                None => tr("These slides cannot be exported at this size.").to_owned(),
            };
            ui.label(
                egui::RichText::new(status)
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        },
    );
    action
}

/// Ratio chips and the terms of a custom ratio.
fn ratio_picker(ui: &mut Ui, current: CarouselRatio) -> CarouselRatio {
    let mut ratio = current;
    let custom = matches!(current, CarouselRatio::Custom { .. });
    moduwu_design::action_row(ui, |ui| {
        for (preset, label) in PRESET_RATIOS {
            if moduwu_design::chip(ui, label, current == preset).clicked() {
                ratio = preset;
            }
        }
        if moduwu_design::chip(ui, tr("Custom"), custom).clicked() && !custom {
            let (width, height) = current.terms();
            ratio = CarouselRatio::Custom { width, height };
        }
    });
    if let CarouselRatio::Custom {
        mut width,
        mut height,
    } = ratio
    {
        let terms = 1..=CarouselRatio::MAX_TERM;
        ui.horizontal(|ui| {
            ui.add(
                moduwu_design::NumberField::new(&mut width, terms.clone())
                    .commit_on_finish(true)
                    .id_salt("carousel-custom-ratio-width"),
            );
            ui.label("×");
            ui.add(
                moduwu_design::NumberField::new(&mut height, terms)
                    .commit_on_finish(true)
                    .id_salt("carousel-custom-ratio-height"),
            );
        });
        ratio = CarouselRatio::Custom { width, height };
    }
    ratio
}
