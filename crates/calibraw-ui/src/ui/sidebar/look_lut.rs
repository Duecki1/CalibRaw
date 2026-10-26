//! The Develop card that chooses the photo's LUT and its amount.

use super::adjustment_cards::CardAction;
use super::*;
use crate::app::LOOK_LUT_CARD_TITLE;
use crate::luts::{LookLut, LutEntry, LutId};
use crate::ui::components::preview_list::{preview_list, ListHeight, PreviewListItem};

/// Libraries with more LUTs than this get a search field.
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
    pub(super) can_import: bool,
    /// Docked, the card folds and its list is a few rows high. In portrait
    /// it shows alone, and its list fills the panel so the card fits whole.
    pub(super) layout: ScreenLayout,
}

impl Sidebar {
    /// The LUT card: None, the library's LUTs, then the amount of the chosen
    /// LUT and Import…. A LUT the library does not hold is listed as missing.
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
            if view.can_import && moduwu_design::full_width_button(ui, "Import…").clicked() {
                response.action = Some(LookLutAction::Import);
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
/// photo's LUT when the library lacks it, then every matching LUT. A search
/// lists only matching LUTs.
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
        items.push(PreviewListItem {
            key: Some(look.id),
            label: format!("{} (missing)", look.name),
            selected: true,
            enabled: false,
        });
    }
    items.extend(
        view.entries
            .iter()
            .filter(|entry| entry.name.to_lowercase().contains(query))
            .map(row),
    );
    items
}
