//! White balance stored as offsets from each photo's as-shot white balance,
//! as edits saved it up to v1.3.
//!
//! migration: remove in v2.0.0
//!
//! Up to v1.3 an edit stored `temperature` and `tint`: offsets from the
//! as-shot white balance of its photo, so copies and presets moved a shift
//! instead of a white balance. Edits now store a [`WhiteBalance`] as
//! `white_balance`, next to those fields. Reading an edit without it:
//! - zero offsets are [`WhiteBalance::AsShot`];
//! - other offsets become [`WhiteBalance::LegacyOffset`], because only the
//!   photo knows its as-shot white balance. `LoadedRaw::resolve_white_balance`
//!   converts them when the photo is opened, and the open saves the edit
//!   again.
//!
//! Every edit is also written with `temperature` and `tint`, which readers up
//! to v1.3 require and render: zero for as shot, the stored offsets
//! otherwise. Those readers drop `white_balance` when they save, so their
//! offsets are read back as a legacy white balance.
//!
//! To remove: make `ExposureParams::white_balance` a plain
//! `#[serde(default)]` field of a serializable [`WhiteBalance`] without
//! `LegacyOffset` and `older_reader_offsets`, and delete
//! `LoadedRaw::resolve_white_balance`'s legacy branch. Edits still holding
//! offsets then open as shot.

use crate::pipeline::WhiteBalance;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The serialized `white_balance` field.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredWhiteBalance {
    AsShot,
    Custom { kelvin: f32, tint: f32 },
}

/// The white-balance fields of a serialized exposure object.
#[derive(Deserialize, Serialize)]
struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    white_balance: Option<StoredWhiteBalance>,
    #[serde(default)]
    temperature: f32,
    #[serde(default)]
    tint: f32,
}

pub(crate) fn serialize<S: Serializer>(
    white_balance: &WhiteBalance,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let fields = match *white_balance {
        WhiteBalance::AsShot => Fields {
            white_balance: Some(StoredWhiteBalance::AsShot),
            temperature: 0.0,
            tint: 0.0,
        },
        WhiteBalance::Custom {
            kelvin,
            tint,
            older_reader_offsets: [temperature_offset, tint_offset],
        } => Fields {
            white_balance: Some(StoredWhiteBalance::Custom { kelvin, tint }),
            temperature: temperature_offset,
            tint: tint_offset,
        },
        // Written as v1.3 did, so it is read back unchanged.
        WhiteBalance::LegacyOffset { temperature, tint } => Fields {
            white_balance: None,
            temperature,
            tint,
        },
    };
    fields.serialize(serializer)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<WhiteBalance, D::Error> {
    let Fields {
        white_balance,
        temperature,
        tint: tint_offset,
    } = Fields::deserialize(deserializer)?;
    Ok(match white_balance {
        Some(StoredWhiteBalance::AsShot) => WhiteBalance::AsShot,
        Some(StoredWhiteBalance::Custom { kelvin, tint }) => WhiteBalance::Custom {
            kelvin,
            tint,
            older_reader_offsets: [temperature, tint_offset],
        },
        None if temperature == 0.0 && tint_offset == 0.0 => WhiteBalance::AsShot,
        None => WhiteBalance::LegacyOffset {
            temperature,
            tint: tint_offset,
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::pipeline::{ExposureParams, WhiteBalance};
    use serde_json::{json, Value};

    fn exposure_json(white_balance: WhiteBalance) -> Value {
        serde_json::to_value(ExposureParams {
            white_balance,
            ..ExposureParams::default()
        })
        .unwrap()
    }

    fn read(value: Value) -> WhiteBalance {
        serde_json::from_value::<ExposureParams>(value)
            .unwrap()
            .white_balance
    }

    /// An exposure object as v1.3 saved it.
    fn v1_3(temperature: f32, tint: f32) -> Value {
        let mut value = exposure_json(WhiteBalance::AsShot);
        let object = value.as_object_mut().unwrap();
        object.remove("white_balance");
        object.insert("temperature".to_owned(), json!(temperature));
        object.insert("tint".to_owned(), json!(tint));
        value
    }

    #[test]
    fn zero_offsets_from_older_versions_are_as_shot() {
        assert_eq!(read(v1_3(0.0, 0.0)), WhiteBalance::AsShot);
    }

    #[test]
    fn offsets_from_older_versions_are_kept_until_the_photo_is_opened() {
        let legacy = WhiteBalance::LegacyOffset {
            temperature: -42.5,
            tint: 3.0,
        };
        assert_eq!(read(v1_3(-42.5, 3.0)), legacy);
        // Saved again unchanged, the way v1.3 wrote it.
        let saved = exposure_json(legacy);
        assert_eq!(saved, v1_3(-42.5, 3.0));
        assert_eq!(read(saved), legacy);
    }

    #[test]
    fn readers_up_to_v1_3_get_the_offsets_they_render() {
        let as_shot = exposure_json(WhiteBalance::AsShot);
        assert_eq!(as_shot["white_balance"], "as_shot");
        assert_eq!(
            (&as_shot["temperature"], &as_shot["tint"]),
            (&json!(0.0), &json!(0.0))
        );

        let custom = WhiteBalance::Custom {
            kelvin: 3200.0,
            tint: 12.0,
            older_reader_offsets: [-120.0, -2.5],
        };
        let saved = exposure_json(custom);
        assert_eq!(
            saved["white_balance"],
            json!({"custom": {"kelvin": 3200.0, "tint": 12.0}})
        );
        assert_eq!(
            (&saved["temperature"], &saved["tint"]),
            (&json!(-120.0), &json!(-2.5))
        );
        assert_eq!(read(saved), custom);
    }

    #[test]
    fn offsets_saved_by_an_older_version_over_a_newer_edit_are_read_as_legacy() {
        // v1.3 drops `white_balance` and writes the offsets it rendered.
        let mut value = exposure_json(WhiteBalance::Custom {
            kelvin: 3200.0,
            tint: 12.0,
            older_reader_offsets: [-120.0, -2.5],
        });
        value.as_object_mut().unwrap().remove("white_balance");
        value["temperature"] = json!(-80.0);
        assert_eq!(
            read(value),
            WhiteBalance::LegacyOffset {
                temperature: -80.0,
                tint: -2.5
            }
        );
    }
}
