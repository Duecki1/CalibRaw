//! The "Close other cards when opening a card" preference, shared by the two
//! kinds of foldable sidebar card: adjustment cards and effect cards.
//!
//! Cards are drawn top to bottom, so the cards above one that opens have already
//! been drawn. The opening is kept for one more pass, in which they close too.
//! Only cards drawn in those two passes react, so the cards of another sidebar
//! tab keep their state.

use super::egui;

/// Room kept above an opened card that would otherwise leave the viewport: the
/// sticky Edit header of the Masks tab covers the top of the sidebar. That
/// header is a toolbar row, its frame's margins and stroke, and a card gap.
const ROOM_ABOVE: f32 = moduwu_design::TOOLBAR_HEIGHT + 30.0;
/// Passes after which an opened card stops following its own position, in case
/// something in it keeps changing its height.
const MAX_SETTLE_PASSES: u64 = 60;

#[derive(Clone, Copy)]
struct Opening {
    card: egui::Id,
    pass: u64,
    /// The card's rect in the previous pass. It stops changing once the folding
    /// animations and any scrolling have ended.
    rect: Option<egui::Rect>,
}

#[derive(Clone, Copy, Default)]
struct State {
    enabled: bool,
    opening: Option<Opening>,
}

fn state_id() -> egui::Id {
    egui::Id::new("calibraw-single-open-cards")
}

fn read(ctx: &egui::Context) -> State {
    ctx.data(|data| data.get_temp::<State>(state_id()))
        .unwrap_or_default()
}

fn write(ctx: &egui::Context, state: State) {
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

/// Applies the preference for this pass. Cards read it from the context, as
/// they are drawn by many views that know nothing of the preferences.
pub(super) fn set_enabled(ctx: &egui::Context, enabled: bool) {
    write(
        ctx,
        State {
            enabled,
            ..read(ctx)
        },
    );
}

/// Whether the card keyed `card` is open in this pass, given the state it drew
/// its header with. `was_open` is its state before this pass and `revealed`
/// says that the app is showing it, which counts as opening it even if it was
/// open already. With the preference off the card simply keeps `open`.
pub(super) fn stays_open(
    ctx: &egui::Context,
    card: egui::Id,
    was_open: bool,
    revealed: bool,
    open: bool,
) -> bool {
    let mut state = read(ctx);
    if !state.enabled {
        return open;
    }
    let pass = ctx.cumulative_pass_nr();
    if revealed || (open && !was_open) {
        state.opening = Some(Opening {
            card,
            pass,
            rect: None,
        });
        write(ctx, state);
        return true;
    }
    let another_opened = state
        .opening
        .is_some_and(|opening| opening.card != card && pass <= opening.pass + 1);
    open && !another_opened
}

/// Whether a card is settling after opening. The sidebar's scroll area must not
/// animate meanwhile (`egui::ScrollArea::animated`): an animated scroll lags a
/// frame behind the layout and counts its moves twice while it follows a moving
/// card, which lets the card leave the viewport and then overshoots far enough
/// to lose it again on a short window.
pub(super) fn settling(ctx: &egui::Context) -> bool {
    read(ctx)
        .opening
        .is_some_and(|opening| ctx.cumulative_pass_nr() <= opening.pass + MAX_SETTLE_PASSES)
}

/// Keeps the header of the card that just opened from sliding out of the top of
/// the viewport while the cards above it fold. Call it after drawing the card,
/// inside the sidebar's scroll area, before any other scroll request for the
/// card so that request wins. `rect` is the whole card.
pub(super) fn keep_in_view(ui: &egui::Ui, card: egui::Id, rect: egui::Rect) {
    let ctx = ui.ctx();
    let mut state = read(ctx);
    let Some(mut opening) = state.opening.filter(|opening| opening.card == card) else {
        return;
    };
    let pass = ctx.cumulative_pass_nr();
    // The cards above only start folding in the pass after the opening.
    let others_started = pass > opening.pass + 1;
    if others_started && (opening.rect == Some(rect) || pass > opening.pass + MAX_SETTLE_PASSES) {
        state.opening = None;
        write(ctx, state);
        return;
    }
    opening.rect = Some(rect);
    state.opening = Some(opening);
    write(ctx, state);

    let top = rect.top() - ROOM_ABOVE;
    if top < ui.clip_rect().top() {
        let header = egui::Rect::from_x_y_ranges(
            rect.x_range(),
            top..=rect.top() + moduwu_design::TOOLBAR_HEIGHT,
        );
        ui.scroll_to_rect(header, None);
    }
    ctx.request_repaint();
}
