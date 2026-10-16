//! A file naming the AI job that is running on the GPU, so the next start can
//! tell when a GPU failure ended the process mid-job: a driver reset or a
//! native abort inside an execution provider leaves no error to handle.
//!
//! Model sessions run one at a time (`model_runtime`), so one record suffices.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
};

static RECORD_PATH: OnceLock<PathBuf> = OnceLock::new();
static RUNNING_JOB: Mutex<Option<String>> = Mutex::new(None);

/// Keeps the record at `path` and returns the job a previous run left in it,
/// i.e. the GPU job that run did not survive. Call once at startup, before
/// any AI job. Without a path, jobs are only tracked in memory.
pub fn set_gpu_job_record_path(path: PathBuf) -> Option<String> {
    let interrupted = take_record(&path);
    if RECORD_PATH.set(path).is_err() {
        log::warn!("the GPU AI job record path was already set");
    }
    interrupted
}

/// The AI job running on the GPU in this process, as "model on provider".
pub fn gpu_job_in_progress() -> Option<String> {
    running_job().clone()
}

/// Forgets the running job on an orderly exit: quitting during a job is not a
/// GPU failure.
pub fn clear_gpu_job_record() {
    *running_job() = None;
    if let Some(path) = RECORD_PATH.get() {
        remove_record(path);
    }
}

/// Records a GPU job for as long as it lives.
pub(crate) struct GpuJobRecord(());

impl GpuJobRecord {
    pub(crate) fn begin(model_name: &str, provider: &str) -> Self {
        let job = format!("{model_name} on {provider}");
        if let Some(path) = RECORD_PATH.get() {
            write_record(path, &job);
        }
        *running_job() = Some(job);
        Self(())
    }
}

impl Drop for GpuJobRecord {
    fn drop(&mut self) {
        clear_gpu_job_record();
    }
}

fn running_job() -> MutexGuard<'static, Option<String>> {
    RUNNING_JOB
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_record(path: &Path, job: &str) {
    if let Err(error) = fs::write(path, job) {
        log::warn!(
            "could not write the GPU AI job record {}: {error}",
            path.display()
        );
    }
}

/// The job recorded at `path`, removing the record.
fn take_record(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(job) => {
            remove_record(path);
            Some(job.trim().to_owned()).filter(|job| !job.is_empty())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            log::warn!(
                "could not read the GPU AI job record {}: {error}",
                path.display()
            );
            remove_record(path);
            None
        }
    }
}

fn remove_record(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "could not remove the GPU AI job record {}: {error}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_record, take_record, write_record};

    #[test]
    fn a_record_left_behind_is_reported_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gpu-ai-job");
        assert_eq!(take_record(&path), None);

        write_record(&path, "BiRefNet on DirectML");
        assert_eq!(take_record(&path).as_deref(), Some("BiRefNet on DirectML"));
        assert!(!path.exists());
        assert_eq!(take_record(&path), None);
    }

    #[test]
    fn a_finished_job_leaves_no_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gpu-ai-job");
        write_record(&path, "SkySeg U2Net on CUDA");
        remove_record(&path);
        assert_eq!(take_record(&path), None);

        std::fs::write(&path, " \n").unwrap();
        assert_eq!(take_record(&path), None);
    }
}
