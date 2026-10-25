//! The LUT library: imported `.cube` files kept in an application folder.
//!
//! Each LUT is stored as `<id>.cube` with the bytes it was imported with, so
//! its [`LutId`] stays the fingerprint of the file. `luts.json` lists the
//! LUTs and their names. Files the list does not name are ignored, and list
//! entries whose file is gone are dropped when the library is opened.
//!
//! [`LutLibrary`] is owned by one thread and changes the folder;
//! [`LutStore`] is the shareable, read-only side that loads LUTs for
//! rendering on any thread.

use super::{parse_cube, CubeLut, CubeLutError, LutId, MAX_CUBE_FILE_BYTES};
use crate::file_ops::write_bytes_atomically;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

pub const MAX_LUT_NAME_CHARS: usize = 64;

const INDEX_FILE: &str = "luts.json";
const INDEX_FORMAT: &str = "CalibRaw LUT library";
/// Bump for every incompatible change to the index layout.
const INDEX_SCHEMA_VERSION: u32 = 1;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;
/// Parsed LUTs kept for reuse. A 65-point LUT holds about 3.3 MB.
const CACHED_LUTS: usize = 8;

/// The LUT folder for the settings file at `settings_path`, next to the
/// presets folder.
pub fn lut_folder_for_settings(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("luts")
}

#[derive(Debug)]
pub enum LutLibraryError {
    Io(std::io::Error),
    Cube(CubeLutError),
    InvalidName(String),
    InvalidIndex(String),
    NotFound(LutId),
}

impl fmt::Display for LutLibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Cube(error) => write!(formatter, "{error}"),
            Self::InvalidName(message) | Self::InvalidIndex(message) => {
                formatter.write_str(message)
            }
            Self::NotFound(id) => write!(formatter, "LUT {id} is not in the LUT library"),
        }
    }
}

impl std::error::Error for LutLibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Cube(error) => Some(error),
            Self::InvalidName(_) | Self::InvalidIndex(_) | Self::NotFound(_) => None,
        }
    }
}

impl From<std::io::Error> for LutLibraryError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<CubeLutError> for LutLibraryError {
    fn from(error: CubeLutError) -> Self {
        Self::Cube(error)
    }
}

/// A LUT listed in the library.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LutEntry {
    pub id: LutId,
    pub name: String,
    /// Lattice points per axis.
    pub size: u32,
}

/// The result of [`LutLibrary::import`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LutImport {
    pub entry: LutEntry,
    /// False when the library already held the same file.
    pub added: bool,
}

#[derive(Deserialize, Serialize)]
struct IndexDocument {
    format: String,
    schema_version: u32,
    luts: Vec<LutEntry>,
}

/// Loads library LUTs for rendering. Clones share one cache.
#[derive(Clone, Debug)]
pub struct LutStore {
    inner: Arc<StoreInner>,
}

#[derive(Debug)]
struct StoreInner {
    folder: PathBuf,
    /// Most recently used last. A leaf lock: never held while reading or
    /// parsing a file.
    cache: Mutex<Vec<Arc<CubeLut>>>,
}

impl LutStore {
    pub fn new(folder: PathBuf) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                folder,
                cache: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn folder(&self) -> &Path {
        &self.inner.folder
    }

    /// The LUT `id` if it was read recently, without reading its file.
    pub fn cached(&self, id: LutId) -> Option<Arc<CubeLut>> {
        let cache = self.lock_cache();
        cache.iter().find(|lut| lut.id() == id).cloned()
    }

    /// The LUT `id`, parsed from its library file or taken from the cache.
    pub fn load(&self, id: LutId) -> Result<Arc<CubeLut>, LutLibraryError> {
        if let Some(lut) = self.take_cached(id) {
            return Ok(lut);
        }
        let bytes = match read_bounded(&lut_path(self.folder(), id), MAX_CUBE_FILE_BYTES) {
            Ok(bytes) => bytes,
            Err(LutLibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(LutLibraryError::NotFound(id))
            }
            Err(error) => return Err(error),
        };
        let lut = Arc::new(parse_cube(&bytes)?);
        if lut.id() != id {
            return Err(LutLibraryError::InvalidIndex(format!(
                "the library file of LUT {id} has changed"
            )));
        }
        self.remember(Arc::clone(&lut));
        Ok(lut)
    }

    /// The cached LUT `id`, marked as the most recently used.
    fn take_cached(&self, id: LutId) -> Option<Arc<CubeLut>> {
        let mut cache = self.lock_cache();
        let index = cache.iter().position(|lut| lut.id() == id)?;
        let lut = cache.remove(index);
        cache.push(Arc::clone(&lut));
        Some(lut)
    }

    fn remember(&self, lut: Arc<CubeLut>) {
        let mut cache = self.lock_cache();
        cache.retain(|cached| cached.id() != lut.id());
        cache.push(lut);
        let excess = cache.len().saturating_sub(CACHED_LUTS);
        cache.drain(..excess);
    }

    fn forget(&self, id: LutId) {
        self.lock_cache().retain(|cached| cached.id() != id);
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, Vec<Arc<CubeLut>>> {
        // The cache only holds complete, immutable LUTs, so it stays valid
        // after a panic elsewhere.
        self.inner
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The library's list of LUTs and the operations that change its folder.
#[derive(Debug)]
pub struct LutLibrary {
    store: LutStore,
    /// Sorted by name, case-insensitively.
    entries: Vec<LutEntry>,
}

impl LutLibrary {
    /// Opens the library in `folder`. A missing folder is an empty library.
    pub fn open(folder: PathBuf) -> Result<Self, LutLibraryError> {
        let mut entries = read_index(&folder)?;
        entries.retain(|entry| lut_path(&folder, entry.id).is_file());
        let mut library = Self {
            store: LutStore::new(folder),
            entries,
        };
        library.sort();
        Ok(library)
    }

    /// An empty library in `folder` that does not read it, for when opening failed.
    pub fn empty(folder: PathBuf) -> Self {
        Self {
            store: LutStore::new(folder),
            entries: Vec::new(),
        }
    }

    pub fn store(&self) -> &LutStore {
        &self.store
    }

    pub fn entries(&self) -> &[LutEntry] {
        &self.entries
    }

    pub fn entry(&self, id: LutId) -> Option<&LutEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Adds the `.cube` file `bytes` to the library. The name is the file's
    /// `TITLE`, or else the stem of `file_name`, made unique in the library.
    /// A file the library already holds is not added again.
    pub fn import(&mut self, bytes: &[u8], file_name: &str) -> Result<LutImport, LutLibraryError> {
        let lut = Arc::new(parse_cube(bytes)?);
        if let Some(entry) = self.entry(lut.id()) {
            return Ok(LutImport {
                entry: entry.clone(),
                added: false,
            });
        }
        let stem = Path::new(file_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let base = lut
            .title()
            .and_then(|title| normalize_name(title).ok())
            .or_else(|| normalize_name(stem).ok())
            .unwrap_or_else(|| "LUT".to_owned());
        let entry = LutEntry {
            id: lut.id(),
            name: self.unique_name(&base),
            size: lut.size(),
        };
        write_bytes_atomically(&lut_path(self.store.folder(), entry.id), bytes)?;
        self.entries.push(entry.clone());
        self.sort();
        self.write_index()?;
        self.store.remember(lut);
        Ok(LutImport { entry, added: true })
    }

    /// Imports the `.cube` file at `path`, as [`Self::import`].
    pub fn import_file(&mut self, path: &Path) -> Result<LutImport, LutLibraryError> {
        let bytes = read_bounded(path, MAX_CUBE_FILE_BYTES)?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        self.import(&bytes, &file_name)
    }

    /// Renames the LUT `id` and returns the name it got.
    pub fn rename(&mut self, id: LutId, name: &str) -> Result<String, LutLibraryError> {
        let name = normalize_name(name)?;
        if self
            .entries
            .iter()
            .any(|entry| entry.id != id && same_name(&entry.name, &name))
        {
            return Err(LutLibraryError::InvalidName(format!(
                "a LUT named \u{201c}{name}\u{201d} already exists"
            )));
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(LutLibraryError::NotFound(id))?;
        entry.name = name.clone();
        self.sort();
        self.write_index()?;
        Ok(name)
    }

    /// Removes the LUT `id` and its file. Edits that use it keep naming it.
    pub fn remove(&mut self, id: LutId) -> Result<(), LutLibraryError> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(LutLibraryError::NotFound(id))?;
        let removed = self.entries.remove(index);
        if let Err(error) = self.write_index() {
            self.entries.insert(index, removed);
            return Err(error);
        }
        self.store.forget(id);
        match std::fs::remove_file(lut_path(self.store.folder(), id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| {
            self.entries
                .iter()
                .any(|entry| same_name(&entry.name, name))
        };
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|index| {
                let suffix = format!(" {index}");
                let kept: String = base
                    .chars()
                    .take(MAX_LUT_NAME_CHARS - suffix.chars().count())
                    .collect();
                format!("{}{suffix}", kept.trim_end())
            })
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_owned())
    }

    fn sort(&mut self) {
        self.entries
            .sort_by_cached_key(|entry| (entry.name.to_lowercase(), entry.id));
    }

    fn write_index(&self) -> Result<(), LutLibraryError> {
        let document = IndexDocument {
            format: INDEX_FORMAT.to_owned(),
            schema_version: INDEX_SCHEMA_VERSION,
            luts: self.entries.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| {
            LutLibraryError::InvalidIndex(format!("could not encode the LUT list: {error}"))
        })?;
        write_bytes_atomically(&self.store.folder().join(INDEX_FILE), &bytes)?;
        Ok(())
    }
}

fn read_index(folder: &Path) -> Result<Vec<LutEntry>, LutLibraryError> {
    let bytes = match read_bounded(&folder.join(INDEX_FILE), MAX_INDEX_BYTES) {
        Ok(bytes) => bytes,
        Err(LutLibraryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error),
    };
    let document: IndexDocument = serde_json::from_slice(&bytes).map_err(|error| {
        LutLibraryError::InvalidIndex(format!("the LUT list is not valid: {error}"))
    })?;
    if document.format != INDEX_FORMAT {
        return Err(LutLibraryError::InvalidIndex(
            "the LUT list is not a CalibRaw LUT library".to_owned(),
        ));
    }
    if document.schema_version != INDEX_SCHEMA_VERSION {
        return Err(LutLibraryError::InvalidIndex(format!(
            "LUT list schema {} is not supported",
            document.schema_version
        )));
    }
    let mut entries = Vec::with_capacity(document.luts.len());
    for mut entry in document.luts {
        entry.name = normalize_name(&entry.name)?;
        if !entries.iter().any(|kept: &LutEntry| kept.id == entry.id) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, LutLibraryError> {
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit {
        return Err(CubeLutError::TooLarge(length).into());
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(CubeLutError::TooLarge(bytes.len() as u64).into());
    }
    Ok(bytes)
}

fn lut_path(folder: &Path, id: LutId) -> PathBuf {
    folder.join(format!("{id}.cube"))
}

fn normalize_name(name: &str) -> Result<String, LutLibraryError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LutLibraryError::InvalidName(
            "a LUT name cannot be empty".to_owned(),
        ));
    }
    if name.chars().count() > MAX_LUT_NAME_CHARS {
        return Err(LutLibraryError::InvalidName(format!(
            "a LUT name can have at most {MAX_LUT_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(LutLibraryError::InvalidName(
            "a LUT name cannot contain control characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn same_name(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}
