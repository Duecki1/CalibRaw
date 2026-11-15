//! A photo's Camera match: off, on with a look fitted to its own embedded
//! preview, or on and waiting for that fit.

use super::CameraLook;
use serde::{Deserialize, Serialize};

/// A photo's Camera match. A look is fitted to one photo's own embedded
/// preview, so it never moves to another photo: copy and paste and reset
/// move only whether Camera match is on and its amount ([`Self::component`],
/// [`Self::with_amount`]), and a photo that has no look of its own then waits
/// for a fit ([`Self::Pending`]), which runs when it is open.
///
/// Saved in a sidecar as the fields of [`CameraMatchFields`]: a fitted look
/// as `camera_look`, as before this state existed, a pending fit as
/// `camera_match_pending`, nothing when off.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "CameraMatchFields", into = "CameraMatchFields")]
pub enum CameraMatch {
    #[default]
    Off,
    /// On at `amount` (0–1), without a look yet: it is fitted when the photo
    /// is open, and the photo renders without one until then.
    Pending { amount: f32 },
    /// On, with the look fitted to this photo and the amount it applies.
    Fitted(CameraLook),
}

impl CameraMatch {
    /// On at the default amount, waiting for a fit.
    pub const PENDING_DEFAULT: Self = Self::Pending {
        amount: CameraLook::DEFAULT_AMOUNT,
    };

    /// The fitted look, which renders.
    pub fn look(&self) -> Option<&CameraLook> {
        match self {
            Self::Fitted(look) => Some(look),
            Self::Off | Self::Pending { .. } => None,
        }
    }

    pub fn is_off(&self) -> bool {
        matches!(self, Self::Off)
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    /// What copy and paste moves: the amount while on, `None` while off.
    pub fn component(&self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Pending { amount } => Some(*amount),
            Self::Fitted(look) => Some(look.amount()),
        }
    }

    /// On at `amount`: this photo's own look at it, or a fit to come when it
    /// has none.
    pub fn with_amount(self, amount: f32) -> Self {
        match self {
            Self::Fitted(mut look) => {
                look.set_amount(amount);
                Self::Fitted(look)
            }
            Self::Off | Self::Pending { .. } => Self::Pending {
                amount: sanitized_amount(amount),
            },
        }
    }

    /// The state a finished fit of this photo leaves: a pending match takes
    /// `fitted` at its amount, or turns off when the photo had no look to
    /// match (`None`). Other states stay.
    pub fn resolved(self, fitted: Option<&CameraLook>) -> Self {
        match (self, fitted) {
            (Self::Pending { amount }, Some(look)) => {
                let mut look = look.clone();
                look.set_amount(amount);
                Self::Fitted(look)
            }
            (Self::Pending { .. }, None) => Self::Off,
            (state, _) => state,
        }
    }
}

fn sanitized_amount(amount: f32) -> f32 {
    if amount.is_finite() {
        amount.clamp(0.0, 1.0)
    } else {
        CameraLook::DEFAULT_AMOUNT
    }
}

/// How a sidecar stores a [`CameraMatch`], flattened into the edit state.
#[derive(Clone, Default, Serialize, Deserialize)]
struct CameraMatchFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    camera_look: Option<CameraLook>,
    /// The amount of a pending fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    camera_match_pending: Option<f32>,
}

impl From<CameraMatchFields> for CameraMatch {
    fn from(fields: CameraMatchFields) -> Self {
        match (fields.camera_look, fields.camera_match_pending) {
            // A file holding both was not written by this version; the
            // fitted look is the one that renders.
            (Some(look), _) => Self::Fitted(look),
            (None, Some(amount)) => Self::Pending {
                amount: sanitized_amount(amount),
            },
            (None, None) => Self::Off,
        }
    }
}

impl From<CameraMatch> for CameraMatchFields {
    fn from(state: CameraMatch) -> Self {
        match state {
            CameraMatch::Off => Self::default(),
            CameraMatch::Pending { amount } => Self {
                camera_look: None,
                camera_match_pending: Some(amount),
            },
            CameraMatch::Fitted(look) => Self {
                camera_look: Some(look),
                camera_match_pending: None,
            },
        }
    }
}
