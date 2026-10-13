//! Background components that keep the default name of v1.0.
//!
//! migration: remove in v2.0.0
//!
//! v1.0 named new Background components "Select Not Subject" and stored that
//! name. Components still carrying it get the current default; renamed
//! components keep their name.

use crate::pipeline::{MaskComponent, MaskKind};
use crate::sidecar::EditState;
use std::sync::Arc;

const OLD_DEFAULT_NAME: &str = "Select Not Subject";

fn has_old_default_name(component: &MaskComponent) -> bool {
    component.kind == MaskKind::Background && component.name == OLD_DEFAULT_NAME
}

/// Renames Background components with the v1.0 default name. Returns whether
/// any was renamed.
pub(super) fn migrate(edits: &mut EditState) -> bool {
    let outdated = edits
        .masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(has_old_default_name);
    if !outdated {
        return false;
    }
    for component in Arc::make_mut(&mut edits.masks)
        .masks
        .iter_mut()
        .flat_map(|mask| &mut mask.components)
        .filter(|component| has_old_default_name(component))
    {
        component.name = MaskKind::Background.label().to_owned();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MaskCombineMode;
    use crate::sidecar::default_edit_state;

    #[test]
    fn only_the_old_default_background_name_is_replaced() {
        let mut edits = default_edit_state();
        let masks = Arc::make_mut(&mut edits.masks);
        masks.add_mask(MaskKind::Background).unwrap();
        masks.masks[0].components[0].name = OLD_DEFAULT_NAME.into();
        masks
            .add_component(MaskKind::Background, MaskCombineMode::Add)
            .unwrap();
        masks.masks[0].components[1].name = "Behind her".into();

        assert!(migrate(&mut edits));
        let names: Vec<_> = edits.masks.masks[0]
            .components
            .iter()
            .map(|component| component.name.as_str())
            .collect();
        assert_eq!(names, [MaskKind::Background.label(), "Behind her"]);
        assert!(!migrate(&mut edits));
    }
}
