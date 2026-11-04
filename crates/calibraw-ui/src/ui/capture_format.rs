//! How capture settings read as text, shared by the Info panel, the library's
//! hover card and the Develop toolbar.

/// Between the fields of a one-line metadata summary.
pub(crate) const FIELD_SEPARATOR: &str = " · ";

/// Metadata values are zero or non-finite when the file does not record them.
fn known(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

pub(crate) fn format_aperture(value: f32) -> Option<String> {
    known(value).map(|value| format!("f/{value:.1}"))
}

pub(crate) fn format_shutter_speed(seconds: f32) -> Option<String> {
    known(seconds).map(|seconds| {
        if seconds < 0.5 {
            let denominator = (1.0 / seconds).round().max(1.0);
            format!("1/{denominator:.0} s")
        } else if (seconds - seconds.round()).abs() < 0.05 {
            format!("{seconds:.0} s")
        } else {
            format!("{seconds:.1} s")
        }
    })
}

pub(crate) fn format_iso(value: f32) -> Option<String> {
    known(value).map(|value| {
        if (value - value.round()).abs() < 0.05 {
            format!("ISO {value:.0}")
        } else {
            format!("ISO {value:.1}")
        }
    })
}

pub(crate) fn format_focal_length(value: f32) -> Option<String> {
    known(value).map(|value| {
        if (value - value.round()).abs() < 0.05 {
            format!("{value:.0} mm")
        } else {
            format!("{value:.1} mm")
        }
    })
}
