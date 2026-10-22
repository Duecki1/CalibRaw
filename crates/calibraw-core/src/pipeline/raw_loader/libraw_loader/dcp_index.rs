//! Index of DCP camera profiles in a folder and matching against a camera.

use super::*;

const MAX_DCP_SCAN_FILES: usize = 10_000;
const MAX_DCP_SCAN_DEPTH: usize = 16;

// Ranking of a profile against a camera, used by `dcp_match_score`. The score tiers rank
// where the model matched, a declared model always outranking a file name match. The
// bonuses and the penalty then nudge profiles inside and between the closer file name
// tiers.
/// The profile declares exactly this camera model.
const SCORE_DECLARED_MODEL_EXACT: i32 = 1000;
/// The profile's declared camera model contains this model.
const SCORE_DECLARED_MODEL_CONTAINS: i32 = 900;
/// The file name is exactly the model, for profiles that declare no camera.
const SCORE_FILENAME_EXACT: i32 = 700;
/// The file name contains the model.
const SCORE_FILENAME_CONTAINS: i32 = 600;
/// Only the folder path mentions the model.
const SCORE_PATH_CONTAINS: i32 = 550;
/// Shortest normalised model name that may match by containment; shorter names would
/// match unrelated profiles.
const MIN_CONTAINED_MODEL_KEY_LEN: usize = 4;
/// The camera make appears in the profile name, file name or path.
const BONUS_MAKE_MATCH: i32 = 25;
/// Adobe's "Camera Standard" profile (also written "Camera ST").
const BONUS_CAMERA_STANDARD: i32 = 20;
/// Any other "standard" profile.
const BONUS_STANDARD: i32 = 15;
/// Monochrome profiles, which would drop the colour of a colour camera's photo.
const PENALTY_MONOCHROME: i32 = 30;

pub(super) struct MatchedDcpProfile {
    score: i32,
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) profile: DcpProfile,
}

#[derive(Clone)]
struct IndexedDcpProfile {
    pub(super) path: PathBuf,
    pub(super) name: String,
    profile_name: Option<String>,
    camera_model: Option<String>,
}

#[derive(Clone)]
struct CachedDcpIndex {
    root_modified: Option<SystemTime>,
    profiles: Arc<Vec<IndexedDcpProfile>>,
}

static DCP_PROFILE_INDEX: OnceLock<Mutex<HashMap<PathBuf, CachedDcpIndex>>> = OnceLock::new();
static DCP_PROFILE_SCAN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn dcp_profile_index_cache() -> &'static Mutex<HashMap<PathBuf, CachedDcpIndex>> {
    DCP_PROFILE_INDEX.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Locks `mutex`, continuing with the data of a poisoned lock: the index and the scan
/// lock hold no invariant that a panic in the middle of an update could leave broken.
fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(in crate::pipeline::raw_loader) fn invalidate_dcp_profile_index() {
    if let Some(cache) = DCP_PROFILE_INDEX.get() {
        lock_ignoring_poison(cache).clear();
    }
}

/// The cached index of `cache_key` when the folder has not changed since it was built.
fn cached_profiles(
    cache_key: &Path,
    root_modified: Option<SystemTime>,
) -> Option<Arc<Vec<IndexedDcpProfile>>> {
    let cache = lock_ignoring_poison(dcp_profile_index_cache());
    cache
        .get(cache_key)
        .filter(|index| index.root_modified == root_modified)
        .map(|index| Arc::clone(&index.profiles))
}

pub(in crate::pipeline::raw_loader) fn prewarm_dcp_profile_index(folder: &Path) {
    if let Err(error) = indexed_dcp_profiles(folder) {
        log::warn!(
            "could not prewarm DCP profile index for {}: {error:#}",
            folder.display()
        );
    }
}

fn indexed_dcp_profiles(folder: &Path) -> Result<Arc<Vec<IndexedDcpProfile>>> {
    let metadata = fs::metadata(folder)
        .with_context(|| format!("inspect DCP profile folder {}", folder.display()))?;
    anyhow::ensure!(
        metadata.is_dir(),
        "configured DCP profile path is not a folder"
    );
    let cache_key = fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let root_modified = metadata.modified().ok();

    if let Some(profiles) = cached_profiles(&cache_key, root_modified) {
        return Ok(profiles);
    }

    let _scan_guard = lock_ignoring_poison(DCP_PROFILE_SCAN_LOCK.get_or_init(|| Mutex::new(())));
    // Another thread may have indexed this folder while this one waited for the scan lock.
    if let Some(profiles) = cached_profiles(&cache_key, root_modified) {
        return Ok(profiles);
    }

    let mut stack = vec![(folder.to_path_buf(), 0usize)];
    let mut scanned = 0usize;
    let mut profiles = Vec::new();
    'scan: while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_DCP_SCAN_DEPTH {
            continue;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!(
                    "could not read DCP directory {}: {error}",
                    directory.display()
                );
                continue;
            }
        };
        for entry in entries {
            if scanned >= MAX_DCP_SCAN_FILES {
                log::warn!(
                    "stopped DCP profile scan after {MAX_DCP_SCAN_FILES} filesystem entries"
                );
                break 'scan;
            }
            scanned += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!("could not inspect DCP directory entry: {error}");
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() && !file_type.is_symlink() {
                stack.push((entry.path(), depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dcp"))
            {
                continue;
            }
            match entry.metadata() {
                Ok(metadata) if metadata.len() > 0 && metadata.len() <= MAX_DCP_FILE_BYTES => {}
                _ => continue,
            }
            let identity = match DcpProfile::identity_from_path(&path) {
                Ok(Some(identity)) => identity,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!(
                        "ignoring invalid DCP profile identity {}: {error:#}",
                        path.display()
                    );
                    continue;
                }
            };
            let name = dcp_profile_display_name(identity.name.as_deref(), &path);
            profiles.push(IndexedDcpProfile {
                path,
                name,
                profile_name: identity.name,
                camera_model: identity.camera_model,
            });
        }
    }

    let profiles = Arc::new(profiles);
    lock_ignoring_poison(dcp_profile_index_cache()).insert(
        cache_key,
        CachedDcpIndex {
            root_modified,
            profiles: Arc::clone(&profiles),
        },
    );
    crate::diagnostics::record(format!(
        "Indexed {} DCP profiles after scanning {scanned} filesystem entries",
        profiles.len()
    ));
    Ok(profiles)
}

pub(super) fn find_matching_dcp_profiles(
    folder: &Path,
    camera_make: &str,
    camera_model: &str,
) -> Result<Vec<MatchedDcpProfile>> {
    let make_key = normalize_camera_name(camera_make);
    let model_key = normalize_camera_name(camera_model);
    let combined_key = normalize_camera_name(&format!("{camera_make} {camera_model}"));
    if model_key.is_empty() {
        return Ok(Vec::new());
    }

    let index = indexed_dcp_profiles(folder)?;
    let mut matches = Vec::new();
    for indexed in index.iter() {
        let score = dcp_match_score(
            indexed.camera_model.as_deref(),
            indexed.profile_name.as_deref(),
            &indexed.path,
            &make_key,
            &model_key,
            &combined_key,
        );
        if score <= 0 {
            continue;
        }
        let profile = match DcpProfile::from_path(&indexed.path) {
            Ok(Some(profile)) => profile,
            Ok(None) => continue,
            Err(error) => {
                log::warn!(
                    "ignoring matched but invalid DCP profile {}: {error:#}",
                    indexed.path.display()
                );
                continue;
            }
        };
        matches.push(MatchedDcpProfile {
            score,
            path: indexed.path.clone(),
            name: indexed.name.clone(),
            profile,
        });
    }

    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| {
                left.path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .cmp(&right.path.to_string_lossy().to_ascii_lowercase())
            })
    });

    for index in 0..matches.len() {
        let duplicate = matches.iter().enumerate().any(|(other_index, other)| {
            other_index != index && other.name.eq_ignore_ascii_case(&matches[index].name)
        });
        if duplicate {
            let file_name = matches[index]
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned);
            if let Some(file_name) = file_name {
                let base_name = matches[index].name.clone();
                matches[index].name = format!("{base_name} — {file_name}");
            }
        }
    }

    Ok(matches)
}

pub(super) fn dcp_profile_display_name(profile_name: Option<&str>, path: &Path) -> String {
    profile_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "Camera profile".to_owned())
}

fn dcp_match_score(
    camera_model: Option<&str>,
    profile_name: Option<&str>,
    path: &Path,
    make_key: &str,
    model_key: &str,
    combined_key: &str,
) -> i32 {
    let declared = camera_model.map(normalize_camera_name).unwrap_or_default();
    let filename = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(normalize_camera_name)
        .unwrap_or_default();
    let profile_name = profile_name.map(normalize_camera_name).unwrap_or_default();
    let path_key = normalize_camera_name(&path.to_string_lossy());

    let contains_model = model_key.len() >= MIN_CONTAINED_MODEL_KEY_LEN;
    let mut score = if !declared.is_empty() {
        if declared == model_key || declared == combined_key {
            SCORE_DECLARED_MODEL_EXACT
        } else if contains_model && declared.contains(model_key) {
            SCORE_DECLARED_MODEL_CONTAINS
        } else {
            return 0;
        }
    } else if filename == model_key || filename == combined_key {
        SCORE_FILENAME_EXACT
    } else if contains_model && filename.contains(model_key) {
        SCORE_FILENAME_CONTAINS
    } else if contains_model && path_key.contains(model_key) {
        SCORE_PATH_CONTAINS
    } else {
        return 0;
    };

    if !make_key.is_empty()
        && (declared.contains(make_key)
            || filename.contains(make_key)
            || path_key.contains(make_key))
    {
        score += BONUS_MAKE_MATCH;
    }
    if profile_name.contains("camerastandard")
        || filename.contains("camerastandard")
        || profile_name.ends_with("camerast")
        || filename.ends_with("camerast")
    {
        score += BONUS_CAMERA_STANDARD;
    } else if profile_name.contains("standard") || filename.contains("standard") {
        score += BONUS_STANDARD;
    }
    if profile_name.contains("monochrome")
        || filename.contains("monochrome")
        || profile_name.ends_with("camerabw")
        || filename.ends_with("camerabw")
    {
        score -= PENALTY_MONOCHROME;
    }
    score
}

fn normalize_camera_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}
