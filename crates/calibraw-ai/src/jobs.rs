//! The worker thread every AI job runs on: it reports downloads and progress,
//! turns a panic inside ONNX Runtime into an error, always ends with
//! `AiJobEvent::Finished` and records how long the job took in
//! `diagnostics` ("AI job calibraw-onnx-mask finished in 18.81 s").

use crate::registry::install::{ModelDownloadProgress, ModelInstallSpec};
use crate::registry::ModelSpec;
use anyhow::Result;
use std::{
    panic::AssertUnwindSafe,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

/// What a running AI job reports: `T` is its result, `P` its own progress.
#[derive(Debug)]
pub enum AiJobEvent<T, P> {
    DownloadProgress(ModelDownloadProgress),
    Progress(P),
    /// The last event. An error carries its whole chain of causes.
    Finished(Result<T, String>),
}

/// How a kind of job names itself in its thread and its errors.
pub(crate) struct JobKind {
    pub(crate) thread: &'static str,
    /// Completes "could not start {worker}".
    pub(crate) worker: &'static str,
    /// The error a panic becomes, followed by the panic's message.
    pub(crate) panicked: &'static str,
    /// The error a cancelled job ends with.
    pub(crate) cancelled: &'static str,
}

/// A running job's side of its event channel.
pub(crate) struct JobContext<T, P> {
    kind: &'static JobKind,
    events: mpsc::Sender<AiJobEvent<T, P>>,
    cancellation: Arc<AtomicBool>,
}

impl<T, P> JobContext<T, P> {
    pub(crate) fn ensure_not_cancelled(&self) -> Result<()> {
        anyhow::ensure!(
            !self.cancellation.load(Ordering::Acquire),
            "{}",
            self.kind.cancelled
        );
        Ok(())
    }

    pub(crate) fn cancellation(&self) -> &AtomicBool {
        &self.cancellation
    }

    pub(crate) fn progress(&self, progress: P) {
        let _ = self.events.send(AiJobEvent::Progress(progress));
    }

    /// Makes sure a verified copy of `spec`'s model is at `path`, downloading
    /// it when allowed and stopping when the job is cancelled.
    pub(crate) fn install(
        &self,
        spec: &ModelSpec,
        path: &Path,
        allow_download: bool,
    ) -> Result<()> {
        spec.ensure_installed(
            path,
            allow_download,
            |progress| self.download_progress(progress),
            || self.ensure_not_cancelled(),
        )
    }

    /// `install` for a package rather than a single model.
    pub(crate) fn install_package(&self, package: &ModelInstallSpec, path: &Path) -> Result<()> {
        package.ensure_installed(
            path,
            true,
            |progress| self.download_progress(progress),
            || self.ensure_not_cancelled(),
        )
    }

    fn download_progress(&self, progress: ModelDownloadProgress) {
        let _ = self.events.send(AiJobEvent::DownloadProgress(progress));
    }
}

/// Runs `run` on a new thread and returns its events. The receiver always
/// gets `Finished`, also when the thread cannot start or `run` panics.
pub(crate) fn spawn_job<T, P>(
    kind: &'static JobKind,
    cancellation: Arc<AtomicBool>,
    run: impl FnOnce(&JobContext<T, P>) -> Result<T> + Send + 'static,
) -> mpsc::Receiver<AiJobEvent<T, P>>
where
    T: Send + 'static,
    P: Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let context = JobContext {
        kind,
        events: sender.clone(),
        cancellation,
    };
    let spawn = std::thread::Builder::new()
        .name(kind.thread.to_owned())
        .spawn(move || {
            let started = Instant::now();
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| run(&context)))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown ONNX Runtime failure");
                    Err(anyhow::anyhow!("{}: {message}", kind.panicked))
                });
            let seconds = started.elapsed().as_secs_f64();
            calibraw_core::diagnostics::record(match &result {
                Ok(_) => format!("AI job {} finished in {seconds:.2} s", kind.thread),
                Err(error) => format!(
                    "AI job {} failed after {seconds:.2} s: {error:#}",
                    kind.thread
                ),
            });
            let _ = context.events.send(AiJobEvent::Finished(
                result.map_err(|error| format!("{error:#}")),
            ));
        });
    if let Err(error) = spawn {
        let _ = sender.send(AiJobEvent::Finished(Err(format!(
            "could not start {}: {error}",
            kind.worker
        ))));
    }
    receiver
}

#[cfg(test)]
mod tests {
    use super::{spawn_job, AiJobEvent, JobKind};
    use std::sync::{atomic::AtomicBool, Arc};

    const TEST_JOB: JobKind = JobKind {
        thread: "calibraw-test-job",
        worker: "test worker",
        panicked: "ONNX Runtime terminated the test job",
        cancelled: "test job cancelled",
    };

    fn finish(receiver: std::sync::mpsc::Receiver<AiJobEvent<u32, u32>>) -> Vec<String> {
        receiver
            .into_iter()
            .map(|event| match event {
                AiJobEvent::DownloadProgress(progress) => format!("download {}", progress.label),
                AiJobEvent::Progress(step) => format!("progress {step}"),
                AiJobEvent::Finished(Ok(value)) => format!("ok {value}"),
                AiJobEvent::Finished(Err(error)) => format!("err {error}"),
            })
            .collect()
    }

    #[test]
    fn a_job_reports_progress_then_finishes() {
        let receiver = spawn_job(&TEST_JOB, Arc::new(AtomicBool::new(false)), |job| {
            job.progress(1);
            job.ensure_not_cancelled()?;
            Ok(7)
        });
        assert_eq!(finish(receiver), ["progress 1", "ok 7"]);
        assert!(
            calibraw_core::diagnostics::snapshot().contains("AI job calibraw-test-job finished in")
        );
    }

    #[test]
    fn a_panic_finishes_the_job_with_an_error() {
        let receiver = spawn_job::<u32, u32>(&TEST_JOB, Arc::new(AtomicBool::new(false)), |_| {
            panic!("kernel failed")
        });
        assert_eq!(
            finish(receiver),
            ["err ONNX Runtime terminated the test job: kernel failed"]
        );
    }

    #[test]
    fn a_cancelled_job_ends_with_its_cancellation_error() {
        let receiver = spawn_job::<u32, u32>(&TEST_JOB, Arc::new(AtomicBool::new(true)), |job| {
            job.ensure_not_cancelled()?;
            Ok(1)
        });
        assert_eq!(finish(receiver), ["err test job cancelled"]);
    }
}
