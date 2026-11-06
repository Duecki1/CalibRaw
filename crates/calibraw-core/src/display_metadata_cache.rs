//! Per-folder cache of what the library shows of each photo without opening
//! it: display metadata (oriented dimensions and capture settings) and, for
//! libraries whose sidecars are slow to reach, a summary of each sidecar.
//!
//! One JSON file per folder lives in the thumbnail cache, so clearing that
//! cache clears it too. Entries are keyed by file name. Display metadata is
//! validated by the photo's size and modification time, a sidecar summary by
//! the sidecar's; a mismatch is a miss.

use crate::file_ops::write_bytes_atomically;
use crate::pipeline::RawDisplayMetadata;
use crate::sidecar::SidecarSummary;
use crate::thumbnail_cache::desktop_cache_path_for_raw;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The file name ending of a folder's cache, after an identity of the folder.
pub const CACHE_SUFFIX: &str = ".calibraw-folder-metadata.json";
/// Bump when display-metadata extraction changes, so stored values are re-read.
const CACHE_VERSION: u32 = 2;
const MAX_CACHE_BYTES: u64 = 32 * 1024 * 1024;

/// Identifies one version of a file's contents for cache validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    bytes: u64,
    /// Modification time in nanoseconds since the Unix epoch; 0 when unavailable.
    modified_nanos: u64,
}

impl FileStamp {
    pub const fn new(bytes: u64, modified_nanos: u64) -> Self {
        Self {
            bytes,
            modified_nanos,
        }
    }

    pub fn from_metadata(metadata: &fs::Metadata) -> Self {
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| {
                u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
            });
        Self {
            bytes: metadata.len(),
            modified_nanos,
        }
    }
}

/// Borrows the entries when saving and owns them when loading.
#[derive(Serialize, Deserialize)]
struct CacheDocument<'a> {
    version: u32,
    entries: Cow<'a, HashMap<String, CachedEntry>>,
    /// Absent from the caches of libraries that read sidecars directly, whose
    /// files therefore keep their earlier layout.
    #[serde(default, skip_serializing_if = "has_no_sidecars")]
    sidecars: Cow<'a, HashMap<String, CachedSidecar>>,
}

/// Takes the field as serde passes it, a `Cow`.
fn has_no_sidecars(sidecars: &impl Deref<Target = HashMap<String, CachedSidecar>>) -> bool {
    sidecars.is_empty()
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedEntry {
    stamp: FileStamp,
    dimensions: [u32; 2],
    aperture: f32,
    iso_speed: f32,
    shutter_seconds: f32,
    focal_length: f32,
    taken_seconds: u64,
}

impl CachedEntry {
    fn new(stamp: FileStamp, metadata: RawDisplayMetadata) -> Option<Self> {
        let RawDisplayMetadata {
            aperture,
            dimensions,
            iso_speed,
            shutter_seconds,
            focal_length,
            taken_seconds,
        } = metadata;
        // JSON has no non-finite numbers; such values are simply not cached.
        [aperture, iso_speed, shutter_seconds, focal_length]
            .iter()
            .all(|value| value.is_finite())
            .then_some(Self {
                stamp,
                dimensions,
                aperture,
                iso_speed,
                shutter_seconds,
                focal_length,
                taken_seconds,
            })
    }

    fn metadata(&self) -> RawDisplayMetadata {
        RawDisplayMetadata {
            aperture: self.aperture,
            dimensions: self.dimensions,
            iso_speed: self.iso_speed,
            shutter_seconds: self.shutter_seconds,
            focal_length: self.focal_length,
            taken_seconds: self.taken_seconds,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedSidecar {
    /// The sidecar's stamp, not the photo's.
    stamp: FileStamp,
    summary: SidecarSummary,
}

/// What a scan has not claimed yet, and what it has seen, of one kind of entry.
struct ScanEntries<T> {
    /// Stored entries not yet claimed by this scan.
    unclaimed: HashMap<String, T>,
    /// Entries for files seen during this scan.
    current: HashMap<String, T>,
}

impl<T> ScanEntries<T> {
    fn new(stored: HashMap<String, T>) -> Self {
        Self {
            unclaimed: stored,
            current: HashMap::new(),
        }
    }
}

/// The cached metadata of one folder for the duration of a scan.
///
/// Look up every scanned file with [`Self::get`] (and [`Self::sidecar`] for
/// files with a sidecar), record misses with [`Self::insert`] and
/// [`Self::insert_sidecar`], then [`Self::save`], repeatedly if misses are
/// filled in over time. Entries for files the scan did not see are dropped on
/// save.
pub struct FolderDisplayMetadataCache {
    /// Where the cache is stored; `None` keeps it for this scan only.
    path: Option<PathBuf>,
    entries: ScanEntries<CachedEntry>,
    sidecars: ScanEntries<CachedSidecar>,
    changed: bool,
}

impl FolderDisplayMetadataCache {
    /// The cache of a folder on disk, in the desktop thumbnail cache.
    pub fn load(folder: &Path) -> Self {
        Self::load_file(desktop_cache_path_for_raw(folder, CACHE_SUFFIX))
    }

    /// The cache stored at `path`, for a folder the platform names.
    pub fn load_file(path: PathBuf) -> Self {
        let (entries, sidecars) = read_entries(&path).unwrap_or_else(|error| {
            log::warn!(
                "could not read folder metadata cache {}: {error}",
                path.display()
            );
            (HashMap::new(), HashMap::new())
        });
        Self {
            path: Some(path),
            entries: ScanEntries::new(entries),
            sidecars: ScanEntries::new(sidecars),
            changed: false,
        }
    }

    /// A cache that starts empty and is never stored, for a platform that
    /// offers no place for it.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            entries: ScanEntries::new(HashMap::new()),
            sidecars: ScanEntries::new(HashMap::new()),
            changed: false,
        }
    }

    /// Returns the stored metadata of `file_name` when its stamp still matches.
    pub fn get(&mut self, file_name: &OsStr, stamp: FileStamp) -> Option<RawDisplayMetadata> {
        let name = file_name.to_str()?;
        let entry = self.entries.unclaimed.remove(name)?;
        if entry.stamp != stamp {
            self.changed = true;
            return None;
        }
        let metadata = entry.metadata();
        self.entries.current.insert(name.to_owned(), entry);
        Some(metadata)
    }

    pub fn insert(&mut self, file_name: &OsStr, stamp: FileStamp, metadata: RawDisplayMetadata) {
        let Some(name) = file_name.to_str() else {
            return;
        };
        if let Some(entry) = CachedEntry::new(stamp, metadata) {
            self.entries.current.insert(name.to_owned(), entry);
            self.changed = true;
        }
    }

    /// Returns the stored summary of the sidecar of `file_name` when the
    /// sidecar's stamp still matches.
    pub fn sidecar(&mut self, file_name: &OsStr, stamp: FileStamp) -> Option<SidecarSummary> {
        let name = file_name.to_str()?;
        let entry = self.sidecars.unclaimed.remove(name)?;
        if entry.stamp != stamp {
            self.changed = true;
            return None;
        }
        let summary = entry.summary.clone();
        self.sidecars.current.insert(name.to_owned(), entry);
        Some(summary)
    }

    /// Records the summary of the sidecar of `file_name`, as of `stamp`.
    pub fn insert_sidecar(&mut self, file_name: &OsStr, stamp: FileStamp, summary: SidecarSummary) {
        let Some(name) = file_name.to_str() else {
            return;
        };
        // A summary holds only values decoded from JSON, which are finite.
        self.sidecars
            .current
            .insert(name.to_owned(), CachedSidecar { stamp, summary });
        self.changed = true;
    }

    /// Writes the entries seen during this scan when they differ from the stored
    /// file. Saving again writes only entries inserted since the last save.
    pub fn save(&mut self) -> Result<(), String> {
        // Unclaimed entries belong to files that were removed or not scanned.
        if !self.changed && self.entries.unclaimed.is_empty() && self.sidecars.unclaimed.is_empty()
        {
            return Ok(());
        }
        if let Some(path) = &self.path {
            let document = CacheDocument {
                version: CACHE_VERSION,
                entries: Cow::Borrowed(&self.entries.current),
                sidecars: Cow::Borrowed(&self.sidecars.current),
            };
            let bytes = serde_json::to_vec(&document)
                .map_err(|error| format!("could not encode folder metadata cache: {error}"))?;
            write_bytes_atomically(path, &bytes).map_err(|error| {
                format!(
                    "could not write folder metadata cache {}: {error}",
                    path.display()
                )
            })?;
        }
        self.entries.unclaimed.clear();
        self.sidecars.unclaimed.clear();
        self.changed = false;
        Ok(())
    }
}

type StoredEntries = (HashMap<String, CachedEntry>, HashMap<String, CachedSidecar>);

fn read_entries(path: &Path) -> Result<StoredEntries, String> {
    let bytes = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CACHE_BYTES => {
            fs::read(path).map_err(|error| error.to_string())?
        }
        Ok(_) => return Err("the cache file is not a regular file of a safe size".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((HashMap::new(), HashMap::new()))
        }
        Err(error) => return Err(error.to_string()),
    };
    let document: CacheDocument<'static> =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(if document.version == CACHE_VERSION {
        (
            document.entries.into_owned(),
            document.sidecars.into_owned(),
        )
    } else {
        (HashMap::new(), HashMap::new())
    })
}
