//! A file naming the AI work on the GPU, so the next start can tell when a
//! GPU failure ended the process during it: a driver reset or a native abort
//! inside an execution provider leaves no error to handle. It names a running
//! job, or else a model whose GPU session is loaded between jobs, whose
//! memory alone can push the GPU into paging and a driver reset.
//!
//! Model sessions live in one slot and run one at a time (`slot`), so one
//! record suffices.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
};

static RECORD_PATH: OnceLock<PathBuf> = OnceLock::new();
/// Guards the record file as well as the in-memory state, so ending the
/// records cannot interleave with a job writing one.
static RECORDS: Mutex<Records> = Mutex::new(Records::new());

/// AI work on the GPU, as "model on provider".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuAiActivity {
    /// A job ran, or a session was being created.
    Job(String),
    /// A model's session was loaded between its jobs.
    IdleModel(String),
}

/// The second line of the record of an idle model. A record without it, as
/// every record before idle models were recorded, names a job.
const IDLE_MARKER: &str = "idle";

impl GpuAiActivity {
    fn record_text(&self) -> String {
        match self {
            Self::Job(job) => job.clone(),
            Self::IdleModel(model) => format!("{model}\n{IDLE_MARKER}"),
        }
    }

    fn of_record_text(text: &str) -> Option<Self> {
        let mut lines = text.lines().map(str::trim);
        let work = lines.next().filter(|work| !work.is_empty())?.to_owned();
        Some(if lines.next() == Some(IDLE_MARKER) {
            Self::IdleModel(work)
        } else {
            Self::Job(work)
        })
    }
}

struct Records {
    running_job: Option<String>,
    loaded_model: Option<String>,
    /// What the file says, so the same is not written again.
    written: Option<GpuAiActivity>,
    /// Set by `end_gpu_job_records`; later work is not recorded.
    ended: bool,
}

impl Records {
    const fn new() -> Self {
        Self {
            running_job: None,
            loaded_model: None,
            written: None,
            ended: false,
        }
    }

    /// The work to record: a running job before a loaded model.
    fn activity(&self) -> Option<GpuAiActivity> {
        match (&self.running_job, &self.loaded_model) {
            (Some(job), _) => Some(GpuAiActivity::Job(job.clone())),
            (None, Some(model)) => Some(GpuAiActivity::IdleModel(model.clone())),
            (None, None) => None,
        }
    }

    /// Brings the file at `path` in line with the work to record.
    fn sync(&mut self, path: Option<&Path>) {
        let activity = self.activity();
        if activity == self.written {
            return;
        }
        if let Some(path) = path {
            match &activity {
                Some(activity) => write_record(path, activity),
                None => remove_record(path),
            }
        }
        self.written = activity;
    }

    fn begin_job(&mut self, path: Option<&Path>, job: String) {
        if !self.ended {
            self.running_job = Some(job);
            self.sync(path);
        }
    }

    fn forget_running_job(&mut self, path: Option<&Path>) {
        self.running_job = None;
        self.sync(path);
    }

    fn load_model(&mut self, path: Option<&Path>, model: String) {
        if !self.ended {
            self.loaded_model = Some(model);
            self.sync(path);
        }
    }

    fn forget_loaded_model(&mut self, path: Option<&Path>) {
        self.loaded_model = None;
        self.sync(path);
    }

    fn end(&mut self, path: Option<&Path>) {
        self.ended = true;
        self.running_job = None;
        self.loaded_model = None;
        self.sync(path);
    }
}

/// Keeps the record at `path` and returns the work a previous run left in
/// it, i.e. the GPU work that run did not survive. Call once at startup,
/// before any AI job. Without a path, work is only tracked in memory.
pub(crate) fn set_gpu_job_record_path(path: PathBuf) -> Option<GpuAiActivity> {
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

/// The AI work on the GPU in this process: a running job, or else a model
/// loaded there between jobs.
pub fn gpu_ai_activity() -> Option<GpuAiActivity> {
    records().activity()
}

/// Forgets the running job and loaded model and records no later work, for
/// an orderly exit or a restart: quitting during a job or with a model loaded
/// is not a GPU failure, and work that goes on until the process ends must
/// not leave a record behind.
pub fn end_gpu_job_records() {
    records().end(record_path());
}

/// Records a GPU job for as long as it lives.
pub(crate) struct GpuJobRecord(());

impl GpuJobRecord {
    pub(crate) fn begin(model_name: &str, provider: &str) -> Self {
        records().begin_job(record_path(), format!("{model_name} on {provider}"));
        Self(())
    }
}

impl Drop for GpuJobRecord {
    fn drop(&mut self) {
        records().forget_running_job(record_path());
    }
}

/// Records a model's GPU session for as long as it is loaded.
pub(crate) struct GpuSessionRecord(());

impl GpuSessionRecord {
    pub(crate) fn begin(model_name: &str, provider: &str) -> Self {
        records().load_model(record_path(), format!("{model_name} on {provider}"));
        Self(())
    }
}

impl Drop for GpuSessionRecord {
    fn drop(&mut self) {
        records().forget_loaded_model(record_path());
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

/// Writes the record through to disk, so that a failure taking the system
/// down keeps it, and creates its folder, which on Windows only exists once
/// a thumbnail has been cached.
fn write_record(path: &Path, activity: &GpuAiActivity) {
    let text = activity.record_text();
    if let Err(error) = calibraw_core::file_ops::write_bytes_atomically(path, text.as_bytes()) {
        log::warn!(
            "could not write the GPU AI job record {}: {error}",
            path.display()
        );
    }
}

/// The work recorded at `path`, removing the record.
fn take_record(path: &Path) -> Option<GpuAiActivity> {
    match fs::read_to_string(path) {
        Ok(text) => {
            remove_record(path);
            GpuAiActivity::of_record_text(&text)
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
