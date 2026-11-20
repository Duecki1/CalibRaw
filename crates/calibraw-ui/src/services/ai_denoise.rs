//! RawNIND AI denoise of a photo that is not open in Develop: an export of a
//! Library photo, or a merge's sources. Runs the model on the caller's worker
//! thread, without asking, so the models must already be installed; nothing is
//! downloaded.

use crate::i18n::tr;
use crate::pipeline::{AiDenoisedImage, LoadedRaw};
use calibraw_ai::models::denoise::{AiDenoiseProgress, RawNindJob};
use calibraw_ai::AiJobEvent;
use calibraw_gpu::wgpu;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Whether the RawNIND models are installed, which [`install_ai_denoise`]
/// needs. Hashes the model files, which takes seconds, so call it on a worker.
pub(crate) fn ai_denoise_models_installed() -> bool {
    calibraw_ai::models::denoise::models_are_verified(
        &calibraw_ai::models::denoise::model_cache_dir(),
    )
}

/// Installs an AI-denoised image on `raw`. With `saved_result`, the result
/// saved there is restored when it still matches `raw`, and otherwise a new
/// RawNIND result is saved there. A restore that does not match deletes the
/// file, so `saved_result` is only for the photo's own source, prepared as its
/// edit prepares it. Without it RawNIND runs and the result lasts only as long
/// as `raw`. `progress` gets the completed and total model tiles.
pub(crate) fn install_ai_denoise(
    raw: &Arc<LoadedRaw>,
    saved_result: Option<&Path>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cancellation: &Arc<AtomicBool>,
    mut progress: impl FnMut(usize, usize),
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
            allow_model_download: false,
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
    progress: &mut impl FnMut(usize, usize),
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
            }) => progress(completed, total),
            AiJobEvent::Progress(AiDenoiseProgress::ResultNotSaved(error)) => {
                // The caller still uses the result; the next open runs it again.
                log::warn!("AI-denoise result was not saved: {error}");
            }
            AiJobEvent::Progress(AiDenoiseProgress::SavedResultUnusable)
            | AiJobEvent::DownloadProgress(_) => {}
            AiJobEvent::Finished(result) => return result,
        }
    }
    Err(tr("RawNIND worker stopped unexpectedly.").to_owned())
}
