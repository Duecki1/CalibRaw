//! Matching the camera and lens of a RAW against the Lensfun database.

use super::*;
use std::collections::HashMap;

pub(super) fn find_camera(database: &Database, make: &str, model: &str) -> Option<*const lfCamera> {
    if model.trim().is_empty() {
        return None;
    }
    let make = CString::new(make).ok()?;
    let model = CString::new(model).ok()?;
    let exact = OwnedPointerList {
        pointer: unsafe { lf_db_find_cameras(database.0, make.as_ptr(), model.as_ptr()) },
    };
    if let Some(camera) = exact.first() {
        return Some(camera);
    }
    let loose = OwnedPointerList {
        pointer: unsafe {
            lf_db_find_cameras_ext(
                database.0,
                make.as_ptr(),
                model.as_ptr(),
                LF_SEARCH_LOOSE | LF_SEARCH_SORT_AND_UNIQUIFY,
            )
        },
    };
    loose.first()
}

pub(super) fn compatible_lenses(database: &Database, camera: *const lfCamera) -> Vec<LensfunLens> {
    let empty = CString::new("").expect("an empty string contains no NUL byte");
    let list = OwnedPointerList {
        pointer: unsafe {
            lf_db_find_lenses(
                database.0,
                camera,
                ptr::null(),
                empty.as_ptr(),
                LF_SEARCH_SORT_AND_UNIQUIFY,
            )
        },
    };
    let values = list
        .values()
        .into_iter()
        .filter_map(lens_name)
        .collect::<Vec<_>>();
    if !values.is_empty() {
        return values;
    }

    pointer_list(unsafe { lf_db_get_lenses(database.0) })
        .into_iter()
        .filter_map(lens_name)
        .filter(|lens| find_lens(database, Some(camera), lens).is_some())
        .collect()
}

pub(super) fn all_lenses(database: &Database) -> Vec<LensfunLens> {
    pointer_list(unsafe { lf_db_get_lenses(database.0) })
        .into_iter()
        .filter_map(lens_name)
        .collect()
}

pub(super) fn sort_and_deduplicate_lenses(lenses: &mut Vec<LensfunLens>) {
    lenses.sort_by(|left, right| {
        left.maker
            .to_lowercase()
            .cmp(&right.maker.to_lowercase())
            .then_with(|| left.model.to_lowercase().cmp(&right.model.to_lowercase()))
    });
    lenses.dedup_by(|left, right| {
        left.maker.eq_ignore_ascii_case(&right.maker)
            && left.model.eq_ignore_ascii_case(&right.model)
    });
}

/// Picks the profile for the RAW's lens. `catalog` lists the profiles that
/// fit the camera, or every profile when the camera is unknown.
pub(super) fn find_auto_lens(
    database: &Database,
    camera: Option<*const lfCamera>,
    raw: &LoadedRaw,
    catalog: &[LensfunLens],
) -> Option<LensfunLens> {
    if raw.lens_model.trim().is_empty() {
        return None;
    }

    let mut candidates = if let Some(camera) = camera {
        let maker = if raw.lens_make.trim().is_empty() {
            None
        } else {
            CString::new(raw.lens_make.trim()).ok()
        };
        let model = CString::new(raw.lens_model.trim()).ok()?;
        let list = OwnedPointerList {
            pointer: unsafe {
                lf_db_find_lenses(
                    database.0,
                    camera,
                    maker.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
                    model.as_ptr(),
                    LF_SEARCH_LOOSE | LF_SEARCH_SORT_AND_UNIQUIFY,
                )
            },
        };
        list.values()
            .into_iter()
            .filter_map(lens_name)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    if candidates.is_empty() {
        candidates = catalog.to_vec();
    }
    sort_and_deduplicate_lenses(&mut candidates);

    let names = ProfileNames::load(database);
    let exact = candidates
        .iter()
        .filter(|candidate| {
            names
                .models(candidate)
                .into_iter()
                .any(|model| lens_metadata_is_exact(raw, &candidate.maker, model))
        })
        .collect::<Vec<_>>();
    if let Some(&first) = exact.first() {
        if exact.iter().all(|other| same_lens(first, other)) {
            return Some(first.clone());
        }
    }

    if camera.is_some()
        && candidates.len() == 1
        && profile_supports_capture(database, camera, raw, &candidates[0])
    {
        return candidates.into_iter().next();
    }

    ranked_match(database, camera, raw, &names, candidates)
        .or_else(|| optics_match(database, camera, raw, &names, catalog))
}

/// Every model name of each profile, by the name `lens_name` shows. Lensfun
/// master shows many profiles by an `en` manufacturer name, while EXIF often
/// reports the default one: `E 70-300mm F4.5-6.3 A047` is shown as
/// `Tamron 70-300mm F4.5-6.3 Di III RXD`.
struct ProfileNames(HashMap<(String, String), Vec<String>>);

impl ProfileNames {
    fn load(database: &Database) -> Self {
        let mut names = HashMap::<_, Vec<String>>::new();
        for pointer in pointer_list(unsafe { lf_db_get_lenses(database.0) }) {
            let Some(lens) = lens_name(pointer) else {
                continue;
            };
            // SAFETY: `pointer_list` returns only non-null lenses the database owns.
            let variants = multilingual_variants(unsafe { (*pointer).Model });
            let models = names.entry(profile_key(&lens)).or_default();
            for variant in variants {
                if !models.contains(&variant) {
                    models.push(variant);
                }
            }
        }
        Self(names)
    }

    /// The model names of `candidate`'s profile, its shown name first.
    fn models<'a>(&'a self, candidate: &'a LensfunLens) -> Vec<&'a str> {
        let mut models = vec![candidate.model.as_str()];
        if let Some(variants) = self.0.get(&profile_key(candidate)) {
            models.extend(
                variants
                    .iter()
                    .map(String::as_str)
                    .filter(|variant| *variant != candidate.model),
            );
        }
        models
    }
}

fn profile_key(lens: &LensfunLens) -> (String, String) {
    (lens.maker.to_lowercase(), lens.model.to_lowercase())
}

/// Whether two profiles name the same lens. Lensfun lists some lenses twice,
/// such as `Tamron 70-300mm F4.5-6.3 Di III RXD` and `… F/4.5-6.3 …`; the
/// matchers then take the first instead of treating them as rivals.
fn same_lens(left: &LensfunLens, right: &LensfunLens) -> bool {
    canonical_text(&left.maker) == canonical_text(&right.maker)
        && canonical_text(&left.model) == canonical_text(&right.model)
}

fn ranked_match(
    database: &Database,
    camera: Option<*const lfCamera>,
    raw: &LoadedRaw,
    names: &ProfileNames,
    candidates: Vec<LensfunLens>,
) -> Option<LensfunLens> {
    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| {
            if !maker_is_compatible(&raw.lens_make, &raw.lens_model, &candidate.maker)
                || !profile_supports_capture(database, camera, raw, &candidate)
            {
                return None;
            }
            let score = names
                .models(&candidate)
                .into_iter()
                .filter_map(|model| automatic_match_score(raw, &candidate.maker, model))
                .max_by(f32::total_cmp)?;
            Some((score, candidate))
        })
        .collect::<Vec<_>>();
    // A stable sort keeps catalog order among equal scores.
    ranked.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (best_score, best) = ranked.first()?;
    if *best_score < 0.90 {
        return None;
    }
    let rival = ranked
        .iter()
        .skip(1)
        .find(|(_, other)| !same_lens(best, other));
    if let Some((runner_up, _)) = rival {
        if *best_score - *runner_up < 0.08 {
            return None;
        }
    }
    Some(best.clone())
}

/// Matches EXIF names that abbreviate the Lensfun name, such as
/// `OLYMPUS M.12-40mm F2.8` for `Olympus M.Zuiko Digital ED 12-40mm f/2.8 Pro`
/// or `LEICA DG SUMMILUX 25/F1.4` for `Leica DG Summilux 25mm f/1.4 Asph.`.
/// Focal range, aperture and version numeral must agree, and every other word
/// of the EXIF name must appear in the profile's maker or model. Of the
/// profiles left, the one adding the fewest words wins, so a plain
/// `M.14-42mm F3.5-5.6` does not pick the `EZ` or `L` variant; a tie between
/// different lenses matches nothing. Every name of a profile is tried.
fn optics_match(
    database: &Database,
    camera: Option<*const lfCamera>,
    raw: &LoadedRaw,
    names: &ProfileNames,
    catalog: &[LensfunLens],
) -> Option<LensfunLens> {
    let optics = lens_optics(&raw.lens_model)?;
    let raw_maker_words = tokenized(&raw.lens_make);
    let raw_words = lens_words(&raw.lens_model)
        .into_iter()
        .filter(|word| !raw_maker_words.contains(word))
        .collect::<Vec<_>>();
    let raw_versions = version_numerals(&raw_words);
    let raw_names = raw_words
        .iter()
        .filter(|word| !is_version_numeral(word))
        .collect::<Vec<_>>();
    // Optics and a version numeral alone do not identify a lens.
    if raw_names.is_empty() {
        return None;
    }

    // The words `model` adds to the EXIF name, when it names the same optics.
    let added_words = |maker: &str, model: &str| {
        if lens_optics(model) != Some(optics) {
            return None;
        }
        let words = lens_words(model);
        if version_numerals(&words) != raw_versions {
            return None;
        }
        let maker_words = lens_maker_words(maker);
        if !raw_names
            .iter()
            .all(|name| words.contains(name) || maker_words.contains(name))
        {
            return None;
        }
        Some(
            words
                .iter()
                .filter(|word| !raw_names.contains(word) && !is_version_numeral(word))
                .count(),
        )
    };

    let mut best: Option<(usize, &LensfunLens)> = None;
    let mut tied = false;
    for candidate in catalog {
        if !maker_is_compatible(&raw.lens_make, &raw.lens_model, &candidate.maker) {
            continue;
        }
        let Some(added) = names
            .models(candidate)
            .into_iter()
            .filter_map(|model| added_words(&candidate.maker, model))
            .min()
        else {
            continue;
        };
        if !profile_supports_capture(database, camera, raw, candidate) {
            continue;
        }
        match best {
            Some((fewest, _)) if added > fewest => {}
            Some((fewest, first)) if added == fewest => {
                tied |= !same_lens(first, candidate);
            }
            _ => {
                best = Some((added, candidate));
                tied = false;
            }
        }
    }
    if tied {
        return None;
    }
    best.map(|(_, candidate)| candidate.clone())
}

/// Focal range in millimetres and maximum aperture as f-numbers, read from a
/// lens name; a prime or a constant-aperture zoom repeats its single value.
#[derive(Clone, Copy, PartialEq)]
struct LensOptics {
    focal: [f32; 2],
    aperture: [f32; 2],
}

/// Reads `12-40mm f/2.8`, `12-40mm F2.8` and Panasonic's `25/F1.4`. Names
/// without both values, such as `Summilux-TL 1:1.4/35`, give `None`.
fn lens_optics(model: &str) -> Option<LensOptics> {
    let text = model.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut focal = None;
    let mut aperture = None;
    let mut index = 0;
    while index < bytes.len() {
        let starts_word = index == 0 || !bytes[index - 1].is_ascii_alphabetic();
        if aperture.is_none() && bytes[index] == b'f' && starts_word {
            let start = index + 1 + usize::from(bytes.get(index + 1) == Some(&b'/'));
            if let Some((range, end)) = number_range(bytes, start) {
                aperture = Some(range);
                index = end;
                continue;
            }
        }
        if focal.is_none() {
            if let Some((range, end)) = number_range(bytes, index) {
                if bytes[end..].starts_with(b"mm") || bytes[end..].starts_with(b"/f") {
                    focal = Some(range);
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    Some(LensOptics {
        focal: focal?,
        aperture: aperture?,
    })
}

/// A decimal number or a `low-high` range starting at `start`, and the index
/// after it.
fn number_range(bytes: &[u8], start: usize) -> Option<([f32; 2], usize)> {
    let (low, end) = decimal(bytes, start)?;
    if bytes.get(end) == Some(&b'-') {
        if let Some((high, end)) = decimal(bytes, end + 1) {
            return Some(([low, high], end));
        }
    }
    Some(([low, low], end))
}

fn decimal(bytes: &[u8], start: usize) -> Option<(f32, usize)> {
    let digits_from = |from: usize| {
        from + bytes[from.min(bytes.len())..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let mut end = digits_from(start);
    if end == start {
        return None;
    }
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end = digits_from(end + 1);
    }
    let value = std::str::from_utf8(&bytes[start..end]).ok()?.parse().ok()?;
    Some((value, end))
}

/// The words of a lens name without numbers, so `M.12-40mm F2.8 Pro` gives
/// `m` and `pro`.
fn lens_words(model: &str) -> Vec<String> {
    tokenized(model)
        .into_iter()
        .filter(|token| !token.chars().any(|character| character.is_ascii_digit()))
        .filter(|token| !matches!(token.as_str(), "f" | "mm" | "lens"))
        .collect()
}

/// Lensfun files lenses that OM Digital Solutions names `OM …` in EXIF under
/// Olympus, and its own lenses under OM Digital Solutions.
fn lens_maker_words(maker: &str) -> Vec<String> {
    let mut words = tokenized(maker);
    if words.iter().any(|word| word == "olympus" || word == "om") {
        words.extend(["olympus".to_owned(), "om".to_owned()]);
    }
    words
}

fn is_version_numeral(word: &str) -> bool {
    matches!(word, "ii" | "iii" | "iv")
}

fn version_numerals(words: &[String]) -> Vec<&str> {
    let mut numerals = words
        .iter()
        .map(String::as_str)
        .filter(|word| is_version_numeral(word))
        .collect::<Vec<_>>();
    numerals.sort_unstable();
    numerals.dedup();
    numerals
}

/// Whether `model`, one name of a profile by `maker`, is the RAW's lens name.
fn lens_metadata_is_exact(raw: &LoadedRaw, maker: &str, model: &str) -> bool {
    if !maker_is_compatible(&raw.lens_make, &raw.lens_model, maker) {
        return false;
    }
    canonical_lens_model(&raw.lens_model, &[raw.lens_make.as_str(), maker])
        == canonical_lens_model(model, &[maker, raw.lens_make.as_str()])
}

/// How well `model`, one name of a profile by `maker`, matches the RAW's lens
/// name, from 0.78 to 1; `None` when it does not.
fn automatic_match_score(raw: &LoadedRaw, maker: &str, model: &str) -> Option<f32> {
    let raw_model = canonical_lens_model(&raw.lens_model, &[raw.lens_make.as_str(), maker]);
    let candidate_model = canonical_lens_model(model, &[maker, raw.lens_make.as_str()]);
    if raw_model.is_empty() || candidate_model.is_empty() {
        return None;
    }
    if raw_model == candidate_model {
        return Some(1.0);
    }

    let raw_codes = lens_model_codes(&raw.lens_model);
    let candidate_codes = lens_model_codes(model);
    if raw_codes
        .iter()
        .any(|code| candidate_codes.iter().any(|other| other == code))
    {
        return Some(0.995);
    }

    if raw_model.len().min(candidate_model.len()) >= 8
        && (raw_model.contains(&candidate_model) || candidate_model.contains(&raw_model))
    {
        return Some(0.96);
    }

    let raw_tokens = lens_tokens(&raw.lens_model, &raw.lens_make, maker);
    let candidate_tokens = lens_tokens(model, maker, &raw.lens_make);
    if raw_tokens.is_empty() || candidate_tokens.is_empty() {
        return None;
    }
    let numeric_tokens = raw_tokens
        .iter()
        .filter(|token| token.chars().any(|character| character.is_ascii_digit()))
        .collect::<Vec<_>>();
    if !numeric_tokens.is_empty()
        && !numeric_tokens
            .iter()
            .all(|token| candidate_tokens.iter().any(|other| other == *token))
    {
        return None;
    }
    let shared = raw_tokens
        .iter()
        .filter(|token| candidate_tokens.iter().any(|other| other == *token))
        .count();
    let similarity = shared as f32 / raw_tokens.len().max(candidate_tokens.len()) as f32;
    (similarity >= 0.75).then_some(0.78 + similarity * 0.18)
}

fn profile_supports_capture(
    database: &Database,
    camera: Option<*const lfCamera>,
    raw: &LoadedRaw,
    candidate: &LensfunLens,
) -> bool {
    if camera.is_none() {
        return true;
    }
    let Some(focal) = positive(raw.focal_length) else {
        return true;
    };
    let Some(lens) = find_lens(database, camera, candidate) else {
        return true;
    };
    let Some(fields) = lens_fields(lens) else {
        return true;
    };
    let (min_focal, max_focal) = (fields.min_focal, fields.max_focal);
    if !min_focal.is_finite() || !max_focal.is_finite() || min_focal <= 0.0 || max_focal < min_focal
    {
        return true;
    }
    let tolerance = (0.03 * max_focal).max(0.75);
    focal >= min_focal - tolerance && focal <= max_focal + tolerance
}

fn maker_is_compatible(raw_maker: &str, raw_model: &str, candidate_maker: &str) -> bool {
    let raw = canonical_text(raw_maker);
    if raw.is_empty() {
        return true;
    }
    let candidate = canonical_text(candidate_maker);
    if candidate.is_empty() {
        return true;
    }
    raw == candidate
        || raw.contains(&candidate)
        || candidate.contains(&raw)
        || canonical_text(raw_model).starts_with(&candidate)
}

fn canonical_lens_model(model: &str, makers: &[&str]) -> String {
    let mut canonical = canonical_text(model);
    for maker in makers {
        let maker = canonical_text(maker);
        if !maker.is_empty() && canonical.starts_with(&maker) && canonical.len() > maker.len() {
            canonical = canonical[maker.len()..].to_owned();
        }
    }
    canonical
}

fn canonical_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn lens_model_codes(model: &str) -> Vec<String> {
    let mut codes = tokenized(model)
        .into_iter()
        .filter(|token| token.len() >= 3)
        .filter(|token| {
            token
                .chars()
                .any(|character| character.is_ascii_alphabetic())
        })
        .filter(|token| token.chars().any(|character| character.is_ascii_digit()))
        .filter(|token| !token.ends_with("mm"))
        .filter(|token| {
            !token.strip_prefix('f').is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
            })
        })
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn lens_tokens(model: &str, primary_maker: &str, alternate_maker: &str) -> Vec<String> {
    let maker_tokens = tokenized(primary_maker)
        .into_iter()
        .chain(tokenized(alternate_maker))
        .collect::<Vec<_>>();
    let mut tokens = tokenized(model)
        .into_iter()
        .filter(|token| token != "lens")
        .filter(|token| !maker_tokens.iter().any(|maker| maker == token))
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn tokenized(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut token = String::new();
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            token.push(character);
        } else if !token.is_empty() {
            result.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        result.push(token);
    }
    result
}

pub(super) fn find_lens(
    database: &Database,
    camera: Option<*const lfCamera>,
    selection: &LensfunLens,
) -> Option<*const lfLens> {
    if let Some(camera) = camera {
        let maker = CString::new(selection.maker.as_str()).ok()?;
        let model = CString::new(selection.model.as_str()).ok()?;
        let list = OwnedPointerList {
            pointer: unsafe {
                lf_db_find_lenses(
                    database.0,
                    camera,
                    maker.as_ptr(),
                    model.as_ptr(),
                    LF_SEARCH_SORT_AND_UNIQUIFY,
                )
            },
        };
        if let Some(lens) = list.first() {
            return Some(lens);
        }
    }

    find_lens_in_database(database, selection)
}

/// Compares every name a profile has, so a selection saved before the profile
/// gained a translated name still finds it: Lensfun master gives many lenses
/// an `en` manufacturer name, which `lens_name` now returns.
fn find_lens_in_database(database: &Database, selection: &LensfunLens) -> Option<*const lfLens> {
    let has_name = |names: *mut c_char, wanted: &str| {
        multilingual_variants(names)
            .iter()
            .any(|name| name.eq_ignore_ascii_case(wanted))
    };
    pointer_list(unsafe { lf_db_get_lenses(database.0) })
        .into_iter()
        .find(|&pointer| {
            // SAFETY: `pointer_list` returns only non-null lenses the database owns.
            let lens = unsafe { &*pointer };
            has_name(lens.Model, &selection.model)
                && (selection.maker.trim().is_empty() || has_name(lens.Maker, &selection.maker))
        })
}

#[derive(Clone, Copy)]
pub(super) struct LensFields {
    pub(super) crop_factor: f32,
    pub(super) min_focal: f32,
    pub(super) max_focal: f32,
}

pub(super) fn camera_name(pointer: *const lfCamera) -> Option<(String, String)> {
    if pointer.is_null() {
        return None;
    }
    let camera = unsafe { &*pointer };
    Some((
        multilingual_string(camera.Maker),
        multilingual_string(camera.Model),
    ))
}

pub(super) fn camera_crop_factor(pointer: *const lfCamera) -> Option<f32> {
    if pointer.is_null() {
        return None;
    }
    Some(unsafe { (*pointer).CropFactor })
}

pub(super) fn lens_fields(pointer: *const lfLens) -> Option<LensFields> {
    if pointer.is_null() {
        return None;
    }
    let lens = unsafe { &*pointer };
    Some(LensFields {
        crop_factor: lens.CropFactor,
        min_focal: lens.MinFocal,
        max_focal: lens.MaxFocal,
    })
}

pub(super) fn lens_name(pointer: *const lfLens) -> Option<LensfunLens> {
    if pointer.is_null() {
        return None;
    }
    let lens = unsafe { &*pointer };
    let maker = multilingual_string(lens.Maker);
    let model = multilingual_string(lens.Model);
    if maker.is_empty() && model.is_empty() {
        None
    } else {
        Some(LensfunLens {
            maker,
            model,
            ..LensfunLens::default()
        })
    }
}

pub(super) fn pointer_list<T>(pointer: *const *const T) -> Vec<*const T> {
    if pointer.is_null() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut index = 0usize;
    loop {
        let value = unsafe { *pointer.add(index) };
        if value.is_null() {
            break;
        }
        result.push(value);
        index += 1;
    }
    result
}

/// The default string and every translation of a Lensfun multi-language
/// string, trimmed.
fn multilingual_variants(pointer: *mut c_char) -> Vec<String> {
    if pointer.is_null() {
        return Vec::new();
    }
    let mut variants = Vec::new();
    // SAFETY: a Lensfun multi-language string is its default string followed
    // by `language`, `translation` string pairs and ends with an empty string;
    // the walk stops there as `lf_mlstr_get` does, so it stays inside it.
    unsafe {
        let default = CStr::from_ptr(pointer);
        variants.push(default);
        let mut cursor = pointer.add(default.to_bytes_with_nul().len());
        while *cursor != 0 {
            let language = CStr::from_ptr(cursor);
            let translation = CStr::from_ptr(cursor.add(language.to_bytes_with_nul().len()));
            if translation.is_empty() {
                break;
            }
            variants.push(translation);
            cursor = translation
                .as_ptr()
                .add(translation.to_bytes_with_nul().len())
                .cast_mut();
        }
    }
    variants
        .into_iter()
        .map(|variant| variant.to_string_lossy().trim().to_owned())
        .collect()
}

fn multilingual_string(pointer: *mut c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let localized = unsafe { lf_mlstr_get(pointer) };
    if localized.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(localized) }
            .to_string_lossy()
            .trim()
            .to_owned()
    }
}

pub(super) fn positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}
