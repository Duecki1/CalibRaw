//! A file naming the AI job that is running on the GPU, so the next start can
//! tell when a GPU failure ended the process mid-job: a driver reset or a
//! native abort inside an execution provider leaves no error to handle.
//!
//! Model sessions run one at a time (`slot`), so one record suffices.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
};

static RECORD_PATH: OnceLock<PathBuf> = OnceLock::new();
/// Guards the record file as well as the in-memory state, so ending the
/// records cannot interleave with a job writing one.
static RECORDS: Mutex<Records> = Mutex::new(Records {
    running_job: None,
    ended: false,
});

struct Records {
    running_job: Option<String>,
    /// Set by `end_gpu_job_records`; later jobs are not recorded.
    ended: bool,
}

impl Records {
    fn begin(&mut self, path: Option<&Path>, job: String) {
        if self.ended {
            return;
        }
        if let Some(path) = path {
            write_record(path, &job);
        }
        self.running_job = Some(job);
    }

    fn forget_running_job(&mut self, path: Option<&Path>) {
        self.running_job = None;
        if let Some(path) = path {
            remove_record(path);
        }
    }

    fn end(&mut self, path: Option<&Path>) {
        self.ended = true;
        self.forget_running_job(path);
    }
}

/// Keeps the record at `path` and returns the job a previous run left in it,
/// i.e. the GPU job that run did not survive. Call once at startup, before
/// any AI job. Without a path, jobs are only tracked in memory.
pub(crate) fn set_gpu_job_record_path(path: PathBuf) -> Option<String> {
    let interrupted = take_record(&path);
    if RECORD_PATH.set(path).is_err() {
        log::warn!("the GPU AI job record path was already set");
    }
    interrupted
}

/// The AI job running on the GPU in this process, as "model on provider".
pub fn gpu_job_in_progress() -> Option<String> {
    records().running_job.clone()
}

/// Forgets the running job and records no later ones, for an orderly exit or
/// a restart: quitting during a job is not a GPU failure, and a job that
/// keeps running until the process ends must not leave a record behind.
pub fn end_gpu_job_records() {
    records().end(record_path());
}

/// Records a GPU job for as long as it lives.
pub(crate) struct GpuJobRecord(());

impl GpuJobRecord {
    pub(crate) fn begin(model_name: &str, provider: &str) -> Self {
        records().begin(record_path(), format!("{model_name} on {provider}"));
        Self(())
    }
}

impl Drop for GpuJobRecord {
    fn drop(&mut self) {
        records().forget_running_job(record_path());
    }
}

fn record_path() -> Option<&'static Path> {
    RECORD_PATH.get().map(PathBuf::as_path)
}

fn records() -> MutexGuard<'static, Records> {
    RECORDS
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
    use super::{remove_record, take_record, write_record, Records};

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

    /// A job still running when CalibRaw exits, or one that starts after,
    /// is not a GPU failure the next start should report.
    #[test]
    fn jobs_after_the_end_leave_no_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gpu-ai-job");
        let mut records = Records {
            running_job: None,
            ended: false,
        };
        records.begin(Some(&path), "BiRefNet on DirectML".to_owned());
        assert!(path.exists());
        assert_eq!(records.running_job.as_deref(), Some("BiRefNet on DirectML"));

        records.end(Some(&path));
        assert!(!path.exists());
        records.begin(Some(&path), "BiRefNet on DirectML".to_owned());
        assert!(!path.exists());
        assert_eq!(records.running_job, None);
    }
}
