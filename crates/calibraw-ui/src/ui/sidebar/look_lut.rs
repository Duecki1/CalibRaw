//! The Develop card that chooses the photo's LUT and its amount.

use super::adjustment_cards::CardAction;
use super::*;
use crate::app::LOOK_LUT_CARD_TITLE;
use crate::luts::{LookLut, LutEntry, LutId};
use crate::ui::components::preview_list::{preview_list, ListHeight, PreviewListItem};

/// Libraries with more LUTs than this get a search field and a Recent group.
const SEARCH_FROM_LUTS: usize = 8;
/// Rows the list shows before it scrolls in the docked panel.
const VISIBLE_LUT_ROWS: f32 = 7.5;
/// Rows the list keeps in the portrait panel, where it fills the panel.
const MIN_PORTRAIT_LUT_ROWS: f32 = 3.0;

/// What the LUT card asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LookLutAction {
    Select(Option<LutId>),
    /// 0–1.
    Amount(f32),
    Import,
}

pub(super) struct LookLutResponse {
    pub(super) action: Option<LookLutAction>,
    /// The row a hovering pointer rests on; `Some(None)` is the None row.
    pub(super) hovered: Option<Option<LutId>>,
}

/// What the LUT card shows.
pub(super) struct LookLutView<'a> {
    pub(super) look: Option<&'a LookLut>,
    /// Every library LUT, sorted by name.
    pub(super) entries: &'a [LutEntry],
    /// Recently chosen library LUTs, most recent first.
    pub(super) recent: &'a [LutId],
    pub(super) can_import: bool,
    /// Docked, the card folds and its list is a few rows high. In portrait
    /// it shows alone, and its list fills the panel so the card fits whole.
    pub(super) layout: ScreenLayout,
}

impl Sidebar {
    /// The LUT card: None, the library's LUTs, then Import… and the amount of
    /// the chosen LUT. A LUT the library does not hold is listed as missing.
    pub(super) fn show_look_lut(ui: &mut Ui, view: &LookLutView<'_>) -> LookLutResponse {
        let mut response = LookLutResponse {
            action: None,
            hovered: None,
        };
        let foldable = view.layout == ScreenLayout::Horizontal;
        let card = Self::adjustment_card(ui, LOOK_LUT_CARD_TITLE, false, foldable, true, |ui| {
            let query = Self::show_look_lut_search(ui, view.entries.len());
            let items = look_lut_items(view, &query);
            let list_id = "develop-look-lut-list";
            let list = match view.layout {
                ScreenLayout::Horizontal => {
                    preview_list(ui, list_id, ListHeight::Rows(VISIBLE_LUT_ROWS), items)
                }
                ScreenLayout::Vertical => {
                    let min_height = (moduwu_design::CONTROL_HEIGHT + ui.spacing().item_spacing.y)
                        * MIN_PORTRAIT_LUT_ROWS;
                    Self::portrait_fill(ui, list_id, min_height, |ui, height| {
                        preview_list(ui, list_id, ListHeight::Points(height), items)
                    })
                }
            };
            if let Some(choice) = list.clicked {
                response.action = Some(LookLutAction::Select(choice));
            }
            response.hovered = list.hovered;
            if view.can_import && moduwu_design::full_width_button(ui, "Import…").clicked() {
                response.action = Some(LookLutAction::Import);
            }
            if let Some(look) = view.look {
                let mut percent = look.amount * 100.0;
                if AdjustmentSlider::new("Amount", &mut percent, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .reset_to(LookLut::DEFAULT_AMOUNT * 100.0)
                    .show(ui)
                {
                    response.action = Some(LookLutAction::Amount(percent / 100.0));
                }
            }
        });
        if matches!(card, CardAction::Reset) {
            response.action = Some(LookLutAction::Select(None));
        }
        response
    }

    /// The search field of a large library, which keeps its text in the
    /// view's memory. Returns the lowercase query; empty without a field.
    fn show_look_lut_search(ui: &mut Ui, lut_count: usize) -> String {
        if lut_count <= SEARCH_FROM_LUTS {
            return String::new();
        }
        let id = ui.id().with("develop-look-lut-search");
        let mut query = ui.data_mut(|data| data.get_temp::<String>(id).unwrap_or_default());
        ui.add_sized(
            [ui.available_width(), moduwu_design::CONTROL_HEIGHT],
            moduwu_design::singleline_text_edit(&mut query).hint_text(format!(
                "{} Search LUTs…",
                egui_phosphor::regular::MAGNIFYING_GLASS
            )),
        );
        let lowercase = query.trim().to_lowercase();
        ui.data_mut(|data| data.insert_temp(id, query));
        lowercase
    }
}

/// The rows of the LUT list for the lowercase search `query`: None, the
/// photo's LUT when the library lacks it, the recent LUTs, then every
/// matching LUT. A search lists only matching LUTs.
fn look_lut_items(view: &LookLutView<'_>, query: &str) -> Vec<PreviewListItem<Option<LutId>>> {
    let selected = view.look.map(|look| look.id);
    let row = |entry: &LutEntry| {
        PreviewListItem::choice(Some(entry.id), &entry.name, selected == Some(entry.id))
    };
    let mut items = Vec::new();
    if query.is_empty() {
        items.push(PreviewListItem::choice(None, "None", selected.is_none()));
    }
    if let Some(look) = view
        .look
        .filter(|look| view.entries.iter().all(|entry| entry.id != look.id))
    {
        items.push(PreviewListItem::Choice {
            key: Some(look.id),
            label: format!("{} (missing)", look.name),
            selected: true,
            enabled: false,
        });
    }
    if query.is_empty() {
        let recent: Vec<_> = view
            .recent
            .iter()
            .filter_map(|id| view.entries.iter().find(|entry| entry.id == *id))
            .collect();
        if view.entries.len() > SEARCH_FROM_LUTS && !recent.is_empty() {
            items.push(PreviewListItem::Heading("Recent"));
            items.extend(recent.into_iter().map(row));
            items.push(PreviewListItem::Heading("All"));
        }
        items.extend(view.entries.iter().map(row));
    } else {
        items.extend(
            view.entries
                .iter()
                .filter(|entry| entry.name.to_lowercase().contains(query))
                .map(row),
        );
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, fingerprint: u8) -> LutEntry {
        LutEntry {
            id: LutId::of_bytes(&[fingerprint]),
            name: name.to_owned(),
            size: 33,
        }
    }

    fn labels(items: &[PreviewListItem<Option<LutId>>]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                PreviewListItem::Heading(title) => format!("# {title}"),
                PreviewListItem::Choice { label, .. } => label.clone(),
            })
            .collect()
    }

    fn view<'a>(entries: &'a [LutEntry], recent: &'a [LutId]) -> LookLutView<'a> {
        LookLutView {
            look: None,
            entries,
            recent,
            can_import: true,
            layout: ScreenLayout::Horizontal,
        }
    }

    #[test]
    fn a_small_library_lists_none_and_every_lut() {
        let entries = [entry("Film", 1), entry("Warm", 2)];
        let recent = [entries[1].id];
        assert_eq!(
            labels(&look_lut_items(&view(&entries, &recent), "")),
            ["None", "Film", "Warm"]
        );
    }

    #[test]
    fn a_large_library_lists_recent_luts_first_and_searches_by_name() {
        let entries: Vec<_> = (0..=SEARCH_FROM_LUTS as u8)
            .map(|index| entry(&format!("Look {index}"), index))
            .collect();
        let recent = [entries[3].id];
        let items = look_lut_items(&view(&entries, &recent), "");
        assert_eq!(labels(&items[..4]), ["None", "# Recent", "Look 3", "# All"]);
        assert_eq!(items.len(), 4 + entries.len());

        let found = look_lut_items(&view(&entries, &recent), "look 8");
        assert_eq!(labels(&found), ["Look 8"]);
    }

    #[test]
    fn a_lut_missing_from_the_library_stays_listed_and_selected() {
        let entries = [entry("Film", 1)];
        let missing = LookLut {
            id: LutId::of_bytes(b"gone"),
            name: "Gone".to_owned(),
            amount: 1.0,
        };
        let view = LookLutView {
            look: Some(&missing),
            ..view(&entries, &[])
        };
        let items = look_lut_items(&view, "");
        assert_eq!(labels(&items), ["None", "Gone (missing)", "Film"]);
        assert!(matches!(
            items[1],
            PreviewListItem::Choice {
                selected: true,
                enabled: false,
                ..
            }
        ));
    }

    #[test]
    fn the_portrait_card_fits_the_panel_with_its_list_scrolling_inside() {
        let entries: Vec<_> = (0..30u8)
            .map(|index| entry(&format!("Look {index}"), index))
            .collect();
        let look = LookLut {
            id: entries[0].id,
            name: entries[0].name.clone(),
            amount: 1.0,
        };
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let panel = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 520.0));
        let mut frame = || {
            let mut overflow = 0.0;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(panel),
                    ..Default::default()
                },
                |ui| {
                    let scroll = egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(140.0);
                            Sidebar::show_look_lut(
                                ui,
                                &LookLutView {
                                    look: Some(&look),
                                    layout: ScreenLayout::Vertical,
                                    ..view(&entries, &[])
                                },
                            );
                        });
                    overflow = scroll.content_size.y - scroll.inner_rect.height();
                    Sidebar::settle_portrait_fill(ui.ctx(), overflow);
                },
            );
            (overflow, output.shapes)
        };
        frame();
        let (overflow, shapes) = frame();
        assert!(overflow.abs() < 1.0, "{overflow}");
        let amount =
            crate::ui::sidebar::test_support::text_rect(&shapes, "Amount").expect("Amount label");
        assert!(amount.bottom() <= panel.bottom(), "{amount:?}");
        assert!(crate::ui::sidebar::test_support::text_rect(&shapes, "Look 29").is_none());
    }
}
