use super::job_state::ForegroundProgress;
use crate::platform::Platform;
use calibraw_ai::{AiJobEvent, ModelDownloadProgress};
use eframe::egui;
use std::sync::mpsc;

pub(crate) fn spawn_ui_worker<T, F>(context: &egui::Context, work: F) -> mpsc::Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let context = context.clone();
    std::thread::spawn(move || {
        let result = work();
        let _ = sender.send(result);
        context.request_repaint();
    });
    receiver
}

pub(crate) fn drain_worker_events<T>(
    receiver: Option<&mpsc::Receiver<T>>,
    is_terminal: impl Fn(&T) -> bool,
) -> (Vec<T>, bool) {
    let mut events = Vec::new();
    let mut disconnected = false;
    let Some(receiver) = receiver else {
        return (events, disconnected);
    };
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                let terminal = is_terminal(&event);
                events.push(event);
                if terminal {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }
    (events, disconnected)
}

/// An AI job's report other than its result.
pub(crate) enum AiJobUpdate<P> {
    Download(ModelDownloadProgress),
    Progress(P),
}

/// Passes the AI job's reports since the last frame to `update`, in order,
/// and returns its result once it finished. A job whose worker stopped
/// without a result finishes with the error `stopped` makes.
///
/// A finished job has dropped its session and buffers, so the memory they
/// held is returned to the system through `platform`.
pub(crate) fn poll_ai_job<T, P>(
    platform: &dyn Platform,
    receiver: &mpsc::Receiver<AiJobEvent<T, P>>,
    stopped: impl FnOnce() -> String,
    mut update: impl FnMut(AiJobUpdate<P>),
) -> Option<Result<T, String>> {
    let (events, disconnected) = drain_worker_events(Some(receiver), |event| {
        matches!(event, AiJobEvent::Finished(_))
    });
    let mut finished = None;
    for event in events {
        match event {
            AiJobEvent::DownloadProgress(progress) => update(AiJobUpdate::Download(progress)),
            AiJobEvent::Progress(progress) => update(AiJobUpdate::Progress(progress)),
            AiJobEvent::Finished(result) => finished = Some(result),
        }
    }
    if finished.is_none() && disconnected {
        finished = Some(Err(stopped()));
    }
    if finished.is_some() {
        platform.release_free_memory();
    }
    finished
}

/// Progress of a model download, in megabytes.
pub(crate) fn model_download_progress(progress: ModelDownloadProgress) -> ForegroundProgress {
    ForegroundProgress::units(
        progress.downloaded,
        progress.total,
        Some("bytes".to_owned()),
        format!("Downloading {}", progress.label),
    )
    .with_detail(megabytes_detail(progress))
}

/// "downloaded / total MB".
pub(crate) fn megabytes_detail(progress: ModelDownloadProgress) -> String {
    format!(
        "{:.1} / {:.1} MB",
        progress.downloaded as f64 / 1_000_000.0,
        progress.total as f64 / 1_000_000.0
    )
}

#[cfg(test)]
mod tests {
    use super::{drain_worker_events, poll_ai_job};
    use crate::platform::fake::FakePlatform;
    use crate::platform::PlatformCaps;
    use calibraw_ai::AiJobEvent;
    use std::sync::mpsc;

    #[test]
    fn a_finished_ai_job_returns_freed_memory() {
        let platform = FakePlatform::new(PlatformCaps::ANDROID);
        let (sender, receiver) = mpsc::channel::<AiJobEvent<u8, u8>>();
        let poll = || poll_ai_job(platform.as_ref(), &receiver, String::new, |_| {});
        sender.send(AiJobEvent::Progress(1)).unwrap();
        assert!(poll().is_none());
        assert!(platform.calls().is_empty());

        sender.send(AiJobEvent::Finished(Ok(7))).unwrap();
        assert_eq!(poll(), Some(Ok(7)));
        assert_eq!(platform.calls(), ["release_free_memory"]);

        // A worker that died without a result has dropped its buffers too.
        drop(sender);
        assert_eq!(poll(), Some(Err(String::new())));
        assert_eq!(
            platform.calls(),
            ["release_free_memory", "release_free_memory"]
        );
    }

    #[test]
    fn draining_stops_after_a_terminal_event_and_keeps_later_ones() {
        let (sender, receiver) = mpsc::channel();
        for event in [1, 2, 99, 3] {
            sender.send(event).unwrap();
        }
        let (events, disconnected) = drain_worker_events(Some(&receiver), |event| *event == 99);
        assert_eq!(events, [1, 2, 99]);
        assert!(!disconnected);
        assert_eq!(receiver.try_recv(), Ok(3));
    }

    #[test]
    fn a_dead_worker_is_reported_after_its_queued_events() {
        let (sender, receiver) = mpsc::channel();
        sender.send(1).unwrap();
        drop(sender);
        let (events, disconnected) = drain_worker_events(Some(&receiver), |_| false);
        assert_eq!(events, [1]);
        assert!(disconnected);

        // A worker that sent its terminal event before exiting is not reported.
        let (sender, receiver) = mpsc::channel();
        sender.send(99).unwrap();
        drop(sender);
        let (events, disconnected) = drain_worker_events(Some(&receiver), |event| *event == 99);
        assert_eq!(events, [99]);
        assert!(!disconnected);
    }

    #[test]
    fn no_receiver_means_no_events() {
        let (events, disconnected) = drain_worker_events::<u8>(None, |_| true);
        assert!(events.is_empty());
        assert!(!disconnected);
    }
}
