//! AI parity harness: every published model runs one fixed input from the
//! Demo photos through its public job API and is compared with a golden
//! output stored in `data/ai-parity/`. This is the "same output" proof for
//! restructuring AI code and changing how sessions are placed
//! (docs/GPU_REWORK.md, P0.2).
//!
//! Every test is `#[ignore]`d: it needs the Demo photos, the models and an
//! ONNX Runtime, and some cases need a wgpu adapter. Run all of them with
//!
//! ```sh
//! CALIBRAW_PARITY_PHOTOS=/path/to/Demo \
//!   cargo test --locked -p calibraw-ai --test parity -- --ignored --nocapture
//! ```
//!
//! Environment:
//!
//! - `CALIBRAW_PARITY_PHOTOS` (required): the Demo photo folder. Inputs are
//!   pinned by SHA-256, so a changed photo fails instead of silently moving
//!   the baseline.
//! - `CALIBRAW_PARITY_MODELS`: the model folder; defaults to the desktop model
//!   cache the app downloads into.
//! - `CALIBRAW_PARITY_DOWNLOAD=1`: download missing models into it.
//! - `CALIBRAW_PARITY_RUNTIME`: an ONNX Runtime library to load instead of
//!   the automatic runtime.
//! - `CALIBRAW_PARITY_GPU=1`: keep GPU acceleration on, to compare GPU
//!   sessions with the CPU goldens. Without it every session runs on the CPU.
//! - `CALIBRAW_PARITY_BLESS=1`: write the goldens and their SHA-256 pins from
//!   this run instead of comparing. CPU sessions only.
//!
//! Goldens are CPU outputs at the revision that blessed them, so CPU runs of
//! unchanged code report "bit-identical". The tolerances are for GPU
//! sessions and for float reordering in restructured code. The Remove and
//! X-Trans denoise cases render or demosaic on the GPU before inference, so
//! their goldens are specific to the adapter that blessed them.
//!
//! Golden formats: masks, depth and alpha are 8-bit grayscale PNGs; Bayer
//! denoise output is a 16-bit grayscale PNG of sensor codes; linear RGB is a
//! 16-bit RGB PNG whose samples are IEEE float16 bit patterns, so it
//! round-trips exactly. `SHA256SUMS` pins every golden in `sha256sum` format.

use calibraw_ai::ai_denoise::{spawn_rawnind_denoise, AiDenoiseEvent, RawNindJob};
use calibraw_ai::ai_masks::{
    sha256_file_hex, spawn_ai_mask, spawn_object_mask, AiMaskEvent, AiMaskModel,
    AiMaskWorkerRequest, ObjectInferenceCache, ObjectMaskEvent, ObjectMaskRequest,
    ObjectMaskWorkerRequest, SubjectMaskQuality, SubjectModelPrecision, DEPTH_MODEL,
    SKYSEG_MODEL_FILENAME,
};
use calibraw_ai::pipeline::{
    crop_raw, load_raw_file, CfaKind, ExposureParams, GeometryTransform, LoadedRaw, MaskStack,
    ObjectStroke, RemoveBrushPoint, RemoveBrushStroke, RemoveEditState,
};
use calibraw_ai::remove::{spawn_remove, RemoveEvent, RemoveRequest, BIG_LAMA_MODEL_FILENAME};
use calibraw_gpu::wgpu;
use calibraw_gpu::{request_headless_device, HeadlessDevice, HeadlessDeviceRequest};
use half::f16;
use image::{imageops::FilterType, GrayImage, ImageBuffer, Luma, Rgb, RgbaImage};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};

const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/ai-parity");
const PINS: &str = "SHA256SUMS";

/// A Demo photo, relative to `CALIBRAW_PARITY_PHOTOS`.
struct Photo {
    path: &'static str,
    sha256: &'static str,
}

/// A ship below a cliff under an overcast sky: subject, sky, depth and object
/// prompts all have a clear target.
const SCENE_PHOTO: Photo = Photo {
    path: "Demo3.5/DSC01000.jpg",
    sha256: "a46fff617da3b759054b9821fec1c524171f218aa040cb92faa6b0976f8cb394",
};
const BAYER_RAW: Photo = Photo {
    path: "Demo1/DSC00950.arw",
    sha256: "91762691cba4b6653848eef8deebf30d956bab9b7c44192272fc008ba9e4b5e4",
};
const XTRANS_RAW: Photo = Photo {
    path: "Demo1/DSCF3284.raf",
    sha256: "0fc9d989f900fea0a5ac658f812da7bbd12d4ba70a7206380177b5496bfb1252",
};

/// Long edge of the mask input, about the size of the editor preview the
/// app passes to mask jobs.
const SCENE_EDGE: u32 = 1024;
/// Edge of the RAW crops. A multiple of 6, so crops keep the X-Trans phase,
/// and small enough for a quick CPU run while still spanning several
/// RawNIND tiles' overlap.
const DENOISE_CROP_EDGE: u32 = 384;
const REMOVE_CROP_EDGE: u32 = 768;

// ---------------------------------------------------------------------------
// Cases

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the BiRefNet 768 model and ONNX Runtime"]
fn subject_low_float32() {
    subject_case(
        "subject-low-fp32",
        SubjectMaskQuality::Low,
        SubjectModelPrecision::Float32,
    );
}

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the BiRefNet 768 float16 model and ONNX Runtime"]
fn subject_low_float16() {
    subject_case(
        "subject-low-fp16",
        SubjectMaskQuality::Low,
        SubjectModelPrecision::Float16,
    );
}

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the BiRefNet model and ONNX Runtime"]
fn subject_medium_float32() {
    subject_case(
        "subject-medium-fp32",
        SubjectMaskQuality::Medium,
        SubjectModelPrecision::Float32,
    );
}

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the BiRefNet float16 model and ONNX Runtime"]
fn subject_medium_float16() {
    subject_case(
        "subject-medium-fp16",
        SubjectMaskQuality::Medium,
        SubjectModelPrecision::Float16,
    );
}

/// Medium's model plus the cropped refinement pass, which only High runs.
#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the BiRefNet model and ONNX Runtime"]
fn subject_high_float32() {
    subject_case(
        "subject-high-fp32",
        SubjectMaskQuality::High,
        SubjectModelPrecision::Float32,
    );
}

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the SkySeg model and ONNX Runtime"]
fn sky() {
    let harness = Harness::lock();
    let model_path = harness.models.join(SKYSEG_MODEL_FILENAME);
    let output = harness.mask_job(AiMaskModel::Sky, model_path, Default::default());
    harness.check("sky", Tolerance::MaskIou(0.995), &output);
}

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the depth model and ONNX Runtime"]
fn depth() {
    let harness = Harness::lock();
    let model_path = harness.models.join(DEPTH_MODEL.cache_filename);
    let output = harness.mask_job(AiMaskModel::Depth, model_path, Default::default());
    let output = match output {
        Output::Mask(plane) => Output::Depth(plane),
        other => other,
    };
    harness.check("depth", Tolerance::Levels { mean: 1.0, max: 24 }, &output);
}

/// SAM 2.1 encoder and decoder: one positive stroke along the ship's hull,
/// then a background stroke on the water, which reuses the encoder output
/// and runs the decoder only.
#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the SAM 2.1 models and ONNX Runtime"]
fn object() {
    let harness = Harness::lock();
    let hull = ObjectStroke {
        points: line([0.50, 0.905], [0.84, 0.905], 12),
        positive: true,
        brush_size: 0.015,
    };
    let (output, cache) = harness.object_job(vec![hull.clone()], None);
    harness.check("object", Tolerance::MaskIou(0.98), &output);

    let water = ObjectStroke {
        points: line([0.30, 0.97], [0.45, 0.97], 6),
        positive: false,
        brush_size: 0.015,
    };
    let (output, _) = harness.object_job(vec![hull, water], Some(cache));
    harness.check("object-decoder-only", Tolerance::MaskIou(0.98), &output);
}

/// Big-LaMa through the Remove job: a short stroke on a RAW crop. Scene
/// rendering runs on the GPU in every mode.
#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the Big-LaMa model, ONNX Runtime and a wgpu adapter"]
fn remove() {
    let harness = Harness::lock();
    let raw = Arc::new(harness.centre_crop(&BAYER_RAW, CfaKind::Bayer, REMOVE_CROP_EDGE));
    let gpu = harness.gpu_device();
    let centre = REMOVE_CROP_EDGE as f32 / 2.0;
    let brush = RemoveBrushStroke {
        points: line([centre - 60.0, centre], [centre + 60.0, centre + 20.0], 9)
            .into_iter()
            .map(|[x, y]| RemoveBrushPoint { x, y, radius: 18.0 })
            .collect(),
        dilation_radius: 0,
    };
    let events = spawn_remove(RemoveRequest {
        device: gpu.device.clone(),
        queue: gpu.queue,
        raw,
        geometry: GeometryTransform::default(),
        exposure: ExposureParams::default(),
        masks: MaskStack::default(),
        existing: RemoveEditState::default(),
        brush,
        opacity: 1.0,
        model_path: harness.models.join(BIG_LAMA_MODEL_FILENAME),
        allow_download: harness.download,
        runtime_path: harness.runtime_path(),
        runtime_sha256: harness.runtime_sha256(),
        program_prewarm: None,
        cancellation: Arc::new(AtomicBool::new(false)),
    });
    let stroke = finish(events, |event| match event {
        RemoveEvent::Finished(result) => Some(result),
        _ => None,
    });
    let [patch] = stroke.patches.as_slice() else {
        panic!(
            "the parity stroke must plan one Remove pass, found {}",
            stroke.patches.len()
        );
    };
    let bounds = patch.bounds;
    let output = Output::Inpaint {
        origin: [bounds.x, bounds.y],
        rgb: LinearRgb {
            width: bounds.width,
            height: bounds.height,
            bits: patch.rgb_scene16f.to_vec(),
        },
        alpha: Plane {
            width: bounds.width,
            height: bounds.height,
            values: patch.alpha.to_vec(),
        },
    };
    harness.check("remove", Tolerance::Psnr(40.0), &output);
}

#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the RawNIND models and ONNX Runtime"]
fn denoise_bayer() {
    let harness = Harness::lock();
    let raw = harness.centre_crop(&BAYER_RAW, CfaKind::Bayer, DENOISE_CROP_EDGE);
    let output = harness.denoise_job(raw);
    harness.check(
        "denoise-bayer",
        Tolerance::Codes {
            mean: 2.0,
            max: 256,
        },
        &output,
    );
}

/// The linear model; X-Trans is demosaiced on the GPU before inference.
#[test]
#[ignore = "needs CALIBRAW_PARITY_PHOTOS, the RawNIND models, ONNX Runtime and a wgpu adapter"]
fn denoise_xtrans() {
    let harness = Harness::lock();
    let raw = harness.centre_crop(&XTRANS_RAW, CfaKind::XTrans, DENOISE_CROP_EDGE);
    let output = harness.denoise_job(raw);
    harness.check(
        "denoise-xtrans",
        Tolerance::Linear {
            mean: 2e-4,
            max: 2e-2,
        },
        &output,
    );
}

fn subject_case(name: &str, quality: SubjectMaskQuality, precision: SubjectModelPrecision) {
    let harness = Harness::lock();
    let model_path = harness.models.join(quality.model(precision).cache_filename);
    let output = harness.mask_job(AiMaskModel::Subject, model_path, (quality, precision));
    harness.check(name, Tolerance::MaskIou(0.995), &output);
}

// ---------------------------------------------------------------------------
// Harness

/// Settings from the environment. Holding a `Harness` serializes the cases:
/// sessions are large, and acceleration is a process-wide setting.
struct Harness {
    _serial: MutexGuard<'static, ()>,
    photos: PathBuf,
    models: PathBuf,
    runtime: Option<(PathBuf, String)>,
    download: bool,
    gpu: bool,
    bless: bool,
}

impl Harness {
    fn lock() -> Self {
        static SERIAL: Mutex<()> = Mutex::new(());
        // A failed case poisons the lock; the next case is still valid.
        let serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let photos = PathBuf::from(
            std::env::var_os("CALIBRAW_PARITY_PHOTOS")
                .expect("set CALIBRAW_PARITY_PHOTOS to the Demo photo folder"),
        );
        let models = std::env::var_os("CALIBRAW_PARITY_MODELS")
            .map_or_else(calibraw_ai::desktop_model_cache_root, PathBuf::from);
        let runtime = std::env::var_os("CALIBRAW_PARITY_RUNTIME").map(|path| {
            let path = PathBuf::from(path);
            let sha256 = sha256_file_hex(&path)
                .unwrap_or_else(|error| panic!("hash {}: {error:#}", path.display()));
            (path, sha256)
        });
        let flag = |name: &str| std::env::var_os(name).is_some_and(|value| value == "1");
        let gpu = flag("CALIBRAW_PARITY_GPU");
        let bless = flag("CALIBRAW_PARITY_BLESS");
        assert!(
            !(gpu && bless),
            "goldens come from CPU sessions; unset CALIBRAW_PARITY_GPU to bless"
        );
        calibraw_ai::set_ai_acceleration_enabled(gpu);
        Self {
            _serial: serial,
            photos,
            models,
            runtime,
            download: flag("CALIBRAW_PARITY_DOWNLOAD"),
            gpu,
            bless,
        }
    }

    fn runtime_path(&self) -> Option<PathBuf> {
        self.runtime.as_ref().map(|(path, _)| path.clone())
    }

    fn runtime_sha256(&self) -> Option<String> {
        self.runtime.as_ref().map(|(_, sha256)| sha256.clone())
    }

    fn photo(&self, photo: &Photo) -> PathBuf {
        let path = self.photos.join(photo.path);
        let sha256 = sha256_file_hex(&path)
            .unwrap_or_else(|error| panic!("hash {}: {error:#}", path.display()));
        assert_eq!(
            sha256,
            photo.sha256,
            "{} changed; the parity inputs are pinned",
            path.display()
        );
        path
    }

    /// `SCENE_PHOTO` scaled to `SCENE_EDGE`, decoded once per process.
    fn scene(&self) -> &'static RgbaImage {
        static SCENE: OnceLock<RgbaImage> = OnceLock::new();
        SCENE.get_or_init(|| {
            let path = self.photo(&SCENE_PHOTO);
            let image = image::open(&path)
                .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()))
                .into_rgba8();
            let scale = SCENE_EDGE as f32 / image.width().max(image.height()) as f32;
            let width = (image.width() as f32 * scale).round() as u32;
            let height = (image.height() as f32 * scale).round() as u32;
            image::imageops::resize(&image, width, height, FilterType::Triangle)
        })
    }

    /// A centred square crop of a RAW, with offsets on a 6-pixel grid so
    /// Bayer and X-Trans phases match the full frame.
    fn centre_crop(&self, photo: &Photo, cfa: CfaKind, edge: u32) -> LoadedRaw {
        let path = self.photo(photo);
        let raw = load_raw_file(&path)
            .unwrap_or_else(|error| panic!("load {}: {error:#}", path.display()));
        assert_eq!(raw.cfa_kind, cfa, "{} has an unexpected CFA", photo.path);
        assert!(raw.width >= edge && raw.height >= edge);
        let x = (raw.width - edge) / 2 / 6 * 6;
        let y = (raw.height - edge) / 2 / 6 * 6;
        crop_raw(&raw, x, y, edge, edge)
    }

    fn gpu_device(&self) -> HeadlessDevice {
        let device = request_headless_device(&HeadlessDeviceRequest {
            label: "AI parity",
            power_preference: wgpu::PowerPreference::HighPerformance,
            texture_dimension: 8192,
        })
        .unwrap_or_else(|error| panic!("{error}"));
        eprintln!("wgpu adapter: {}", device.adapter_info.name);
        device
    }

    fn mask_job(
        &self,
        model: AiMaskModel,
        model_path: PathBuf,
        (subject_quality, subject_precision): (SubjectMaskQuality, SubjectModelPrecision),
    ) -> Output {
        let scene = self.scene();
        let events = spawn_ai_mask(
            AiMaskWorkerRequest {
                model,
                subject_quality,
                subject_precision,
                model_path,
                allow_download: self.download,
                runtime_path: self.runtime_path(),
                runtime_sha256: self.runtime_sha256(),
                width: scene.width(),
                height: scene.height(),
                rgba: scene.as_raw().clone(),
            },
            Arc::new(AtomicBool::new(false)),
        );
        let result = finish(events, |event| match event {
            AiMaskEvent::Finished(result) => Some(result),
            _ => None,
        });
        Output::Mask(Plane {
            width: result.width,
            height: result.height,
            values: result.mask,
        })
    }

    fn object_job(
        &self,
        strokes: Vec<ObjectStroke>,
        cache: Option<ObjectInferenceCache>,
    ) -> (Output, ObjectInferenceCache) {
        let scene = self.scene();
        let events = spawn_object_mask(ObjectMaskWorkerRequest {
            encoder_path: self.models.join("sam2.1-hiera-tiny.encoder.onnx"),
            decoder_path: self.models.join("sam2.1-hiera-tiny.decoder.onnx"),
            allow_download: self.download,
            runtime_path: self.runtime_path(),
            runtime_sha256: self.runtime_sha256(),
            inference: ObjectMaskRequest {
                source_width: scene.width(),
                source_height: scene.height(),
                source_rgba: scene.as_raw().clone(),
                strokes,
                brush_size: 0.015,
                edge_refine: 0.0,
                cache,
            },
            cancellation: Arc::new(AtomicBool::new(false)),
        });
        let result = finish(events, |event| match event {
            ObjectMaskEvent::Finished(result) => Some(result),
            _ => None,
        });
        let output = Output::Mask(Plane {
            width: result.width,
            height: result.height,
            values: result.mask,
        });
        (output, result.cache)
    }

    fn denoise_job(&self, raw: LoadedRaw) -> Output {
        let (width, height) = (raw.width, raw.height);
        // `ai_denoise::model_cache_dir()` is this folder in the desktop cache.
        let model_dir = self.models.join("rawdenoise-nind-1.0");
        let gpu = self.gpu_device();
        let events = spawn_rawnind_denoise(
            model_dir,
            self.runtime_path(),
            self.runtime_sha256(),
            Arc::new(raw),
            Some(gpu.device.clone()),
            Some(gpu.queue),
            RawNindJob::Infer {
                save_to: None,
                allow_model_download: self.download,
            },
            Arc::new(AtomicBool::new(false)),
        );
        let image = finish(events, |event| match event {
            AiDenoiseEvent::Finished(result) => Some(result),
            _ => None,
        });
        assert_eq!((image.width, image.height), (width, height));
        // The Bayer model returns denoised sensor codes, the linear model
        // camera RGB.
        match (image.bayer_cfa(), image.camera_rgb16f()) {
            (Some(codes), _) => Output::Cfa(Codes {
                width,
                height,
                values: codes.to_vec(),
            }),
            (None, Some(rgb)) => Output::Linear(LinearRgb {
                width,
                height,
                bits: rgb.to_vec(),
            }),
            (None, None) => panic!("AI denoise returned no pixels"),
        }
    }

    /// Compares `output` with the golden `name`, or writes it when blessing.
    fn check(&self, name: &str, tolerance: Tolerance, output: &Output) {
        // Statuses accumulate per model name over the process.
        let providers = calibraw_ai::active_execution_providers();
        let sessions = providers
            .iter()
            .map(|status| {
                let degraded = if status.degraded { " (degraded)" } else { "" };
                format!(
                    "{} on {}{degraded}",
                    status.model_name, status.active_provider
                )
            })
            .collect::<Vec<_>>();
        eprintln!("{name}: sessions so far: {}", sessions.join(", "));
        let directory = Path::new(GOLDEN_DIR);
        let files = output.files(name);
        if self.bless {
            fs::create_dir_all(directory).expect("create data/ai-parity");
            let mut pins = read_pins(directory);
            for (file, bytes) in &files {
                let path = directory.join(file);
                fs::write(&path, bytes).unwrap_or_else(|error| panic!("write {file}: {error}"));
                pins.insert(file.clone(), sha256_file_hex(&path).expect("hash golden"));
            }
            write_pins(directory, &pins);
            eprintln!("{name}: blessed {} files", files.len());
            return;
        }
        let pins = read_pins(directory);
        let mut golden = BTreeMap::new();
        for (file, _) in &files {
            let path = directory.join(file);
            let expected = pins.get(file).unwrap_or_else(|| {
                panic!("no golden {file}; bless with CALIBRAW_PARITY_BLESS=1 on the CPU")
            });
            let actual = sha256_file_hex(&path)
                .unwrap_or_else(|error| panic!("hash golden {file}: {error:#}"));
            assert_eq!(&actual, expected, "golden {file} does not match {PINS}");
            golden.insert(file.clone(), fs::read(&path).expect("read golden"));
        }
        let golden = Output::decode(output, name, &golden);
        match compare(tolerance, &golden, output) {
            Ok(report) => eprintln!("{name}: {report}"),
            Err(report) => panic!("{name}: {report}"),
        }
        if self.gpu && providers.iter().any(|status| status.degraded) {
            eprintln!("{name}: warning: a session fell back to the CPU");
        }
    }
}

/// Drains `events` until the job finishes and returns its result.
fn finish<E, T>(events: mpsc::Receiver<E>, finished: impl Fn(E) -> Option<Result<T, String>>) -> T {
    for event in events {
        if let Some(result) = finished(event) {
            return result.unwrap_or_else(|error| panic!("AI job failed: {error}"));
        }
    }
    panic!("AI job ended without a result");
}

/// `count` evenly spaced points from `from` to `to`.
fn line(from: [f32; 2], to: [f32; 2], count: usize) -> Vec<[f32; 2]> {
    (0..count)
        .map(|index| {
            let t = index as f32 / (count - 1) as f32;
            [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ]
        })
        .collect()
}

fn read_pins(directory: &Path) -> BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(directory.join(PINS)) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| line.split_once("  "))
        .map(|(sha256, file)| (file.to_owned(), sha256.to_owned()))
        .collect()
}

fn write_pins(directory: &Path, pins: &BTreeMap<String, String>) {
    let mut text = String::new();
    for (file, sha256) in pins {
        let _ = writeln!(text, "{sha256}  {file}");
    }
    fs::write(directory.join(PINS), text).expect("write SHA256SUMS");
}

// ---------------------------------------------------------------------------
// Outputs and metrics

struct Plane {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

/// One RAW sensor code per pixel, as stored by the Bayer denoiser.
struct Codes {
    width: u32,
    height: u32,
    values: Vec<u16>,
}

/// Linear RGB (scene or camera) as float16 bit patterns, interleaved.
struct LinearRgb {
    width: u32,
    height: u32,
    bits: Vec<u16>,
}

impl LinearRgb {
    fn values(&self) -> impl Iterator<Item = f32> + '_ {
        self.bits.iter().map(|bits| f16::from_bits(*bits).to_f32())
    }
}

enum Output {
    /// Probability mask, 0–255.
    Mask(Plane),
    /// Relative depth, 0–255.
    Depth(Plane),
    Cfa(Codes),
    Linear(LinearRgb),
    /// One Remove patch at `origin` in the crop.
    Inpaint {
        origin: [u32; 2],
        rgb: LinearRgb,
        alpha: Plane,
    },
}

impl Output {
    /// Golden file names and contents.
    fn files(&self, name: &str) -> Vec<(String, Vec<u8>)> {
        match self {
            Self::Mask(plane) | Self::Depth(plane) => vec![(format!("{name}.png"), png_l8(plane))],
            Self::Cfa(codes) => vec![(format!("{name}.png"), png_l16(codes))],
            Self::Linear(rgb) => vec![(format!("{name}.png"), png_f16(rgb))],
            Self::Inpaint { origin, rgb, alpha } => vec![
                (format!("{name}.png"), png_f16(rgb)),
                (format!("{name}-alpha.png"), png_l8(alpha)),
                (
                    format!("{name}-origin.txt"),
                    format!("{} {}\n", origin[0], origin[1]).into_bytes(),
                ),
            ],
        }
    }

    /// The golden of the same kind as `like`, from its `files`.
    fn decode(like: &Self, name: &str, files: &BTreeMap<String, Vec<u8>>) -> Self {
        let file = |suffix: &str| &files[&format!("{name}{suffix}")];
        match like {
            Self::Mask(_) => Self::Mask(decode_l8(file(".png"))),
            Self::Depth(_) => Self::Depth(decode_l8(file(".png"))),
            Self::Cfa(_) => Self::Cfa(decode_l16(file(".png"))),
            Self::Linear(_) => Self::Linear(decode_f16(file(".png"))),
            Self::Inpaint { .. } => {
                let origin = String::from_utf8_lossy(file("-origin.txt"))
                    .split_whitespace()
                    .map(|value| value.parse().expect("golden origin"))
                    .collect::<Vec<u32>>();
                Self::Inpaint {
                    origin: [origin[0], origin[1]],
                    rgb: decode_f16(file(".png")),
                    alpha: decode_l8(file("-alpha.png")),
                }
            }
        }
    }
}

fn png_l8(plane: &Plane) -> Vec<u8> {
    let image = GrayImage::from_raw(plane.width, plane.height, plane.values.clone())
        .expect("plane dimensions");
    encode_png(&image)
}

fn png_l16(codes: &Codes) -> Vec<u8> {
    let image =
        ImageBuffer::<Luma<u16>, _>::from_raw(codes.width, codes.height, codes.values.clone())
            .expect("CFA dimensions");
    encode_png(&image)
}

fn png_f16(rgb: &LinearRgb) -> Vec<u8> {
    let image = ImageBuffer::<Rgb<u16>, _>::from_raw(rgb.width, rgb.height, rgb.bits.clone())
        .expect("RGB dimensions");
    encode_png(&image)
}

fn encode_png<P>(image: &ImageBuffer<P, Vec<P::Subpixel>>) -> Vec<u8>
where
    P: image::PixelWithColorType,
    [P::Subpixel]: image::EncodableLayout,
{
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode golden PNG");
    bytes.into_inner()
}

fn decode_l8(bytes: &[u8]) -> Plane {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("decode golden PNG");
    let image::DynamicImage::ImageLuma8(image) = image else {
        panic!("golden mask is not 8-bit grayscale");
    };
    Plane {
        width: image.width(),
        height: image.height(),
        values: image.into_raw(),
    }
}

fn decode_l16(bytes: &[u8]) -> Codes {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("decode golden PNG");
    let image::DynamicImage::ImageLuma16(image) = image else {
        panic!("golden CFA is not 16-bit grayscale");
    };
    Codes {
        width: image.width(),
        height: image.height(),
        values: image.into_raw(),
    }
}

fn decode_f16(bytes: &[u8]) -> LinearRgb {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("decode golden PNG");
    let image::DynamicImage::ImageRgb16(image) = image else {
        panic!("golden linear RGB is not 16-bit RGB");
    };
    LinearRgb {
        width: image.width(),
        height: image.height(),
        bits: image.into_raw(),
    }
}

#[derive(Clone, Copy, Debug)]
enum Tolerance {
    /// Minimum intersection over union of the masks thresholded at 50%.
    MaskIou(f64),
    /// Largest mean and maximum absolute difference in 8-bit levels.
    Levels { mean: f64, max: u8 },
    /// Largest mean and maximum absolute difference in RAW sensor codes.
    Codes { mean: f64, max: u16 },
    /// Largest mean and maximum absolute difference in linear units.
    Linear { mean: f32, max: f32 },
    /// Minimum PSNR of the RGB in dB, with peak 1.0 (scene-linear white).
    /// Alpha comes from the brush, not the model, and must match exactly.
    Psnr(f64),
}

/// A one-line report, `Err` when `actual` is outside `tolerance`.
fn compare(tolerance: Tolerance, golden: &Output, actual: &Output) -> Result<String, String> {
    match (tolerance, golden, actual) {
        (Tolerance::MaskIou(minimum), Output::Mask(golden), Output::Mask(actual)) => {
            same_size(golden.width, golden.height, actual.width, actual.height)?;
            if golden.values == actual.values {
                return Ok("bit-identical".to_owned());
            }
            let (mut intersection, mut union) = (0u64, 0u64);
            for (&golden, &actual) in golden.values.iter().zip(&actual.values) {
                let (golden, actual) = (golden >= 128, actual >= 128);
                intersection += u64::from(golden && actual);
                union += u64::from(golden || actual);
            }
            let iou = if union == 0 {
                1.0
            } else {
                intersection as f64 / union as f64
            };
            let (mean, max) = level_differences(&golden.values, &actual.values);
            let report = format!("IoU {iou:.5} (minimum {minimum}), mean {mean:.3}, max {max}");
            if iou >= minimum {
                Ok(report)
            } else {
                Err(report)
            }
        }
        (Tolerance::Levels { mean, max }, Output::Depth(golden), Output::Depth(actual)) => {
            same_size(golden.width, golden.height, actual.width, actual.height)?;
            if golden.values == actual.values {
                return Ok("bit-identical".to_owned());
            }
            let (actual_mean, actual_max) = level_differences(&golden.values, &actual.values);
            let report = format!(
                "mean {actual_mean:.3} (limit {mean}), max {actual_max} (limit {max}) levels"
            );
            if actual_mean <= mean && actual_max <= max {
                Ok(report)
            } else {
                Err(report)
            }
        }
        (Tolerance::Codes { mean, max }, Output::Cfa(golden), Output::Cfa(actual)) => {
            same_size(golden.width, golden.height, actual.width, actual.height)?;
            if golden.values == actual.values {
                return Ok("bit-identical".to_owned());
            }
            let mut sum = 0u64;
            let mut actual_max = 0u16;
            for (&golden, &actual) in golden.values.iter().zip(&actual.values) {
                let difference = golden.abs_diff(actual);
                sum += u64::from(difference);
                actual_max = actual_max.max(difference);
            }
            let actual_mean = sum as f64 / golden.values.len().max(1) as f64;
            let report = format!(
                "mean {actual_mean:.3} (limit {mean}), max {actual_max} (limit {max}) codes"
            );
            if actual_mean <= mean && actual_max <= max {
                Ok(report)
            } else {
                Err(report)
            }
        }
        (Tolerance::Linear { mean, max }, Output::Linear(golden), Output::Linear(actual)) => {
            same_size(golden.width, golden.height, actual.width, actual.height)?;
            if golden.bits == actual.bits {
                return Ok("bit-identical".to_owned());
            }
            let (actual_mean, actual_max) = linear_differences(golden, actual)?;
            let report = format!(
                "mean {actual_mean:.2e} (limit {mean:.0e}), max {actual_max:.2e} (limit {max:.0e})"
            );
            if actual_mean <= mean && actual_max <= max {
                Ok(report)
            } else {
                Err(report)
            }
        }
        (
            Tolerance::Psnr(minimum),
            Output::Inpaint {
                origin: golden_origin,
                rgb: golden_rgb,
                alpha: golden_alpha,
            },
            Output::Inpaint { origin, rgb, alpha },
        ) => {
            if golden_origin != origin {
                return Err(format!("patch at {origin:?}, golden at {golden_origin:?}"));
            }
            same_size(golden_rgb.width, golden_rgb.height, rgb.width, rgb.height)?;
            if golden_alpha.values != alpha.values {
                return Err("patch alpha differs".to_owned());
            }
            if golden_rgb.bits == rgb.bits {
                return Ok("bit-identical".to_owned());
            }
            let mut squared = 0.0f64;
            for (golden, actual) in golden_rgb.values().zip(rgb.values()) {
                if !actual.is_finite() {
                    return Err("patch contains non-finite values".to_owned());
                }
                squared += f64::from(golden - actual).powi(2);
            }
            let mse = squared / golden_rgb.bits.len() as f64;
            let psnr = -10.0 * mse.log10();
            let report = format!("PSNR {psnr:.2} dB (minimum {minimum})");
            if psnr >= minimum {
                Ok(report)
            } else {
                Err(report)
            }
        }
        _ => Err("golden and output are different kinds".to_owned()),
    }
}

fn same_size(golden_width: u32, golden_height: u32, width: u32, height: u32) -> Result<(), String> {
    if (golden_width, golden_height) == (width, height) {
        Ok(())
    } else {
        Err(format!(
            "output is {width}x{height}, golden {golden_width}x{golden_height}"
        ))
    }
}

/// Mean and maximum absolute difference of two equally long 8-bit planes.
fn level_differences(golden: &[u8], actual: &[u8]) -> (f64, u8) {
    let mut sum = 0u64;
    let mut max = 0u8;
    for (&golden, &actual) in golden.iter().zip(actual) {
        let difference = golden.abs_diff(actual);
        sum += u64::from(difference);
        max = max.max(difference);
    }
    (sum as f64 / golden.len().max(1) as f64, max)
}

fn linear_differences(golden: &LinearRgb, actual: &LinearRgb) -> Result<(f32, f32), String> {
    let mut sum = 0.0f64;
    let mut max = 0.0f32;
    for (golden, actual) in golden.values().zip(actual.values()) {
        if !actual.is_finite() {
            return Err("output contains non-finite values".to_owned());
        }
        let difference = (golden - actual).abs();
        sum += f64::from(difference);
        max = max.max(difference);
    }
    Ok(((sum / golden.bits.len().max(1) as f64) as f32, max))
}

/// The metrics themselves run without models or photos.
mod metrics {
    use super::*;

    fn mask(values: Vec<u8>) -> Output {
        Output::Mask(Plane {
            width: values.len() as u32,
            height: 1,
            values,
        })
    }

    #[test]
    fn mask_iou_thresholds_at_half() {
        let golden = mask(vec![0, 200, 200, 200]);
        assert_eq!(
            compare(
                Tolerance::MaskIou(1.0),
                &golden,
                &mask(vec![0, 200, 200, 200])
            ),
            Ok("bit-identical".to_owned())
        );
        // One of three foreground pixels drops out: IoU 2/3.
        assert!(compare(
            Tolerance::MaskIou(0.66),
            &golden,
            &mask(vec![0, 200, 200, 10])
        )
        .is_ok());
        assert!(compare(
            Tolerance::MaskIou(0.67),
            &golden,
            &mask(vec![0, 200, 200, 10])
        )
        .is_err());
        // Soft changes on the same side of 50% keep IoU 1.
        assert!(compare(
            Tolerance::MaskIou(1.0),
            &golden,
            &mask(vec![100, 129, 255, 200])
        )
        .is_ok());
    }

    #[test]
    fn linear_rgb_round_trips_through_png_exactly() {
        let bits = [0.0f32, 1.0, -0.5, 65504.0, 1e-7, 0.333]
            .map(|value| f16::from_f32(value).to_bits())
            .to_vec();
        let rgb = LinearRgb {
            width: 2,
            height: 1,
            bits: bits.clone(),
        };
        assert_eq!(decode_f16(&png_f16(&rgb)).bits, bits);
    }

    #[test]
    fn linear_and_psnr_limits_apply() {
        let rgb = |values: &[f32]| LinearRgb {
            width: 1,
            height: 1,
            bits: values.iter().map(|v| f16::from_f32(*v).to_bits()).collect(),
        };
        let tolerance = Tolerance::Linear {
            mean: 0.01,
            max: 0.02,
        };
        let golden = Output::Linear(rgb(&[0.5, 0.5, 0.5]));
        assert!(compare(tolerance, &golden, &Output::Linear(rgb(&[0.51, 0.5, 0.5]))).is_ok());
        assert!(compare(tolerance, &golden, &Output::Linear(rgb(&[0.6, 0.5, 0.5]))).is_err());
        assert!(compare(
            tolerance,
            &golden,
            &Output::Linear(rgb(&[f32::NAN, 0.5, 0.5]))
        )
        .is_err());

        let inpaint = |values: &[f32], alpha: u8| Output::Inpaint {
            origin: [4, 8],
            rgb: rgb(values),
            alpha: Plane {
                width: 1,
                height: 1,
                values: vec![alpha],
            },
        };
        let golden = inpaint(&[0.5, 0.5, 0.5], 255);
        // An error of 0.01 everywhere is 40 dB.
        assert!(compare(
            Tolerance::Psnr(39.9),
            &golden,
            &inpaint(&[0.51, 0.51, 0.51], 255)
        )
        .is_ok());
        assert!(compare(
            Tolerance::Psnr(40.5),
            &golden,
            &inpaint(&[0.51, 0.51, 0.51], 255)
        )
        .is_err());
        assert!(compare(
            Tolerance::Psnr(0.0),
            &golden,
            &inpaint(&[0.5, 0.5, 0.5], 254)
        )
        .is_err());
    }
}
