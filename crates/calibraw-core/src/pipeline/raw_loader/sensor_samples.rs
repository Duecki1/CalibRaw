//! The sensor samples of a decoded RAW.

use std::ops::Deref;
use std::sync::Arc;

/// One value per photosite, unchanged once the RAW is decoded. Clones share
/// the samples: the same decode shown with another camera profile
/// (`LoadedRaw::with_profile_color`) does not copy them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SensorSamples(Arc<Vec<u16>>);

impl SensorSamples {
    /// The samples to write, copied first when another view shares them.
    pub fn make_mut(&mut self) -> &mut Vec<u16> {
        Arc::make_mut(&mut self.0)
    }
}

impl Deref for SensorSamples {
    type Target = [u16];

    fn deref(&self) -> &[u16] {
        &self.0
    }
}

impl From<Vec<u16>> for SensorSamples {
    fn from(samples: Vec<u16>) -> Self {
        Self(Arc::new(samples))
    }
}

impl FromIterator<u16> for SensorSamples {
    fn from_iter<I: IntoIterator<Item = u16>>(samples: I) -> Self {
        Self::from(samples.into_iter().collect::<Vec<_>>())
    }
}
