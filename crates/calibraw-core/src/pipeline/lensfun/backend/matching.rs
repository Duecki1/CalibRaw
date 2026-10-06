//! Matching the camera and lens of a RAW against the Lensfun database.

use super::*;

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
            lf_db_find_lenses_hd(
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

pub(super) fn find_auto_lens(
    database: &Database,
    camera: Option<*const lfCamera>,
    raw: &LoadedRaw,
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
                lf_db_find_lenses_hd(
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
        candidates = camera
            .map(|camera| compatible_lenses(database, camera))
            .unwrap_or_else(|| all_lenses(database));
    }
    sort_and_deduplicate_lenses(&mut candidates);

    let exact = candidates
        .iter()
        .filter(|candidate| lens_metadata_is_exact(raw, candidate))
        .cloned()
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return exact.into_iter().next();
    }

    if camera.is_some()
        && candidates.len() == 1
        && profile_supports_capture(database, camera, raw, &candidates[0])
    {
        return candidates.into_iter().next();
    }

    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| {
            automatic_match_score(database, camera, raw, &candidate).map(|score| (score, candidate))
        })
        .collect::<Vec<_>>();
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
    if let Some((runner_up, _)) = ranked.get(1) {
        if *best_score - *runner_up < 0.08 {
            return None;
        }
    }
    Some(best.clone())
}

fn lens_metadata_is_exact(raw: &LoadedRaw, candidate: &LensfunLens) -> bool {
    if !maker_is_compatible(&raw.lens_make, &raw.lens_model, &candidate.maker) {
        return false;
    }
    canonical_lens_model(
        &raw.lens_model,
        &[raw.lens_make.as_str(), candidate.maker.as_str()],
    ) == canonical_lens_model(
        &candidate.model,
        &[candidate.maker.as_str(), raw.lens_make.as_str()],
    )
}

fn automatic_match_score(
    database: &Database,
    camera: Option<*const lfCamera>,
    raw: &LoadedRaw,
    candidate: &LensfunLens,
) -> Option<f32> {
    if !maker_is_compatible(&raw.lens_make, &raw.lens_model, &candidate.maker)
        || !profile_supports_capture(database, camera, raw, candidate)
    {
        return None;
    }

    let raw_model = canonical_lens_model(
        &raw.lens_model,
        &[raw.lens_make.as_str(), candidate.maker.as_str()],
    );
    let candidate_model = canonical_lens_model(
        &candidate.model,
        &[candidate.maker.as_str(), raw.lens_make.as_str()],
    );
    if raw_model.is_empty() || candidate_model.is_empty() {
        return None;
    }
    if raw_model == candidate_model {
        return Some(1.0);
    }

    let raw_codes = lens_model_codes(&raw.lens_model);
    let candidate_codes = lens_model_codes(&candidate.model);
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

    let raw_tokens = lens_tokens(&raw.lens_model, &raw.lens_make, &candidate.maker);
    let candidate_tokens = lens_tokens(&candidate.model, &candidate.maker, &raw.lens_make);
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
                lf_db_find_lenses_hd(
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

fn find_lens_in_database(database: &Database, selection: &LensfunLens) -> Option<*const lfLens> {
    pointer_list(unsafe { lf_db_get_lenses(database.0) })
        .into_iter()
        .find(|pointer| {
            lens_name(*pointer).is_some_and(|candidate| {
                candidate.model.eq_ignore_ascii_case(&selection.model)
                    && (selection.maker.trim().is_empty()
                        || candidate.maker.eq_ignore_ascii_case(&selection.maker))
            })
        })
}

#[derive(Clone, Copy)]
pub(super) struct LensFields {
    pub(super) crop_factor: f32,
    pub(super) min_focal: f32,
    pub(super) max_focal: f32,
    pub(super) lens_type: ffi::lfLensType,
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
        lens_type: lens.Type,
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
