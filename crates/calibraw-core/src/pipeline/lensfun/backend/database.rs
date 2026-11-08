//! The process's Lensfun database and owned modifier and pointer-list handles.

use super::*;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The process's one Lensfun database, loaded on first use and kept until the
/// process ends.
///
/// Lensfun cannot be called from two threads at once: every `lfLens`
/// constructor and destructor updates a global reference count without
/// synchronization, the last destructor frees the lens-name patterns, and
/// `lfLens::GuessParameters` compiles them lazily into a global table while a
/// database loads. So every Lensfun call except a configured modifier's
/// `lf_modifier_apply_*` functions holds this lock: through
/// [`with_database`], or [`lock_lensfun`] for calls that need no database.
static DATABASE: Mutex<Option<Database>> = Mutex::new(None);

/// Runs `use_database` on the database while holding the Lensfun lock. A load
/// that fails is not kept, so the next call tries again.
pub(super) fn with_database<T>(use_database: impl FnOnce(&Database) -> Result<T>) -> Result<T> {
    let mut slot = lock_lensfun();
    if slot.is_none() {
        *slot = Some(Database::load()?);
    }
    match slot.as_ref() {
        Some(database) => use_database(database),
        None => Err(anyhow!("the Lensfun database is not loaded")),
    }
}

/// Holds the Lensfun lock for Lensfun calls that need no database, such as
/// destroying a modifier.
pub(super) fn lock_lensfun() -> MutexGuard<'static, Option<Database>> {
    // A Rust panic never interrupts a Lensfun call, so Lensfun's state is
    // intact when a holder panicked.
    DATABASE.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) struct Database(pub(super) *mut lfDatabase);

// SAFETY: the database is only reached through `DATABASE`'s lock, so one
// thread at a time calls Lensfun with it.
unsafe impl Send for Database {}

impl Database {
    fn create() -> Result<Self> {
        let pointer = unsafe { lf_db_create() };
        if pointer.is_null() {
            Err(anyhow!("Lensfun could not allocate a database"))
        } else {
            Ok(Self(pointer))
        }
    }

    fn load() -> Result<Self> {
        for candidate in database_candidates() {
            if !candidate.exists() {
                continue;
            }
            let database = Self::create()?;
            if load_database_directory(&database, &candidate) {
                log::info!(
                    "loaded bundled Lensfun database from {}",
                    candidate.display()
                );
                return Ok(database);
            }
        }

        let database = Self::create()?;
        let result = unsafe { lf_db_load(database.0) };
        if result != LF_NO_ERROR {
            return Err(anyhow!("Lensfun database load failed with code {result}"));
        }
        Ok(database)
    }
}

fn load_database_directory(database: &Database, directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut files = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
        })
        .collect::<Vec<_>>();
    files.sort();
    if files.is_empty() {
        return false;
    }

    for file in files {
        let Ok(filename) = CString::new(file.to_string_lossy().as_bytes()) else {
            return false;
        };
        let result = unsafe { lf_db_load_path(database.0, filename.as_ptr()) };
        if result != LF_NO_ERROR {
            log::warn!(
                "Lensfun could not load bundled database file {} (code {result})",
                file.display()
            );
            return false;
        }
    }
    true
}

fn database_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(configured) = std::env::var_os("CALIBRAW_LENSFUN_DB") {
        roots.push(PathBuf::from(configured));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            roots.push(directory.join("lensfun"));
            roots.push(directory.join("../share/calibraw/lensfun"));
            roots.push(directory.join("../share/lensfun"));
        }
    }

    let mut candidates = Vec::new();
    for root in roots {
        push_database_candidates(&mut candidates, &root);
    }
    candidates
}

fn push_database_candidates(candidates: &mut Vec<PathBuf>, root: &Path) {
    for candidate in [
        root.join("version_2"),
        root.join("version_1"),
        root.to_owned(),
    ] {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        unsafe { lf_db_destroy(self.0) };
    }
}

pub(super) struct Modifier(pub(super) *mut lfModifier);

// SAFETY: once its corrections are enabled, CalibRaw only calls Lensfun's
// `lf_modifier_apply_*` functions through a shared `&Modifier`. They read the
// modifier's precomputed callbacks and write only to the caller's buffer, so
// concurrent calls on one modifier are safe (darktable applies one modifier
// from parallel loops the same way). Configuration (`lf_modifier_enable_*`)
// takes the raw pointer before the modifier is shared.
unsafe impl Sync for Modifier {}

impl Drop for Modifier {
    fn drop(&mut self) {
        unsafe { lf_modifier_destroy(self.0) };
    }
}

pub(super) struct OwnedPointerList<T> {
    pub(super) pointer: *mut *const T,
}

impl<T> OwnedPointerList<T> {
    pub(super) fn first(&self) -> Option<*const T> {
        if self.pointer.is_null() {
            None
        } else {
            let first = unsafe { *self.pointer };
            (!first.is_null()).then_some(first)
        }
    }

    pub(super) fn values(&self) -> Vec<*const T> {
        pointer_list(self.pointer.cast_const())
    }
}

impl<T> Drop for OwnedPointerList<T> {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe { lf_free(self.pointer.cast()) };
        }
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(super) struct AlignedRgba(pub(super) [f32; 4]);
