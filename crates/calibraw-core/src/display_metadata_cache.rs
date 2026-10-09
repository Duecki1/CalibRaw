//! Per-folder cache of library display metadata (oriented dimensions and
//! capture settings), so rescanning a folder does not reopen every photo.
//!
//! One JSON file per folder lives in the desktop thumbnail cache, so clearing
//! that cache clears it too. Entries are keyed by file name and validated by the
//! file's size and modification time; a mismatch is a miss.

use crate::file_ops::write_bytes_atomically;
use crate::pipeline::RawDisplayMetadata;
use crate::thumbnail_cache::desktop_cache_path_for_raw;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const CACHE_SUFFIX: &str = ".calibraw-folder-metadata.json";
/// Bump when display-metadata extraction changes, so stored values are re-read.
const CACHE_VERSION: u32 = 1;
const MAX_CACHE_BYTES: u64 = 32 * 1024 * 1024;

/// Identifies one version of a file's contents for cache validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    bytes: u64,
    /// Modification time in nanoseconds since the Unix epoch; 0 when unavailable.
    modified_nanos: u64,
}

impl FileStamp {
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

#[derive(Serialize, Deserialize)]
struct CacheDocument {
    version: u32,
    entries: HashMap<String, CachedEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedEntry {
    stamp: FileStamp,
    dimensions: [u32; 2],
    aperture: f32,
    iso_speed: f32,
    shutter_seconds: f32,
    focal_length: f32,
}

impl CachedEntry {
    fn new(stamp: FileStamp, metadata: RawDisplayMetadata) -> Option<Self> {
        let RawDisplayMetadata {
            aperture,
            dimensions,
            iso_speed,
            shutter_seconds,
            focal_length,
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
            })
    }

    fn metadata(&self) -> RawDisplayMetadata {
        RawDisplayMetadata {
            aperture: self.aperture,
            dimensions: self.dimensions,
            iso_speed: self.iso_speed,
            shutter_seconds: self.shutter_seconds,
            focal_length: self.focal_length,
        }
    }
}

/// The cached metadata of one folder for the duration of a scan.
///
/// Look up every scanned file with [`Self::get`], record misses with
/// [`Self::insert`], then [`Self::save`]. Entries for files the scan did not
/// see are dropped on save.
pub struct FolderDisplayMetadataCache {
    path: PathBuf,
    /// Stored entries not yet claimed by this scan.
    unclaimed: HashMap<String, CachedEntry>,
    /// Entries for files seen during this scan.
    current: HashMap<String, CachedEntry>,
    changed: bool,
}

impl FolderDisplayMetadataCache {
    pub fn load(folder: &Path) -> Self {
        Self::load_from(desktop_cache_path_for_raw(folder, CACHE_SUFFIX))
    }

    fn load_from(path: PathBuf) -> Self {
        let unclaimed = read_entries(&path).unwrap_or_else(|error| {
            log::warn!(
                "could not read folder metadata cache {}: {error}",
                path.display()
            );
            HashMap::new()
        });
        Self {
            path,
            unclaimed,
            current: HashMap::new(),
            changed: false,
        }
    }

    /// Returns the stored metadata of `file_name` when its stamp still matches.
    pub fn get(&mut self, file_name: &OsStr, stamp: FileStamp) -> Option<RawDisplayMetadata> {
        let name = file_name.to_str()?;
        let entry = self.unclaimed.remove(name)?;
        if entry.stamp != stamp {
            self.changed = true;
            return None;
        }
        let metadata = entry.metadata();
        self.current.insert(name.to_owned(), entry);
        Some(metadata)
    }

    pub fn insert(&mut self, file_name: &OsStr, stamp: FileStamp, metadata: RawDisplayMetadata) {
        let Some(name) = file_name.to_str() else {
            return;
        };
        if let Some(entry) = CachedEntry::new(stamp, metadata) {
            self.current.insert(name.to_owned(), entry);
            self.changed = true;
        }
    }

    /// Writes the entries seen during this scan when they differ from the stored file.
    pub fn save(self) -> Result<(), String> {
        // Unclaimed entries belong to files that were removed or not scanned.
        if !self.changed && self.unclaimed.is_empty() {
            return Ok(());
        }
        let document = CacheDocument {
            version: CACHE_VERSION,
            entries: self.current,
        };
        let bytes = serde_json::to_vec(&document)
            .map_err(|error| format!("could not encode folder metadata cache: {error}"))?;
        write_bytes_atomically(&self.path, &bytes).map_err(|error| {
            format!(
                "could not write folder metadata cache {}: {error}",
                self.path.display()
            )
        })
    }
}

fn read_entries(path: &Path) -> Result<HashMap<String, CachedEntry>, String> {
    let bytes = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CACHE_BYTES => {
            fs::read(path).map_err(|error| error.to_string())?
        }
        Ok(_) => return Err("the cache file is not a regular file of a safe size".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    let document: CacheDocument =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(if document.version == CACHE_VERSION {
        document.entries
    } else {
        HashMap::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: FileStamp = FileStamp {
        bytes: 25_000_000,
        modified_nanos: 1_700_000_000_123_456_789,
    };
    const METADATA: RawDisplayMetadata = RawDisplayMetadata {
        aperture: 2.8,
        dimensions: [6000, 4000],
        iso_speed: 400.0,
        shutter_seconds: 0.004,
        focal_length: 35.0,
    };

    fn cache_path(directory: &tempfile::TempDir) -> PathBuf {
        directory
            .path()
            .join("ab")
            .join(format!("folder{CACHE_SUFFIX}"))
    }

    #[test]
    fn stored_metadata_round_trips_while_the_stamp_matches() {
        let directory = tempfile::tempdir().unwrap();
        let mut cache = FolderDisplayMetadataCache::load_from(cache_path(&directory));
        assert_eq!(cache.get(OsStr::new("IMG_0001.CR3"), STAMP), None);
        cache.insert(OsStr::new("IMG_0001.CR3"), STAMP, METADATA);
        cache.save().unwrap();

        let mut cache = FolderDisplayMetadataCache::load_from(cache_path(&directory));
        assert_eq!(cache.get(OsStr::new("IMG_0001.CR3"), STAMP), Some(METADATA));
        let changed = FileStamp {
            modified_nanos: STAMP.modified_nanos + 1,
            ..STAMP
        };
        let mut cache = FolderDisplayMetadataCache::load_from(cache_path(&directory));
        assert_eq!(cache.get(OsStr::new("IMG_0001.CR3"), changed), None);
    }

    #[test]
    fn entries_for_files_not_seen_by_the_scan_are_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let mut cache = FolderDisplayMetadataCache::load_from(cache_path(&directory));
        cache.insert(OsStr::new("kept.nef"), STAMP, METADATA);
        cache.insert(OsStr::new("removed.nef"), STAMP, METADATA);
        cache.save().unwrap();

        let mut cache = FolderDisplayMetadataCache::load_from(cache_path(&directory));
        assert!(cache.get(OsStr::new("kept.nef"), STAMP).is_some());
        cache.save().unwrap();

        let mut cache = FolderDisplayMetadataCache::load_from(cache_path(&directory));
        assert!(cache.get(OsStr::new("kept.nef"), STAMP).is_some());
        assert_eq!(cache.get(OsStr::new("removed.nef"), STAMP), None);
    }

    #[test]
    fn unchanged_scans_and_empty_folders_do_not_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = cache_path(&directory);
        FolderDisplayMetadataCache::load_from(path.clone())
            .save()
            .unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn malformed_and_outdated_caches_are_misses() {
        let directory = tempfile::tempdir().unwrap();
        let path = cache_path(&directory);
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        fs::write(&path, b"{not json").unwrap();
        let mut cache = FolderDisplayMetadataCache::load_from(path.clone());
        assert_eq!(cache.get(OsStr::new("IMG_0001.CR3"), STAMP), None);

        let mut entries = HashMap::new();
        entries.insert(
            "IMG_0001.CR3".to_owned(),
            CachedEntry::new(STAMP, METADATA).unwrap(),
        );
        let outdated = CacheDocument {
            version: CACHE_VERSION + 1,
            entries,
        };
        fs::write(&path, serde_json::to_vec(&outdated).unwrap()).unwrap();
        let mut cache = FolderDisplayMetadataCache::load_from(path);
        assert_eq!(cache.get(OsStr::new("IMG_0001.CR3"), STAMP), None);
    }

    #[test]
    fn non_finite_metadata_is_not_cached() {
        let metadata = RawDisplayMetadata {
            aperture: f32::NAN,
            ..METADATA
        };
        assert!(CachedEntry::new(STAMP, metadata).is_none());
    }
}
