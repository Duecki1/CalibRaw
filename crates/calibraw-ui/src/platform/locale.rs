//! The operating system's preferred interface languages.

/// The user's preferred languages as BCP 47 tags (`de-DE`, `en`), most
/// preferred first; empty when the OS reports none.
pub(crate) fn preferred_languages() -> Vec<String> {
    sys_locale::get_locales().collect()
}
