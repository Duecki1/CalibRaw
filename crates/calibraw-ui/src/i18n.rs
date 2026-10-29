//! Interface language: the persisted choice, the active language and string
//! lookup.
//!
//! Strings are looked up by their English text, so English needs no catalog
//! and a string missing from a catalog shows in English. Each other language
//! has a flat JSON object in `crates/calibraw-ui/locales/<code>.json` that maps
//! the English text to its translation. Placeholders are written `{name}` in
//! both and filled by [`tr_fmt`].
//!
//! The active language is process-wide: views read it on the UI thread, and
//! `CalibRawApp` sets it at startup and when the setting changes.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// A language with a catalog, or English.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Language {
    English,
    German,
    French,
    Japanese,
    Hindi,
    SimplifiedChinese,
    TraditionalChinese,
}

impl Language {
    const ALL: [Self; 7] = [
        Self::English,
        Self::German,
        Self::French,
        Self::Japanese,
        Self::Hindi,
        Self::SimplifiedChinese,
        Self::TraditionalChinese,
    ];

    /// The primary BCP 47 subtag matched against the system's languages.
    const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
            Self::French => "fr",
            Self::Japanese => "ja",
            Self::Hindi => "hi",
            Self::SimplifiedChinese | Self::TraditionalChinese => "zh",
        }
    }

    /// The language's own name, shown untranslated so it can be found from
    /// any language.
    pub(crate) const fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "Deutsch",
            Self::French => "Français",
            Self::Japanese => "日本語",
            Self::Hindi => "हिन्दी",
            Self::SimplifiedChinese => "简体中文",
            Self::TraditionalChinese => "繁體中文",
        }
    }

    /// Translations keyed by English text; empty for English. Parsed once.
    fn catalog(self) -> Result<&'static HashMap<String, String>, CatalogError> {
        static ENGLISH: OnceLock<HashMap<String, String>> = OnceLock::new();
        static GERMAN: OnceLock<Result<HashMap<String, String>, CatalogError>> = OnceLock::new();
        static FRENCH: OnceLock<Result<HashMap<String, String>, CatalogError>> = OnceLock::new();
        static JAPANESE: OnceLock<Result<HashMap<String, String>, CatalogError>> = OnceLock::new();
        static HINDI: OnceLock<Result<HashMap<String, String>, CatalogError>> = OnceLock::new();
        static SIMPLIFIED_CHINESE: OnceLock<Result<HashMap<String, String>, CatalogError>> =
            OnceLock::new();
        static TRADITIONAL_CHINESE: OnceLock<Result<HashMap<String, String>, CatalogError>> =
            OnceLock::new();
        match self {
            Self::English => Ok(ENGLISH.get_or_init(HashMap::new)),
            Self::German => GERMAN
                .get_or_init(|| parse_catalog(self, include_str!("../locales/de.json")))
                .as_ref()
                .map_err(Clone::clone),
            Self::French => FRENCH
                .get_or_init(|| parse_catalog(self, include_str!("../locales/fr.json")))
                .as_ref()
                .map_err(Clone::clone),
            Self::Japanese => JAPANESE
                .get_or_init(|| parse_catalog(self, include_str!("../locales/ja.json")))
                .as_ref()
                .map_err(Clone::clone),
            Self::Hindi => HINDI
                .get_or_init(|| parse_catalog(self, include_str!("../locales/hi.json")))
                .as_ref()
                .map_err(Clone::clone),
            Self::SimplifiedChinese => SIMPLIFIED_CHINESE
                .get_or_init(|| parse_catalog(self, include_str!("../locales/zh.json")))
                .as_ref()
                .map_err(Clone::clone),
            Self::TraditionalChinese => TRADITIONAL_CHINESE
                .get_or_init(|| parse_catalog(self, include_str!("../locales/zh-TW.json")))
                .as_ref()
                .map_err(Clone::clone),
        }
    }

    fn matches_system_tag(self, tag: &str) -> bool {
        let mut subtags = tag.split(['-', '_']);
        let primary = subtags.next().unwrap_or_default();
        if !primary.eq_ignore_ascii_case(self.code()) {
            return false;
        }

        if !matches!(self, Self::SimplifiedChinese | Self::TraditionalChinese) {
            return true;
        }

        // An explicit script takes precedence over the region. A bare `zh`
        // keeps its existing Simplified Chinese default.
        let script = subtags.clone().find(|subtag| {
            subtag.eq_ignore_ascii_case("Hans") || subtag.eq_ignore_ascii_case("Hant")
        });
        let traditional = match script {
            Some(script) => script.eq_ignore_ascii_case("Hant"),
            None => subtags.any(|subtag| {
                ["TW", "HK", "MO"]
                    .into_iter()
                    .any(|region| subtag.eq_ignore_ascii_case(region))
            }),
        };
        match self {
            Self::SimplifiedChinese => !traditional,
            Self::TraditionalChinese => traditional,
            _ => true,
        }
    }

    /// The first of the system's preferred languages CalibRaw has, else English.
    fn from_system() -> Self {
        crate::platform::preferred_languages()
            .iter()
            .find_map(|tag| {
                Self::ALL
                    .into_iter()
                    .find(|language| language.matches_system_tag(tag))
            })
            .unwrap_or(Self::English)
    }
}

/// The interface language setting. Serde names are persisted in settings
/// files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LanguageChoice {
    /// Follow the operating system's preferred languages.
    #[default]
    System,
    English,
    German,
    French,
    Japanese,
    Hindi,
    SimplifiedChinese,
    TraditionalChinese,
}

impl LanguageChoice {
    pub(crate) const ALL: [Self; 8] = [
        Self::System,
        Self::English,
        Self::German,
        Self::French,
        Self::Japanese,
        Self::Hindi,
        Self::SimplifiedChinese,
        Self::TraditionalChinese,
    ];

    pub(crate) fn resolve(self) -> Language {
        match self {
            Self::System => Language::from_system(),
            Self::English => Language::English,
            Self::German => Language::German,
            Self::French => Language::French,
            Self::Japanese => Language::Japanese,
            Self::Hindi => Language::Hindi,
            Self::SimplifiedChinese => Language::SimplifiedChinese,
            Self::TraditionalChinese => Language::TraditionalChinese,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => tr("System default"),
            Self::English => Language::English.native_name(),
            Self::German => Language::German.native_name(),
            Self::French => Language::French.native_name(),
            Self::Japanese => Language::Japanese.native_name(),
            Self::Hindi => Language::Hindi.native_name(),
            Self::SimplifiedChinese => Language::SimplifiedChinese.native_name(),
            Self::TraditionalChinese => Language::TraditionalChinese.native_name(),
        }
    }
}

/// `Language as u8`. English until startup installs the saved choice, so
/// code that runs without settings (tests, fatal start-up messages) is English.
static ACTIVE: AtomicU8 = AtomicU8::new(Language::English as u8);

/// Makes `language` the one every later lookup uses. A language whose
/// catalog cannot be read leaves English active.
pub(crate) fn set_active(language: Language) -> Result<(), CatalogError> {
    // Parsed here, not in the middle of the next frame.
    let result = language.catalog().map(|_| language);
    ACTIVE.store(
        result
            .as_ref()
            .map_or(Language::English, |language| *language) as u8,
        Ordering::Relaxed,
    );
    result.map(|_| ())
}

fn active() -> Language {
    let value = ACTIVE.load(Ordering::Relaxed);
    Language::ALL
        .into_iter()
        .find(|language| *language as u8 == value)
        .unwrap_or(Language::English)
}

pub(crate) fn active_uses_traditional_chinese() -> bool {
    active() == Language::TraditionalChinese
}

/// `english` in the active language.
pub(crate) fn tr(english: &'static str) -> &'static str {
    // `set_active` makes only languages with a readable catalog active.
    active()
        .catalog()
        .ok()
        .and_then(|catalog| catalog.get(english))
        .map_or(english, String::as_str)
}

/// `english` in the active language with each `{name}` replaced by the value
/// paired with `name` in `args`.
pub(crate) fn tr_fmt(english: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut text = tr(english).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

/// A bundled catalog that is not a JSON object of strings: a packaging
/// mistake, reported instead of stopping the app.
#[derive(Clone, Debug)]
pub(crate) struct CatalogError {
    language: &'static str,
    message: String,
}

impl Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The {} translation could not be read, so English is shown: {}",
            self.language, self.message
        )
    }
}

impl std::error::Error for CatalogError {}

fn parse_catalog(language: Language, json: &str) -> Result<HashMap<String, String>, CatalogError> {
    serde_json::from_str(json).map_err(|error| CatalogError {
        language: language.native_name(),
        message: error.to_string(),
    })
}
