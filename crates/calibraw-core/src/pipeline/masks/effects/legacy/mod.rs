//! Effects a rebuilt effect replaced. Edits that hold one keep rendering it
//! as they did, but it is never offered for adding
//! ([`MaskEffect::is_legacy`](super::MaskEffect::is_legacy)): its card shows
//! only its name, its switch, Remove and Replace with new, which switches it
//! to the style [`MaskEffect::replacement`](super::MaskEffect::replacement)
//! names and keeps the values they share (`EffectComponent::set_style`).
//!
//! Each effect is marked `legacy: remove in vX.Y.Z` by the rule AGENTS.md
//! gives migrations of user files, here and in its shader
//! (calibraw-gpu `shaders/mask_effects/legacy/`); `cargo xtask migrations`
//! reports it when due. Removing one removes its registry row, its settings,
//! its shader and its parity cases, and is noted in the release notes.

pub(super) mod halation;
pub(super) mod highlight_glow;
