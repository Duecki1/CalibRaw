//! The AI denoise an export needs, installed before it renders. A photo
//! exported from the Library is not open, and an open one may have its
//! result missing, so the export restores the saved result or runs RawNIND
//! itself instead of failing.

use super::*;
use crate::i18n::tr;
use calibraw_ai::models::denoise::{AiDenoiseProgress, RawNindJob};
use calibraw_ai::AiJobEvent;
use std::sync::atomic::Ordering;

/// Whether `raw` must be denoised before `exposure` can render. Rendered
/// photos have no sensor data for RawNIND and export without AI denoise, as
/// they show in Develop.
pub(super) fn export_needs_ai_denoise(raw: &LoadedRaw, exposure: &ExposureParams) -> bool {
    exposure.ai_denoise_enabled
        && !raw.is_pre_demosaiced_raster()
        && raw.ai_denoised_image().is_none()
}

/// Installs the AI denoise `raw` needs: the result saved at `saved_result`
/// when it still matches the RAW, otherwise a new RawNIND run whose result is
/// saved there. Exports run without asking, so the models must already be
/// installed; nothing is downloaded.
pub(super) fn install_export_ai_denoise(
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
) -> Result<crate::pipeline::AiDenoisedImage, String> {
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
                // The export still uses the result; the next open runs it again.
                log::warn!("AI-denoise result of an export was not saved: {error}");
            }
            AiJobEvent::Progress(AiDenoiseProgress::SavedResultUnusable)
            | AiJobEvent::DownloadProgress(_) => {}
            AiJobEvent::Finished(result) => return result,
        }
    }
    Err(tr("RawNIND worker stopped unexpectedly.").to_owned())
}
