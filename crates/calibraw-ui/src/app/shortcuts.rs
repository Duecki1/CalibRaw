//! The keyboard shortcuts the user can change, a preference saved with the app
//! settings. Each [`ShortcutAction`] has default bindings; the settings file
//! stores only the actions whose bindings differ from them, so an untouched
//! action follows a later version's defaults.
//!
//! Matching follows egui's `consume_key`: an extra Shift or Alt is ignored, so
//! the bindings with more modifiers are tried first ([`Shortcuts::take`]).
//! Rating and flag keys ([`ShortcutAction::is_review`]) are plain keys; holding
//! Shift with one also opens the next photo, and a key that is not a letter
//! also matches by position, so digits work on layouts that need Shift for
//! them.

use crate::i18n::tr;
use crate::sidecar::PhotoFlag;
use crate::ui::library::ReviewChange;
use eframe::egui;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// The most bindings one action keeps.
pub(crate) const MAX_BINDINGS: usize = 3;

/// The settings group an action is listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShortcutGroup {
    General,
    Zoom,
    Library,
    Review,
}

impl ShortcutGroup {
    pub(crate) const ALL: [Self; 4] = [Self::General, Self::Zoom, Self::Library, Self::Review];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => tr("General"),
            Self::Zoom => tr("Zoom"),
            Self::Library => tr("Library"),
            Self::Review => tr("Rating and flags"),
        }
    }
}

/// Where a shortcut works. Two actions whose scopes overlap cannot share a
/// binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// Every tab.
    Everywhere,
    Develop,
    Library,
    /// Library and Develop.
    Photos,
}

impl Scope {
    const fn overlaps(self, other: Self) -> bool {
        match (self, other) {
            (Self::Everywhere, _) | (_, Self::Everywhere) => true,
            (Self::Photos, _) | (_, Self::Photos) => true,
            (Self::Develop, Self::Develop) | (Self::Library, Self::Library) => true,
            (Self::Develop, Self::Library) | (Self::Library, Self::Develop) => false,
        }
    }

    /// Whether the library grid's own keys work in this scope.
    const fn includes_library(self) -> bool {
        !matches!(self, Self::Develop)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ShortcutAction {
    Undo,
    Redo,
    SaveEdits,
    PreviousPhoto,
    NextPhoto,
    ToggleClipping,
    NextPreviewBackground,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomNative,
    SearchFilenames,
    SelectAll,
    CopyAdjustments,
    ChooseAdjustmentsToCopy,
    PasteAdjustments,
    Export,
    Delete,
    ClearRating,
    Rate1,
    Rate2,
    Rate3,
    Rate4,
    Rate5,
    LowerRating,
    RaiseRating,
    Pick,
    Reject,
    Unflag,
    TogglePick,
}

impl ShortcutAction {
    /// Every action, in the order the settings list them.
    pub(crate) const ALL: [Self; 30] = [
        Self::Undo,
        Self::Redo,
        Self::SaveEdits,
        Self::PreviousPhoto,
        Self::NextPhoto,
        Self::ToggleClipping,
        Self::NextPreviewBackground,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomFit,
        Self::ZoomNative,
        Self::SearchFilenames,
        Self::SelectAll,
        Self::CopyAdjustments,
        Self::ChooseAdjustmentsToCopy,
        Self::PasteAdjustments,
        Self::Export,
        Self::Delete,
        Self::ClearRating,
        Self::Rate1,
        Self::Rate2,
        Self::Rate3,
        Self::Rate4,
        Self::Rate5,
        Self::LowerRating,
        Self::RaiseRating,
        Self::Pick,
        Self::Reject,
        Self::Unflag,
        Self::TogglePick,
    ];

    /// The rating and flag actions, which Library and Develop both handle.
    pub(crate) const REVIEW: [Self; 12] = [
        Self::ClearRating,
        Self::Rate1,
        Self::Rate2,
        Self::Rate3,
        Self::Rate4,
        Self::Rate5,
        Self::LowerRating,
        Self::RaiseRating,
        Self::Pick,
        Self::Reject,
        Self::Unflag,
        Self::TogglePick,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Undo => tr("Undo"),
            Self::Redo => tr("Redo"),
            Self::SaveEdits => tr("Save edits now"),
            Self::PreviousPhoto => tr("Previous photo in Develop"),
            Self::NextPhoto => tr("Next photo in Develop"),
            Self::ToggleClipping => tr("Toggle shadow and highlight clipping"),
            Self::NextPreviewBackground => tr("Switch the preview background"),
            Self::ZoomIn => tr("Zoom in"),
            Self::ZoomOut => tr("Zoom out"),
            Self::ZoomFit => tr("Fit photo to view"),
            Self::ZoomNative => tr("Zoom to 100%"),
            Self::SearchFilenames => tr("Search filenames"),
            Self::SelectAll => tr("Select all"),
            Self::CopyAdjustments => tr("Copy adjustments"),
            Self::ChooseAdjustmentsToCopy => tr("Choose adjustments to copy…"),
            Self::PasteAdjustments => tr("Paste adjustments"),
            Self::Export => tr("Export"),
            Self::Delete => tr("Delete"),
            Self::ClearRating => tr("Clear the rating"),
            Self::Rate1 => tr("Rate 1 star"),
            Self::Rate2 => tr("Rate 2 stars"),
            Self::Rate3 => tr("Rate 3 stars"),
            Self::Rate4 => tr("Rate 4 stars"),
            Self::Rate5 => tr("Rate 5 stars"),
            Self::LowerRating => tr("Lower the rating"),
            Self::RaiseRating => tr("Raise the rating"),
            Self::Pick => tr("Pick"),
            Self::Reject => tr("Reject"),
            Self::Unflag => tr("Unflag"),
            Self::TogglePick => tr("Switch between picked and unflagged"),
        }
    }

    pub(crate) const fn group(self) -> ShortcutGroup {
        match self {
            Self::Undo
            | Self::Redo
            | Self::SaveEdits
            | Self::PreviousPhoto
            | Self::NextPhoto
            | Self::ToggleClipping
            | Self::NextPreviewBackground => ShortcutGroup::General,
            Self::ZoomIn | Self::ZoomOut | Self::ZoomFit | Self::ZoomNative => ShortcutGroup::Zoom,
            Self::SearchFilenames
            | Self::SelectAll
            | Self::CopyAdjustments
            | Self::ChooseAdjustmentsToCopy
            | Self::PasteAdjustments
            | Self::Export
            | Self::Delete => ShortcutGroup::Library,
            Self::ClearRating
            | Self::Rate1
            | Self::Rate2
            | Self::Rate3
            | Self::Rate4
            | Self::Rate5
            | Self::LowerRating
            | Self::RaiseRating
            | Self::Pick
            | Self::Reject
            | Self::Unflag
            | Self::TogglePick => ShortcutGroup::Review,
        }
    }

    const fn scope(self) -> Scope {
        match self.group() {
            ShortcutGroup::General => match self {
                Self::SaveEdits => Scope::Everywhere,
                _ => Scope::Develop,
            },
            ShortcutGroup::Zoom => Scope::Develop,
            ShortcutGroup::Library => Scope::Library,
            ShortcutGroup::Review => Scope::Photos,
        }
    }

    /// A rating or flag action: bound to plain keys, with Shift opening the
    /// next photo.
    pub(crate) const fn is_review(self) -> bool {
        matches!(self.group(), ShortcutGroup::Review)
    }

    /// The review change of a rating or flag action.
    pub(crate) const fn review_change(self) -> Option<ReviewChange> {
        Some(match self {
            Self::ClearRating => ReviewChange::Rating(0),
            Self::Rate1 => ReviewChange::Rating(1),
            Self::Rate2 => ReviewChange::Rating(2),
            Self::Rate3 => ReviewChange::Rating(3),
            Self::Rate4 => ReviewChange::Rating(4),
            Self::Rate5 => ReviewChange::Rating(5),
            Self::LowerRating => ReviewChange::StepRating(-1),
            Self::RaiseRating => ReviewChange::StepRating(1),
            Self::Pick => ReviewChange::Flag(PhotoFlag::Picked),
            Self::Reject => ReviewChange::Flag(PhotoFlag::Rejected),
            Self::Unflag => ReviewChange::Flag(PhotoFlag::Unflagged),
            Self::TogglePick => ReviewChange::TogglePick,
            _ => return None,
        })
    }

    /// The bindings of a fresh install.
    pub(crate) const fn defaults(self) -> &'static [KeyBinding] {
        use egui::Key;
        match self {
            Self::Undo => &const { [KeyBinding::command(Key::Z)] },
            Self::Redo => {
                &const {
                    [
                        KeyBinding::command_shift(Key::Z),
                        KeyBinding::command(Key::Y),
                    ]
                }
            }
            Self::SaveEdits => &const { [KeyBinding::command(Key::S)] },
            Self::PreviousPhoto => &const { [KeyBinding::plain(Key::ArrowLeft)] },
            Self::NextPhoto => &const { [KeyBinding::plain(Key::ArrowRight)] },
            Self::ToggleClipping => &const { [KeyBinding::plain(Key::J)] },
            Self::NextPreviewBackground => &const { [KeyBinding::plain(Key::B)] },
            // egui's interface-zoom keys, which CalibRaw uses for the preview.
            Self::ZoomIn => {
                &const {
                    [
                        KeyBinding::command(Key::Plus),
                        KeyBinding::command(Key::Equals),
                    ]
                }
            }
            Self::ZoomOut => &const { [KeyBinding::command(Key::Minus)] },
            Self::ZoomFit => &const { [KeyBinding::command(Key::Num0)] },
            Self::ZoomNative => &const { [KeyBinding::command(Key::Num1)] },
            Self::SearchFilenames => &const { [KeyBinding::command(Key::F)] },
            Self::SelectAll => &const { [KeyBinding::command(Key::A)] },
            Self::CopyAdjustments => &const { [KeyBinding::command_shift(Key::C)] },
            Self::ChooseAdjustmentsToCopy => {
                &const {
                    [KeyBinding {
                        command: true,
                        shift: true,
                        alt: true,
                        key: Key::C,
                    }]
                }
            }
            Self::PasteAdjustments => &const { [KeyBinding::command_shift(Key::V)] },
            Self::Export => &const { [KeyBinding::command_shift(Key::E)] },
            Self::Delete => {
                &const {
                    [
                        KeyBinding::plain(Key::Delete),
                        KeyBinding::plain(Key::Backspace),
                    ]
                }
            }
            Self::ClearRating => &const { [KeyBinding::plain(Key::Num0)] },
            Self::Rate1 => &const { [KeyBinding::plain(Key::Num1)] },
            Self::Rate2 => &const { [KeyBinding::plain(Key::Num2)] },
            Self::Rate3 => &const { [KeyBinding::plain(Key::Num3)] },
            Self::Rate4 => &const { [KeyBinding::plain(Key::Num4)] },
            Self::Rate5 => &const { [KeyBinding::plain(Key::Num5)] },
            Self::LowerRating => &const { [KeyBinding::plain(Key::OpenBracket)] },
            Self::RaiseRating => &const { [KeyBinding::plain(Key::CloseBracket)] },
            Self::Pick => &const { [KeyBinding::plain(Key::P)] },
            Self::Reject => &const { [KeyBinding::plain(Key::X)] },
            Self::Unflag => &const { [KeyBinding::plain(Key::U)] },
            Self::TogglePick => &const { [KeyBinding::plain(Key::Backtick)] },
        }
    }

    /// The name the settings file stores. Never change one.
    const fn key(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::SaveEdits => "save_edits",
            Self::PreviousPhoto => "previous_photo",
            Self::NextPhoto => "next_photo",
            Self::ToggleClipping => "toggle_clipping",
            Self::NextPreviewBackground => "next_preview_background",
            Self::ZoomIn => "zoom_in",
            Self::ZoomOut => "zoom_out",
            Self::ZoomFit => "zoom_fit",
            Self::ZoomNative => "zoom_native",
            Self::SearchFilenames => "search_filenames",
            Self::SelectAll => "select_all",
            Self::CopyAdjustments => "copy_adjustments",
            Self::ChooseAdjustmentsToCopy => "choose_adjustments_to_copy",
            Self::PasteAdjustments => "paste_adjustments",
            Self::Export => "export",
            Self::Delete => "delete",
            Self::ClearRating => "clear_rating",
            Self::Rate1 => "rate_1",
            Self::Rate2 => "rate_2",
            Self::Rate3 => "rate_3",
            Self::Rate4 => "rate_4",
            Self::Rate5 => "rate_5",
            Self::LowerRating => "lower_rating",
            Self::RaiseRating => "raise_rating",
            Self::Pick => "pick",
            Self::Reject => "reject",
            Self::Unflag => "unflag",
            Self::TogglePick => "toggle_pick",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.key() == key)
    }
}

/// A key with the modifiers that must be held. `command` is egui's
/// `COMMAND`: Ctrl, or Cmd on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyBinding {
    command: bool,
    shift: bool,
    alt: bool,
    key: egui::Key,
}

impl KeyBinding {
    const fn plain(key: egui::Key) -> Self {
        Self {
            command: false,
            shift: false,
            alt: false,
            key,
        }
    }

    const fn command(key: egui::Key) -> Self {
        Self {
            command: true,
            ..Self::plain(key)
        }
    }

    const fn command_shift(key: egui::Key) -> Self {
        Self {
            shift: true,
            ..Self::command(key)
        }
    }

    /// The binding a key press records, or `None` for a press that cannot
    /// be one: Ctrl without Cmd on macOS, which egui's `COMMAND` does not
    /// cover.
    pub(crate) fn from_press(key: egui::Key, modifiers: egui::Modifiers) -> Option<Self> {
        if modifiers.ctrl && !modifiers.command {
            return None;
        }
        Some(Self {
            command: modifiers.command,
            shift: modifiers.shift,
            alt: modifiers.alt,
            key,
        })
    }

    fn modifiers(self) -> egui::Modifiers {
        egui::Modifiers {
            alt: self.alt,
            shift: self.shift,
            command: self.command,
            ..egui::Modifiers::NONE
        }
    }

    fn modifier_count(self) -> u8 {
        u8::from(self.command) + u8::from(self.shift) + u8::from(self.alt)
    }

    /// The binding as the settings show it, e.g. `Ctrl+Shift+Z`.
    pub(crate) fn label(self) -> String {
        let mut label = String::new();
        for (held, name) in [
            (self.command, crate::platform::PRIMARY_MODIFIER_NAME),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if held {
                label.push_str(name);
                label.push('+');
            }
        }
        label.push_str(key_label(self.key));
        label
    }

    /// The binding as the settings file stores it, e.g. `Command+Shift+Z`.
    fn stored(self) -> String {
        let mut stored = String::new();
        for (held, name) in [
            (self.command, "Command"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if held {
                stored.push_str(name);
                stored.push('+');
            }
        }
        stored.push_str(self.key.name());
        stored
    }

    fn from_stored(stored: &str) -> Option<Self> {
        // The key comes last; its name can itself be "+" only as "Plus".
        let (modifiers, key) = stored.rsplit_once('+').unwrap_or(("", stored));
        let mut binding = Self::plain(egui::Key::from_name(key)?);
        for modifier in modifiers.split('+').filter(|part| !part.is_empty()) {
            match modifier {
                "Command" => binding.command = true,
                "Alt" => binding.alt = true,
                "Shift" => binding.shift = true,
                _ => return None,
            }
        }
        Some(binding)
    }

    /// Whether a press with `modifiers` of `key` (`physical_key` where it
    /// sits) is this rating or flag binding, with or without Shift.
    fn matches_review(
        self,
        key: egui::Key,
        physical_key: Option<egui::Key>,
        modifiers: egui::Modifiers,
    ) -> bool {
        if modifiers.command || modifiers.ctrl || modifiers.alt {
            return false;
        }
        key == self.key || (!is_letter(self.key) && physical_key == Some(self.key))
    }
}

fn is_letter(key: egui::Key) -> bool {
    key.name().len() == 1 && key.name().chars().all(|c| c.is_ascii_alphabetic())
}

fn key_label(key: egui::Key) -> &'static str {
    use egui::Key;
    match key {
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Escape => "Esc",
        Key::PageUp => "Page Up",
        Key::PageDown => "Page Down",
        Key::Minus => "-",
        other => other.symbol_or_name(),
    }
}

/// Why a key press cannot become a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingRefusal {
    /// The key belongs to a control that cannot be changed.
    Reserved,
    /// Rating and flag keys take no Ctrl, Cmd or Alt.
    ReviewNeedsPlainKey,
}

impl BindingRefusal {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Reserved => tr("That key is used by a control that cannot be changed."),
            Self::ReviewNeedsPlainKey => tr("Rating and flag keys work without Ctrl, Cmd or Alt."),
        }
    }
}

/// Whether `binding` belongs to a fixed control where `scope` works: Escape
/// and Tab everywhere, Space for panning, and the library grid's movement
/// and opening keys.
fn reserved(binding: KeyBinding, scope: Scope) -> bool {
    use egui::Key;
    if matches!(binding.key, Key::Escape | Key::Tab) {
        return true;
    }
    if binding.command || binding.alt {
        return false;
    }
    match binding.key {
        Key::Space => true,
        Key::ArrowLeft
        | Key::ArrowRight
        | Key::ArrowUp
        | Key::ArrowDown
        | Key::PageUp
        | Key::PageDown
        | Key::Home
        | Key::End
        | Key::Enter => scope.includes_library(),
        _ => false,
    }
}

/// Whether `a` bound to `a_action` and `b` bound to `b_action` would answer
/// the same press. A rating or flag key also takes the press with Shift.
fn collide(
    a_action: ShortcutAction,
    a: KeyBinding,
    b_action: ShortcutAction,
    b: KeyBinding,
) -> bool {
    if a.key != b.key || !a_action.scope().overlaps(b_action.scope()) {
        return false;
    }
    if a == b {
        return true;
    }
    let shift_only = |binding: KeyBinding| !binding.command && !binding.alt;
    (a_action.is_review() && shift_only(b)) || (b_action.is_review() && shift_only(a))
}

/// Every action's bindings: the defaults, with the user's changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shortcuts {
    /// Only actions whose bindings differ from their defaults. An empty list
    /// leaves the action without a shortcut.
    changed: BTreeMap<ShortcutAction, Vec<KeyBinding>>,
}

impl Shortcuts {
    pub(crate) fn bindings(&self, action: ShortcutAction) -> &[KeyBinding] {
        self.changed
            .get(&action)
            .map_or(action.defaults(), Vec::as_slice)
    }

    pub(crate) fn is_changed(&self, action: ShortcutAction) -> bool {
        self.changed.contains_key(&action)
    }

    /// Whether every action has its default bindings.
    pub(crate) fn is_default(&self) -> bool {
        self.changed.is_empty()
    }

    /// The first binding's label, for text that names the shortcut.
    pub(crate) fn label(&self, action: ShortcutAction) -> Option<String> {
        self.bindings(action).first().map(|binding| binding.label())
    }

    fn set(&mut self, action: ShortcutAction, bindings: Vec<KeyBinding>) {
        if bindings == action.defaults() {
            self.changed.remove(&action);
        } else {
            self.changed.insert(action, bindings);
        }
    }

    /// Binds `binding` to `action`, replacing its binding at `replace` or
    /// adding it. The actions that answered the same press lose that
    /// binding; they are returned, so the settings can say so.
    pub(crate) fn assign(
        &mut self,
        action: ShortcutAction,
        replace: Option<usize>,
        mut binding: KeyBinding,
    ) -> Result<Vec<ShortcutAction>, BindingRefusal> {
        if action.is_review() {
            if binding.command || binding.alt {
                return Err(BindingRefusal::ReviewNeedsPlainKey);
            }
            // Shift is how a rating or flag key opens the next photo.
            binding.shift = false;
        }
        if reserved(binding, action.scope()) {
            return Err(BindingRefusal::Reserved);
        }

        let mut taken_from = Vec::new();
        for other in ShortcutAction::ALL {
            if other == action {
                continue;
            }
            let kept: Vec<KeyBinding> = self
                .bindings(other)
                .iter()
                .copied()
                .filter(|existing| !collide(action, binding, other, *existing))
                .collect();
            if kept.len() != self.bindings(other).len() {
                self.set(other, kept);
                taken_from.push(other);
            }
        }

        let mut bindings = self.bindings(action).to_vec();
        bindings.retain(|existing| *existing != binding);
        match replace {
            Some(index) if index < bindings.len() => bindings[index] = binding,
            _ if bindings.len() < MAX_BINDINGS => bindings.push(binding),
            _ => {
                if let Some(last) = bindings.last_mut() {
                    *last = binding;
                }
            }
        }
        self.set(action, bindings);
        Ok(taken_from)
    }

    pub(crate) fn remove(&mut self, action: ShortcutAction, index: usize) {
        let mut bindings = self.bindings(action).to_vec();
        if index < bindings.len() {
            bindings.remove(index);
            self.set(action, bindings);
        }
    }

    pub(crate) fn reset(&mut self, action: ShortcutAction) {
        self.changed.remove(&action);
    }

    pub(crate) fn reset_all(&mut self) {
        self.changed.clear();
    }

    /// Consumes this frame's first press of one of `actions`' bindings and
    /// returns its action. Bindings with more modifiers are tried first, as
    /// a press matches a binding that lacks its Shift or Alt.
    pub(crate) fn take(
        &self,
        input: &mut egui::InputState,
        actions: &[ShortcutAction],
    ) -> Option<ShortcutAction> {
        let mut candidates: Vec<(KeyBinding, ShortcutAction)> = actions
            .iter()
            .flat_map(|&action| {
                self.bindings(action)
                    .iter()
                    .map(move |&binding| (binding, action))
            })
            .collect();
        candidates.sort_by_key(|(binding, _)| std::cmp::Reverse(binding.modifier_count()));
        candidates
            .into_iter()
            .find(|(binding, _)| input.consume_key(binding.modifiers(), binding.key))
            .map(|(_, action)| action)
    }

    /// Consumes a press of `action`'s bindings.
    pub(crate) fn consume(&self, input: &mut egui::InputState, action: ShortcutAction) -> bool {
        self.take(input, &[action]).is_some()
    }

    /// Whether `action`'s bindings were pressed this frame, leaving the press
    /// for others.
    pub(crate) fn pressed(&self, input: &egui::InputState, action: ShortcutAction) -> bool {
        self.bindings(action).iter().any(|binding| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { key, pressed: true, modifiers, .. }
                        if *key == binding.key && modifiers.matches_logically(binding.modifiers())
                )
            })
        })
    }

    /// Takes this frame's first rating or flag key press: its action, and
    /// whether Shift was held.
    pub(crate) fn take_review(
        &self,
        input: &mut egui::InputState,
    ) -> Option<(ShortcutAction, bool)> {
        let found = input.events.iter().enumerate().find_map(|(index, event)| {
            let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                modifiers,
            } = event
            else {
                return None;
            };
            ShortcutAction::REVIEW
                .into_iter()
                .find(|&action| {
                    self.bindings(action)
                        .iter()
                        .any(|binding| binding.matches_review(*key, *physical_key, *modifiers))
                })
                .map(|action| (index, action, modifiers.shift))
        });
        let (index, action, shift) = found?;
        input.events.remove(index);
        Some((action, shift))
    }
}

impl Serialize for Shortcuts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.changed.iter().map(|(action, bindings)| {
            (
                action.key(),
                bindings
                    .iter()
                    .map(|binding| binding.stored())
                    .collect::<Vec<_>>(),
            )
        }))
    }
}

impl<'de> Deserialize<'de> for Shortcuts {
    /// Actions this version does not know are skipped, and so are bindings
    /// it cannot read, rather than failing the whole settings file. An
    /// action none of whose stored bindings can be read keeps its defaults.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = BTreeMap::<String, Vec<String>>::deserialize(deserializer)?;
        let mut shortcuts = Self::default();
        for (key, bindings) in stored {
            let Some(action) = ShortcutAction::from_key(&key) else {
                continue;
            };
            let mut parsed: Vec<KeyBinding> = Vec::new();
            for binding in bindings.iter().filter_map(|b| KeyBinding::from_stored(b)) {
                if !parsed.contains(&binding) && parsed.len() < MAX_BINDINGS {
                    parsed.push(binding);
                }
            }
            if parsed.is_empty() && !bindings.is_empty() {
                continue;
            }
            shortcuts.set(action, parsed);
        }
        Ok(shortcuts)
    }
}
