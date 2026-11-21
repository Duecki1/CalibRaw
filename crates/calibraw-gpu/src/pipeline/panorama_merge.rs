//! Panorama merge of RAW frames into a floating-point master, or of rendered photos into a
//! 16-bit one ([`MergeSourceKind::master_samples`]).
//!
//! Each frame is developed like every merge source ([`SourceDeveloper`]), handed to the
//! stitcher ([`PanoramaBuilder`]) and staged for compositing: in memory while the
//! frames fit half of the device's intermediate allowance, otherwise in a file beside the
//! output, mapped back. The panorama is then composited strip by strip straight into the
//! TIFF, so it never has to fit in memory whole.

use super::export::{intermediate_allowance, linear_rec2020_icc};
use super::raw_merge::{
    ensure_distinct_sources, integer_samples, MasterSamples, MergeCamera, MergeOptions,
    MergeSourceKind, RasterSources, SourceDenoiser, SourceDeveloper,
};
use super::GpuProgramPrewarm;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::hdr::exposure_scale;
use calibraw_core::pipeline::panorama::{
    FrameStore, PanoramaBuilder, PanoramaFrame, PanoramaLayout, PixelRect, ProjectionChoice,
};
use calibraw_core::pipeline::{
    load_focal_length_35mm, load_raw_display_metadata, PerspectiveCorrection,
};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tiff::encoder::{
    colortype::{ColorType, RGB32Float, RGB16},
    TiffEncoder,
};
use tiff::tags::Tag;

/// What a panorama merge chooses beyond the develop options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanoramaOptions {
    pub projection: ProjectionChoice,
}

/// Target bytes per TIFF strip, as the TIFF export writes them.
const TIFF_STRIP_BYTES: usize = 1024 * 1024;

/// Working memory compositing keeps at least, where the device's allowance is smaller
/// or unknown.
const MIN_COMPOSITE_MEMORY: u64 = 256 * 1024 * 1024;

/// A solved panorama with its staged frames, written by [`Self::write_tiff`].
pub struct PanoramaMaster {
    layout: PanoramaLayout,
    frames: StagedFrames,
    camera: MergeCamera,
    composite_memory: u64,
}

impl PanoramaMaster {
    pub fn width(&self) -> u32 {
        self.layout.width()
    }

    pub fn height(&self) -> u32 {
        self.layout.height()
    }

    /// Whether a lens profile corrected the frames: false when lens correction was off
    /// or no profile matched the reference frame.
    pub fn lens_corrected(&self) -> bool {
        self.camera.lens_corrected()
    }

    /// Whether the panorama was reduced below the frames' resolution to stay within the
    /// size CalibRaw opens.
    pub fn reduced(&self) -> bool {
        self.layout.reduced()
    }

    /// Composites the panorama into a linear Rec.2020 TIFF, tagged and sampled like the
    /// other merges' masters ([`MergeSourceKind::master_samples`]). The callback reports
    /// progress and returns false to cancel between parts.
    pub fn write_tiff(
        &self,
        writer: impl Write + Seek,
        progress: impl FnMut(String) -> bool,
    ) -> Result<()> {
        match self.camera.sources().master_samples() {
            MasterSamples::Float => {
                self.write_samples::<RGB32Float>(writer, progress, |strip| strip.to_vec())
            }
            MasterSamples::Integer => {
                self.write_samples::<RGB16>(writer, progress, integer_samples)
            }
        }
    }

    /// [`Self::write_tiff`] in samples of `C`, which `encode` turns working Rec.2020 into.
    fn write_samples<C: ColorType>(
        &self,
        writer: impl Write + Seek,
        progress: impl FnMut(String) -> bool,
        encode: impl Fn(&[f32]) -> Vec<C::Inner>,
    ) -> Result<()>
    where
        [C::Inner]: tiff::encoder::TiffValue,
    {
        let row_values = self.width() as usize * 3;
        // About as large as the TIFF export's strips: libtiff refuses an uncompressed
        // final strip shorter than its rows per strip once a strip exceeds 100 MB.
        let sample_bytes = std::mem::size_of::<C::Inner>();
        let strip_rows =
            (TIFF_STRIP_BYTES / (row_values * sample_bytes)).clamp(1, self.height() as usize);
        let mut encoder = TiffEncoder::new(writer)?;
        let mut image = encoder.new_image::<C>(self.width(), self.height())?;
        image.rows_per_strip(strip_rows as u32)?;
        image
            .encoder()
            .write_tag(Tag::IccProfile, linear_rec2020_icc().as_slice())?;
        // Composited rows waiting for a whole TIFF strip.
        let mut pending: Vec<f32> = Vec::new();
        self.layout.composite(
            &self.frames,
            self.composite_memory,
            |rows| {
                pending.extend_from_slice(rows);
                let whole = pending.len() / (strip_rows * row_values) * strip_rows * row_values;
                self.camera.to_working_rec2020(&mut pending[..whole]);
                for strip in pending[..whole].chunks_exact(strip_rows * row_values) {
                    image.write_strip(&encode(strip))?;
                }
                pending.drain(..whole);
                Ok(())
            },
            progress,
        )?;
        if !pending.is_empty() {
            self.camera.to_working_rec2020(&mut pending);
            image.write_strip(&encode(&pending))?;
        }
        image.finish()?;
        Ok(())
    }
}

/// Develops, registers and lays out the frames at `paths`. Frames are taken in file name
/// order; the middle one is the reference whose white balance and colour every frame
/// shares. `staging_dir` takes the frames that do not fit in memory. `denoise`, when
/// given, AI-denoises every frame. The callback reports progress and returns false to
/// cancel between frames and tiles.
#[allow(clippy::too_many_arguments)]
pub fn merge_panorama(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: &[PathBuf],
    options: MergeOptions,
    panorama: PanoramaOptions,
    denoise: Option<&mut SourceDenoiser>,
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    staging_dir: &Path,
    mut progress: impl FnMut(String) -> bool,
) -> Result<PanoramaMaster> {
    ensure_distinct_sources(paths, "frame")?;
    let mut sorted: Vec<&PathBuf> = paths.iter().collect();
    sorted.sort();
    // The reference is developed first; the rest keep their order.
    let reference = sorted.remove(sorted.len() / 2);
    sorted.insert(0, reference);
    let allowance = intermediate_allowance();
    let mut frames = StagedFrames::new(staging_dir, allowance / 2);
    let mut builder = PanoramaBuilder::new();
    let mut developer = SourceDeveloper::new(
        device,
        queue,
        options,
        denoise,
        program_prewarm,
        RasterSources::Accept,
    );
    let count = sorted.len();
    let mut exposures: Option<Vec<f32>> = None;
    for (index, path) in sorted.iter().enumerate() {
        let step = format!("frame {}/{count}", index + 1);
        let developed = developer.develop(path, &step, &mut progress)?;
        let camera = developer.camera().context("Nothing to merge")?;
        // A raster's brightness is what its camera or editor made of the exposure, so
        // only RAW frames are balanced by their metadata.
        let exposure = exposures.get_or_insert_with(|| match camera.sources() {
            MergeSourceKind::Raw => relative_exposures(&sorted),
            MergeSourceKind::SceneRaster | MergeSourceKind::Rendered => vec![1.0; count],
        })[index];
        ensure!(
            progress(format!("Finding features in {step}")),
            "Merge cancelled"
        );
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        builder.add(PanoramaFrame {
            label: &label,
            width: camera.width,
            height: camera.height,
            rgb: &developed.rgb,
            peak: &developed.peak,
            lens: developed.lens_geometry.clone(),
            exposure,
            // The camera's own 35 mm equivalent where it reports one (phones do),
            // otherwise the focal length scaled by the crop factor Lensfun knows.
            focal_length_35mm: load_focal_length_35mm(path).or_else(|| {
                PerspectiveCorrection::known_focal_length_35mm(
                    developed.focal_length_mm,
                    camera.crop_factor(),
                )
            }),
        })?;
        frames.push(developed.rgb, camera.width as usize)?;
    }
    ensure!(progress("Aligning frames".to_owned()), "Merge cancelled");
    let layout = builder.solve(panorama.projection)?;
    let camera = developer.into_camera().context("Nothing to merge")?;
    Ok(PanoramaMaster {
        layout,
        frames,
        camera,
        composite_memory: (allowance / 2).max(MIN_COMPOSITE_MEMORY),
    })
}

/// Each frame's exposure relative to the first, from its shutter speed, aperture and
/// ISO; every frame counts as equally exposed when any of them lacks that metadata.
fn relative_exposures(paths: &[&PathBuf]) -> Vec<f32> {
    let scales: Option<Vec<f32>> = paths
        .iter()
        .map(|path| {
            let metadata = load_raw_display_metadata(path).ok()?;
            exposure_scale(
                metadata.shutter_seconds,
                metadata.iso_speed,
                metadata.aperture,
            )
            .ok()
        })
        .collect();
    match scales {
        Some(scales) => scales.iter().map(|s| s / scales[0]).collect(),
        None => {
            calibraw_core::diagnostics::record(
                "Panorama: exposure metadata missing; frames are balanced from their overlaps alone",
            );
            vec![1.0; paths.len()]
        }
    }
}

/// The developed frames, held until compositing reads them back.
struct StagedFrames {
    directory: PathBuf,
    /// Bytes of frames that may still be held in memory.
    memory_left: u64,
    frames: Vec<StagedFrame>,
}

struct StagedFrame {
    width: usize,
    pixels: Pixels,
}

enum Pixels {
    Memory(Vec<f32>),
    // Fields drop in order: the mapping before the file is removed.
    Mapped {
        map: memmap2::Mmap,
        _file: StagingFile,
    },
}

impl StagedFrames {
    fn new(directory: &Path, memory: u64) -> Self {
        Self {
            directory: calibraw_core::file_ops::staging_directory(directory),
            memory_left: memory,
            frames: Vec::new(),
        }
    }

    /// Keeps a frame of camera RGB, `width` pixels per row.
    fn push(&mut self, rgb: Vec<f32>, width: usize) -> Result<()> {
        let bytes = (rgb.len() * std::mem::size_of::<f32>()) as u64;
        let pixels = if bytes <= self.memory_left {
            self.memory_left -= bytes;
            Pixels::Memory(rgb)
        } else {
            let file = StagingFile::new(&self.directory)?;
            let mut writer = BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .read(true)
                    .create_new(true)
                    .open(&file.path)
                    .with_context(|| format!("create staged frame {}", file.path.display()))?,
            );
            writer.write_all(bytemuck::cast_slice(&rgb))?;
            let staged = writer
                .into_inner()
                .map_err(|error| error.into_error())
                .context("write staged frame")?;
            drop(rgb);
            // SAFETY: the staged file has a unique name, is completely written above and
            // is never written again; `Pixels::Mapped` drops the mapping before the
            // file is removed.
            let map = unsafe { memmap2::MmapOptions::new().map(&staged) }
                .with_context(|| format!("map staged frame {}", file.path.display()))?;
            Pixels::Mapped { map, _file: file }
        };
        self.frames.push(StagedFrame { width, pixels });
        Ok(())
    }
}

impl FrameStore for StagedFrames {
    fn read(&self, frame: usize, rect: PixelRect, out: &mut Vec<f32>) -> Result<()> {
        let staged = self.frames.get(frame).context("No such panorama frame")?;
        out.clear();
        out.reserve(rect.width as usize * rect.height as usize * 3);
        let row_values = rect.width as usize * 3;
        for y in rect.y as usize..(rect.y + rect.height) as usize {
            let start = (y * staged.width + rect.x as usize) * 3;
            match &staged.pixels {
                Pixels::Memory(rgb) => {
                    out.extend_from_slice(
                        rgb.get(start..start + row_values)
                            .context("Panorama frame read out of bounds")?,
                    );
                }
                Pixels::Mapped { map, .. } => {
                    let bytes = map
                        .get(start * 4..(start + row_values) * 4)
                        .context("Panorama frame read out of bounds")?;
                    // The mapping is page aligned, so rows of `f32` are aligned too.
                    out.extend(
                        bytes
                            .chunks_exact(4)
                            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])),
                    );
                }
            }
        }
        Ok(())
    }
}

/// A uniquely named file in the staging directory, removed on drop.
struct StagingFile {
    path: PathBuf,
}

impl StagingFile {
    fn new(directory: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(directory)
            .with_context(|| format!("create staging directory {}", directory.display()))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(Self {
            path: directory.join(format!(
                ".calibraw-panorama.{}.{nonce}.{}.part",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )),
        })
    }
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
