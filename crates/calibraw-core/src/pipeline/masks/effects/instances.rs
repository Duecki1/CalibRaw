//! Effects with several instances in one card: Relight's lights and Light
//! Rays' sources.
//!
//! An effect with instances keeps the values every instance shares (Relight's
//! Ambient and Relief, the look of Light Rays) beside an [`EffectInstances`]
//! list, and reports its length through `EffectSettings::instance_count`.
//! calibraw-gpu packs one slot per instance, consecutively and with the same
//! mask, and records in each which instance it holds and how many the card
//! has (`mask_effect_instance` in mask_effects/effect_api.wgsl). Shaders that
//! loop over slots then see every instance as one more slot of the effect;
//! an effect that must combine its instances (Relight shades the surface
//! once with all its lights) does so in the slot of its first instance and
//! leaves the others alone.
//!
//! Every instance after the first takes one of the places a mask or the
//! Effects card has for effects ([`super::MAX_EFFECT_COMPONENTS`]), so the
//! render slots stay within their fixed budget.

use super::params::FloatParamSpec;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Instances one effect card holds at most.
pub const MAX_EFFECT_INSTANCES: usize = 8;

/// The instances of an effect: at least one, at most [`MAX_EFFECT_INSTANCES`].
///
/// Stored with the first instance's fields beside the effect's shared fields,
/// where the effect kept them while it had one instance, and the others in an
/// `additional` list. An edit with one instance therefore stores exactly as
/// before, and readers that predate instances read the first one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectInstances<T> {
    /// The first `count` are in use; the rest stay `T::default()`, so derived
    /// equality compares only the instances in use.
    items: [T; MAX_EFFECT_INSTANCES],
    count: usize,
}

impl<T: Copy + Default> EffectInstances<T> {
    pub fn new(first: T) -> Self {
        let mut items = [T::default(); MAX_EFFECT_INSTANCES];
        items[0] = first;
        Self { items, count: 1 }
    }

    /// Instances in use; at least one.
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn is_full(&self) -> bool {
        self.count == MAX_EFFECT_INSTANCES
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items[..self.count]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items[..self.count]
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.as_slice().get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.as_mut_slice().get_mut(index)
    }

    /// Inserts `item` at `index`, moving later instances along. Returns
    /// whether it was inserted: not when the list is full or `index` lies
    /// past its end.
    pub fn insert(&mut self, index: usize, item: T) -> bool {
        if self.is_full() || index > self.count {
            return false;
        }
        self.items.copy_within(index..self.count, index + 1);
        self.items[index] = item;
        self.count += 1;
        true
    }

    /// Removes the instance at `index`. Returns whether it was removed: the
    /// last remaining instance stays.
    pub fn remove(&mut self, index: usize) -> bool {
        if self.count == 1 || index >= self.count {
            return false;
        }
        self.items.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.items[self.count] = T::default();
        true
    }
}

/// A position in percent of the image mirrored across its vertical center
/// line, for an added instance: a light beside the first goes across the
/// frame from it. A position near the center line moves a quarter of the
/// image aside instead, so the two never coincide.
pub(super) fn mirrored_position(position: [f32; 2], specs: [FloatParamSpec; 2]) -> [f32; 2] {
    const MIN_SEPARATION: f32 = 20.0;
    const SIDE_STEP: f32 = 25.0;
    let [x, y] = position;
    let mirrored = 100.0 - x;
    let x = if (mirrored - x).abs() >= MIN_SEPARATION {
        mirrored
    } else if x + SIDE_STEP <= 100.0 {
        x + SIDE_STEP
    } else {
        x - SIDE_STEP
    };
    [specs[0].clamp(x), specs[1].clamp(y)]
}

impl<T: Copy + Default> Default for EffectInstances<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// The stored layout, written with `#[serde(flatten)]` into the effect's
/// settings.
#[derive(Serialize)]
struct StoredRef<'a, T> {
    #[serde(flatten)]
    first: &'a T,
    #[serde(skip_serializing_if = "<[T]>::is_empty")]
    additional: &'a [T],
}

#[derive(Deserialize)]
struct Stored<T> {
    #[serde(flatten)]
    first: T,
    #[serde(default = "Vec::new")]
    additional: Vec<T>,
}

impl<T: Copy + Default + Serialize> Serialize for EffectInstances<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        StoredRef {
            first: &self.items[0],
            additional: &self.as_slice()[1..],
        }
        .serialize(serializer)
    }
}

impl<'de, T: Copy + Default + Deserialize<'de>> Deserialize<'de> for EffectInstances<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = Stored::<T>::deserialize(deserializer)?;
        let mut instances = Self::new(stored.first);
        for item in stored.additional {
            if !instances.insert(instances.count, item) {
                return Err(serde::de::Error::custom(format!(
                    "an effect holds at most {MAX_EFFECT_INSTANCES} instances"
                )));
            }
        }
        Ok(instances)
    }
}
