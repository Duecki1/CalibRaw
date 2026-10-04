//! Owned Lensfun database, modifier and pointer-list handles.

use super::*;

pub(super) struct Database(pub(super) *mut lfDatabase);

impl Database {
    fn create() -> Result<Self> {
        let pointer = unsafe { lf_db_new() };
        if pointer.is_null() {
            Err(anyhow!("Lensfun could not allocate a database"))
        } else {
            Ok(Self(pointer))
        }
    }

    pub(super) fn load() -> Result<Self> {
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
        let result = unsafe { lf_db_load_file(database.0, filename.as_ptr()) };
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
