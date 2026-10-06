//! Adding, selecting, duplicating, moving and deleting masks and components.

use super::*;

impl MaskStack {
    pub fn add_mask(&mut self, kind: MaskKind) -> Option<(usize, usize)> {
        if self.masks.len() >= MAX_LOCAL_MASKS || !kind.is_available() {
            return None;
        }
        let mask_index = self.masks.len();
        self.masks.push(LocalMask::new(kind, mask_index + 1));
        self.select_mask(mask_index);
        Some((mask_index, 0))
    }

    pub fn add_component(
        &mut self,
        kind: MaskKind,
        combine: MaskCombineMode,
    ) -> Option<(usize, usize)> {
        if !kind.is_available() {
            return None;
        }
        let mask_index = self.selected_mask?;
        let mask = self.masks.get_mut(mask_index)?;
        if mask.components.len() >= MAX_MASK_COMPONENTS {
            return None;
        }
        let component_index = mask.components.len();
        mask.components.push(MaskComponent::new(kind, combine));
        self.selected_component = Some(component_index);
        Some((mask_index, component_index))
    }

    /// Preserve a completed object selection and draw into a new submask.
    pub fn prepare_object_stroke(
        &mut self,
        mask_index: usize,
        component_index: usize,
    ) -> Option<usize> {
        if self.selected_mask != Some(mask_index) {
            return None;
        }
        let component = self
            .masks
            .get(mask_index)?
            .components
            .get(component_index)?;
        let MaskGeometry::Object { mask, .. } = &component.geometry else {
            return None;
        };
        if mask.is_none() {
            return Some(component_index);
        }
        let mut geometry = component.geometry.clone();
        if let MaskGeometry::Object { mask, strokes, .. } = &mut geometry {
            *mask = None;
            strokes.clear();
        }
        let combine = component.combine;
        let (_, new_index) = self.add_component(MaskKind::Object, combine)?;
        self.masks[mask_index].components[new_index].geometry = geometry;
        Some(new_index)
    }

    pub fn selected_mask(&self) -> Option<&LocalMask> {
        self.masks.get(self.selected_mask?)
    }

    pub fn selected_mask_mut(&mut self) -> Option<&mut LocalMask> {
        self.masks.get_mut(self.selected_mask?)
    }

    pub fn selected_component(&self) -> Option<&MaskComponent> {
        self.selected_mask()?
            .components
            .get(self.selected_component?)
    }

    pub fn selected_component_mut(&mut self) -> Option<&mut MaskComponent> {
        let component_index = self.selected_component?;
        self.selected_mask_mut()?
            .components
            .get_mut(component_index)
    }

    pub fn ensure_selection(&mut self) -> Option<(usize, usize)> {
        // Older sidecars stored the generated default component name as text.
        // Keep user-renamed components intact while updating that old default.
        for component in self.masks.iter_mut().flat_map(|mask| &mut mask.components) {
            if component.kind == MaskKind::Background && component.name == "Select Not Subject" {
                component.name = MaskKind::Background.label().to_owned();
            }
        }
        if self.masks.is_empty() {
            self.selected_mask = None;
            self.selected_component = None;
            return None;
        }
        let mask_index = self
            .selected_mask
            .filter(|&index| index < self.masks.len())
            .unwrap_or(self.masks.len() - 1);
        let component_count = self.masks[mask_index].components.len();
        if component_count == 0 {
            self.selected_mask = Some(mask_index);
            self.selected_component = None;
            return None;
        }
        let component_index = self
            .selected_component
            .filter(|&index| index < component_count)
            .unwrap_or(0);
        self.selected_mask = Some(mask_index);
        self.selected_component = Some(component_index);
        Some((mask_index, component_index))
    }

    pub fn select_mask(&mut self, mask_index: usize) -> bool {
        if mask_index >= self.masks.len() {
            return false;
        }
        self.selected_mask = Some(mask_index);
        self.selected_component = (!self.masks[mask_index].components.is_empty()).then_some(0);
        true
    }

    pub fn select_component(&mut self, mask_index: usize, component_index: usize) -> bool {
        if self
            .masks
            .get(mask_index)
            .is_none_or(|mask| component_index >= mask.components.len())
        {
            return false;
        }
        self.selected_mask = Some(mask_index);
        self.selected_component = Some(component_index);
        true
    }

    pub fn delete_mask(&mut self, mask_index: usize) -> bool {
        if mask_index >= self.masks.len() {
            return false;
        }
        self.masks.remove(mask_index);
        for (number, mask) in self.masks.iter_mut().enumerate() {
            if mask.name.starts_with("Mask ") {
                mask.name = format!("Mask {}", number + 1);
            }
        }
        if self.masks.is_empty() {
            self.selected_mask = None;
            self.selected_component = None;
        } else {
            self.select_mask(mask_index.min(self.masks.len() - 1));
        }
        true
    }

    pub fn delete_component(&mut self, mask_index: usize, component_index: usize) -> bool {
        let Some(mask) = self.masks.get_mut(mask_index) else {
            return false;
        };
        if mask.components.len() <= 1 || component_index >= mask.components.len() {
            return false;
        }
        mask.components.remove(component_index);
        self.selected_mask = Some(mask_index);
        self.selected_component = Some(component_index.min(mask.components.len() - 1));
        true
    }

    pub fn duplicate_mask(&mut self, mask_index: usize, invert: bool) -> bool {
        let Some(mask) = self.masks.get(mask_index).cloned() else {
            return false;
        };
        self.insert_mask_copy(mask_index, mask, invert)
    }

    pub fn insert_mask_copy(
        &mut self,
        mask_index: usize,
        mut mask: LocalMask,
        invert: bool,
    ) -> bool {
        if self.masks.len() >= MAX_LOCAL_MASKS || mask_index >= self.masks.len() {
            return false;
        }
        mask.name = copied_name(&mask.name, |candidate| {
            self.masks.iter().any(|mask| mask.name == candidate)
        });
        if invert {
            mask.common.toggle_invert();
            mask.adjustments.reset();
        }
        let insert_at = mask_index + 1;
        self.masks.insert(insert_at, mask);
        self.select_mask(insert_at);
        true
    }

    pub fn duplicate_component(
        &mut self,
        mask_index: usize,
        component_index: usize,
        invert: bool,
    ) -> bool {
        let Some(component) = self
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))
            .cloned()
        else {
            return false;
        };
        self.insert_component_copy(mask_index, component_index, component, invert)
    }

    pub fn insert_component_copy(
        &mut self,
        mask_index: usize,
        component_index: usize,
        mut component: MaskComponent,
        invert: bool,
    ) -> bool {
        let Some(mask) = self.masks.get_mut(mask_index) else {
            return false;
        };
        if mask.components.len() >= MAX_MASK_COMPONENTS || component_index >= mask.components.len()
        {
            return false;
        }
        component.name = copied_name(&component.name, |candidate| {
            mask.components
                .iter()
                .any(|component| component.name == candidate)
        });
        if invert {
            component.common.toggle_invert();
        }
        let insert_at = component_index + 1;
        mask.components.insert(insert_at, component);
        self.selected_mask = Some(mask_index);
        self.selected_component = Some(insert_at);
        true
    }

    pub fn move_submask_component(
        &mut self,
        source_mask: usize,
        source_component: usize,
        target_mask: usize,
        target_insert: usize,
    ) -> Option<(usize, usize)> {
        let source = self.masks.get(source_mask)?;
        if source.components.len() <= 1 || source_component >= source.components.len() {
            return None;
        }
        let target = self.masks.get(target_mask)?;
        if source_mask != target_mask && target.components.len() >= MAX_MASK_COMPONENTS {
            return None;
        }

        let component = self.masks[source_mask].components.remove(source_component);
        let adjusted_insert = if source_mask == target_mask && target_insert > source_component {
            target_insert - 1
        } else {
            target_insert
        };
        let insert_at = adjusted_insert.min(self.masks[target_mask].components.len());
        self.masks[target_mask]
            .components
            .insert(insert_at, component);
        self.selected_mask = Some(target_mask);
        self.selected_component = Some(insert_at);
        Some((target_mask, insert_at))
    }

    /// Move a group to `target_insert`, an insertion index in `masks` before
    /// the move. Groups apply in index order, so this changes how overlapping
    /// groups combine. Selects the moved group, keeping its selected sub-mask
    /// when it was already selected. Returns the new index, or `None` when
    /// the group stays where it is.
    pub fn move_mask(&mut self, source_mask: usize, target_insert: usize) -> Option<usize> {
        if source_mask >= self.masks.len() || target_insert > self.masks.len() {
            return None;
        }
        let insert_at = if target_insert > source_mask {
            target_insert - 1
        } else {
            target_insert
        };
        if insert_at == source_mask {
            return None;
        }

        let selected_component = (self.selected_mask == Some(source_mask))
            .then_some(self.selected_component)
            .flatten();
        let mask = self.masks.remove(source_mask);
        self.masks.insert(insert_at, mask);
        self.select_mask(insert_at);
        if selected_component.is_some() {
            self.selected_component = selected_component;
        }
        Some(insert_at)
    }
}
