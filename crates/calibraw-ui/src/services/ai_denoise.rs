//! RawNIND AI denoise of a photo that is not open in Develop: an export of a
//! Library photo, or a merge's sources. Runs the model on the caller's worker
//! thread. It downloads the models only when the caller says the user accepted
//! that through the consent dialog (`CalibRawApp::request_ai_consent`), which
//! the caller asks before it starts; the ONNX Runtime downloads whenever a job
//! loads it, so that question covers it too.

use crate::i18n::tr;
use crate::pipeline::{AiDenoisedImage, LoadedRaw};
use calibraw_ai::models::denoise::{AiDenoiseProgress, RawNindJob};
use calibraw_ai::{AiJobEvent, ModelDownloadProgress};
use calibraw_gpu::wgpu;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How far [`install_ai_denoise`] has got.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AiDenoiseStatus {
    /// Downloading the RawNIND models, which the caller allowed.
    Downloading(ModelDownloadProgress),
    /// Denoising: `completed` of `total` model tiles; a total of 0 while it
    /// prepares.
    Denoising { completed: usize, total: usize },
}

/// Installs an AI-denoised image on `raw`. With `saved_result`, the result
/// saved there is restored when it still matches `raw`, and otherwise a new
/// RawNIND result is saved there. A restore that does not match deletes the
/// file, so `saved_result` is only for the photo's own source, prepared as its
/// edit prepares it. Without it RawNIND runs and the result lasts only as long
/// as `raw`. `allow_model_download` downloads missing models, which the user
/// must have accepted; without it missing models fail the job.
pub(crate) fn install_ai_denoise(
    raw: &Arc<LoadedRaw>,
    saved_result: Option<&Path>,
    allow_model_download: bool,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cancellation: &Arc<AtomicBool>,
    mut progress: impl FnMut(AiDenoiseStatus),
) -> Result<(), String> {
    let mut run = |job| run_rawnind(raw, job, device, queue, cancellation, &mut progress);
    let restored = match saved_result.filter(|path| path.is_file()) {
        Some(path) => run(RawNindJob::Restore {
            path: path.to_owned(),
        })
        .ok(),
        None => None,
    };
    let image = match restored {
        Some(image) => image,
        None if cancellation.load(Ordering::Acquire) => {
            return Err("AI denoise cancelled".to_owned());
        }
        None => run(RawNindJob::Infer {
            save_to: saved_result.map(Path::to_owned),
            allow_model_download,
        })?,
    };
    raw.set_ai_denoised_image(image)
        .map_err(|error| format!("{error:#}"))
}

fn run_rawnind(
    raw: &Arc<LoadedRaw>,
    job: RawNindJob,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cancellation: &Arc<AtomicBool>,
    progress: &mut impl FnMut(AiDenoiseStatus),
) -> Result<AiDenoisedImage, String> {
    let receiver = calibraw_ai::models::denoise::spawn_rawnind_denoise(
        calibraw_ai::models::denoise::model_cache_dir(),
        Arc::clone(raw),
        Some(device.clone()),
        Some(queue.clone()),
        job,
        Arc::clone(cancellation),
    );
    while let Ok(event) = receiver.recv() {
        match event {
            AiJobEvent::Progress(AiDenoiseProgress::Phase {
                completed, total, ..
            }) => progress(AiDenoiseStatus::Denoising { completed, total }),
            AiJobEvent::DownloadProgress(download) => {
                progress(AiDenoiseStatus::Downloading(download));
            }
            AiJobEvent::Progress(AiDenoiseProgress::ResultNotSaved(error)) => {
                // The caller still uses the result; the next open runs it again.
                log::warn!("AI-denoise result was not saved: {error}");
            }
            AiJobEvent::Progress(AiDenoiseProgress::SavedResultUnusable) => {}
            AiJobEvent::Finished(result) => return result,
        }
    }
    Err(tr("RawNIND worker stopped unexpectedly.").to_owned())
}
