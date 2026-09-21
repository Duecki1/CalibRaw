use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct WhiteBalancePreset {
    pub name: String,
    pub coefficients: [f32; 4],
}

#[derive(Deserialize)]
struct Database {
    wb_presets: Vec<Maker>,
}

#[derive(Deserialize)]
struct Maker {
    maker: String,
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    model: String,
    presets: Vec<Preset>,
}

#[derive(Deserialize)]
struct Preset {
    name: String,
    channels: [f32; 4],
}

fn parse_database(source: &str) -> Result<Database, String> {
    let database: Database = serde_json::from_str(source)
        .map_err(|error| format!("invalid white-balance preset database: {error}"))?;
    let mut seen = HashSet::new();
    for maker in &database.wb_presets {
        for model in &maker.models {
            for preset in &model.presets {
                let key = (&maker.maker, &model.model, &preset.name);
                if !seen.insert(key) {
                    return Err(format!(
                        "duplicate white-balance preset for {} {}: {}",
                        maker.maker, model.model, preset.name
                    ));
                }
            }
        }
    }
    Ok(database)
}

fn normalized_camera_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

pub fn for_camera(camera_maker: &str, camera_model: &str) -> Vec<WhiteBalancePreset> {
    type Catalog = HashMap<(String, String), Vec<WhiteBalancePreset>>;
    static CATALOG: OnceLock<Option<Catalog>> = OnceLock::new();
    let Some(catalog) = CATALOG
        .get_or_init(|| {
            let database = parse_database(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../data/wb_presets.json"
            )))
            .ok()?;
            Some(
                database
                    .wb_presets
                    .into_iter()
                    .flat_map(|maker| {
                        let maker_key = normalized_camera_name(&maker.maker);
                        maker.models.into_iter().map(move |model| {
                            let key = (maker_key.clone(), normalized_camera_name(&model.model));
                            let presets = model
                                .presets
                                .into_iter()
                                .filter(|preset| {
                                    preset.channels[..3]
                                        .iter()
                                        .all(|value| value.is_finite() && *value > 0.0)
                                })
                                .map(|preset| WhiteBalancePreset {
                                    name: preset.name,
                                    coefficients: preset.channels,
                                })
                                .collect();
                            (key, presets)
                        })
                    })
                    .collect(),
            )
        })
        .as_ref()
    else {
        return Vec::new();
    };
    catalog
        .get(&(
            normalized_camera_name(camera_maker),
            normalized_camera_name(camera_model),
        ))
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{for_camera, parse_database};

    fn preset_coefficients(maker: &str, model: &str, name: &str) -> Vec<[f32; 4]> {
        for_camera(maker, model)
            .into_iter()
            .filter(|preset| preset.name == name)
            .map(|preset| preset.coefficients)
            .collect()
    }

    #[test]
    fn bundled_darktable_database_matches_camera_names_robustly() {
        let presets = for_camera("SONY", "ILCE-7CM2");
        assert!(presets.iter().any(|preset| preset.name == "Daylight"));
        assert!(presets.iter().any(|preset| preset.name == "8500K"));
    }

    #[test]
    fn bundled_database_has_unique_camera_preset_names() {
        parse_database(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/wb_presets.json"
        )))
        .expect("bundled white-balance presets must not contain duplicate model/preset names");
    }

    #[test]
    fn duplicate_camera_preset_names_are_rejected() {
        let duplicate = r#"{
            "wb_presets": [{
                "maker": "Panasonic",
                "models": [{
                    "model": "DC-S9",
                    "presets": [
                        {"name": "Shade", "channels": [2.3, 1.0, 1.5, 0.0]},
                        {"name": "Shade", "channels": [2.4, 1.0, 1.6, 0.0]}
                    ]
                }]
            }]
        }"#;

        let error = match parse_database(duplicate) {
            Ok(_) => panic!("duplicate preset names must fail"),
            Err(error) => error,
        };
        assert!(error.contains("Panasonic DC-S9: Shade"));
    }

    #[test]
    fn authoritative_panasonic_zero_tuning_coefficients_are_retained() {
        // Upstream contains conflicting zero-tuning rows for these presets. Keep the
        // values that are continuous with the corresponding fine-tuning series.
        assert_eq!(
            preset_coefficients("Panasonic", "DC-S9", "Shade"),
            vec![[2.33984375, 1.0, 1.5390625, 0.0]]
        );
        assert_eq!(
            preset_coefficients("Panasonic", "DC-S9", "Cloudy"),
            vec![[2.21484375, 1.0, 1.61328125, 0.0]]
        );
        assert_eq!(
            preset_coefficients("Panasonic", "DMC-G2", "Cloudy"),
            vec![[2.030418, 1.0, 1.326996, 0.0]]
        );
    }
}
