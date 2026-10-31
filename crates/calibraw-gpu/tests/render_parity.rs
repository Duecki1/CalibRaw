//! Render parity: every case in `data/render-parity/` is exported and compared
//! with its golden image.
//!
//! A case is `<input>/<name>.json`, a CalibRaw sidecar whose missing fields
//! keep their default values, so it lists only what it changes; a sidecar the
//! app saved works as it is. `<input>` names the synthetic photo it develops
//! (see [`input`]). The golden is `<name>.png` beside it, a 16-bit sRGB PNG.
//!
//! Cases render on the machine's GPU, or wgpu's software adapter without one
//! (Mesa's lavapipe in CI). GPUs round differently, so a render matches when
//! the mean colour of every [`BLOCK_EDGE`]-pixel square stays within
//! [`BLOCK_TOLERANCE`] of the golden: rounding that moves single pixels, such
//! as a grain cell or mask edge, averages out, while a changed tone or colour
//! does not. `CALIBRAW_PARITY_BLESS=1` rewrites every golden; review the
//! changed images before committing them. A failing case leaves its render and
//! an amplified difference image in `target/tmp/render-parity/`.
//!
//! The edit is rendered as the export path receives it. What the app prepares
//! before an export is not covered: lens corrections, LUTs and the source
//! images of range, depth and AI masks.

use calibraw_core::sidecar::{self, EditState};
use calibraw_gpu::pipeline::{
    build_proxy, spawn_tiled_export, CfaKind, CompactPixelMap, ExportBitDepth, ExportEvent,
    ExportFiles, ExportFormat, ExportMetadata, ExportSettings, ExportTarget, LoadedRaw, ProxySpec,
    TileSpec, TiledExportJob,
};
use calibraw_gpu::{request_headless_device, wgpu, HeadlessAdapter, HeadlessDeviceRequest};
use image::{ImageBuffer, Rgb, RgbImage};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, Arc};

type Rgb16Image = ImageBuffer<Rgb<u16>, Vec<u16>>;

/// Divisible by the 2×2 Bayer and 6×6 X-Trans patterns.
const WIDTH: u32 = 288;
const HEIGHT: u32 = 192;
/// Edge of the squares whose mean colours are compared.
const BLOCK_EDGE: u32 = 8;
/// How far a square's mean may move in any channel, in output units (two
/// 8-bit levels).
const BLOCK_TOLERANCE: f32 = 2.0 / 255.0;

#[test]
fn rendered_cases_match_their_goldens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/render-parity");
    let output = Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-parity");
    fs::create_dir_all(&output).unwrap();
    let bless = std::env::var_os("CALIBRAW_PARITY_BLESS").is_some();
    let gpu = request_headless_device(&HeadlessDeviceRequest {
        label: "render parity",
        adapter: HeadlessAdapter::PreferredOrSoftware(wgpu::PowerPreference::HighPerformance),
        texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
    })
    .unwrap_or_else(|error| panic!("render parity needs a GPU adapter: {error}"));

    let mut failures = Vec::new();
    for case in cases(&root) {
        let input_name = case
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        let name = format!(
            "{input_name}-{}",
            case.file_stem().unwrap().to_string_lossy()
        );
        let rendered = output.join(format!("{name}.png"));
        render(
            &gpu.device,
            &gpu.queue,
            input(&input_name),
            edits(&case),
            &rendered,
        );
        let golden = case.with_extension("png");
        if bless {
            fs::copy(&rendered, &golden).unwrap();
            continue;
        }
        let actual = image::open(&rendered).unwrap().into_rgb16();
        let Ok(expected) = image::open(&golden) else {
            failures.push(format!("{name}: no golden at {}", golden.display()));
            continue;
        };
        let expected = expected.into_rgb16();
        let difference = largest_block_difference(&expected, &actual);
        if difference > BLOCK_TOLERANCE {
            let diff = output.join(format!("{name}.diff.png"));
            difference_image(&expected, &actual).save(&diff).unwrap();
            failures.push(format!(
                "{name}: a square's mean moved by {:.1} 8-bit levels; see {} and {}",
                difference * 255.0,
                rendered.display(),
                diff.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "rendered on {}; bless with CALIBRAW_PARITY_BLESS=1 if the change is intended:\n{}",
        gpu.adapter_info.name,
        failures.join("\n")
    );
}

/// Every case, sorted so failures list in a stable order.
fn cases(root: &Path) -> Vec<PathBuf> {
    let mut cases = Vec::new();
    for input in fs::read_dir(root).unwrap() {
        for entry in fs::read_dir(input.unwrap().path()).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                cases.push(path);
            }
        }
    }
    cases.sort();
    assert!(!cases.is_empty(), "{} has no cases", root.display());
    cases
}

/// The case's sidecar, its missing fields taken from a default edit.
fn edits(case: &Path) -> EditState {
    let mut document: Value =
        serde_json::from_slice(&sidecar::encode(sidecar::default_edit_state()).unwrap()).unwrap();
    let changes: Value = serde_json::from_slice(&fs::read(case).unwrap())
        .unwrap_or_else(|error| panic!("{}: {error}", case.display()));
    merge(&mut document, changes);
    sidecar::decode(&serde_json::to_vec(&document).unwrap())
        .unwrap_or_else(|error| panic!("{}: {error}", case.display()))
        .edits
}

/// Overwrites `base` with `changes`, merging objects field by field.
fn merge(base: &mut Value, changes: Value) {
    match (base, changes) {
        (Value::Object(base), Value::Object(changes)) => {
            for (key, value) in changes {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, changes) => *base = changes,
    }
}

/// The synthetic photo a case develops:
/// - `scene`: the test scene as scene-linear Rec.2020, which skips the sensor
///   stages;
/// - `bayer` and `xtrans`: the scene seen through an RGGB or X-Trans mosaic,
///   with black level, white balance, clipped highlights and noise;
/// - `bayer-binned` and `xtrans-binned`: those mosaics at half their size,
///   binned to RGB as a reduced preview's proxy is.
fn input(name: &str) -> LoadedRaw {
    if let Some(sensor) = name.strip_suffix("-binned") {
        return build_proxy(
            &input(sensor),
            ProxySpec {
                max_edge: WIDTH / 2,
            },
        );
    }
    let scene: Vec<f32> = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).flat_map(move |x| scene(x, y)))
        .collect();
    let raster = LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, scene.clone()).unwrap();
    // Colour indices: 0 red, 1 green, 2 blue, 3 the second Bayer green.
    let (cfa_kind, period, pattern) = match name {
        "scene" => return raster,
        "bayer" => (CfaKind::Bayer, 2, vec![0, 1, 3, 2]),
        "xtrans" => (CfaKind::XTrans, 6, XTRANS.concat()),
        _ => panic!(
            "unknown input {name:?}; use scene, bayer, xtrans, bayer-binned or xtrans-binned"
        ),
    };
    const BLACK: f32 = 512.0;
    const WHITE: f32 = 16383.0;
    // As-shot white balance: red and blue reach the sensor weaker than green.
    const WB: [f32; 4] = [2.1, 1.0, 1.6, 1.0];
    // Scene white lands at half the sensor's range, so the top of the
    // exposure ramp and the specular spots clip.
    const SCENE_WHITE: f32 = 0.5;
    let pixels = (0..WIDTH * HEIGHT)
        .map(|index| {
            let (x, y) = (index % WIDTH, index / WIDTH);
            let color = usize::from(pattern[((y % period) * period + x % period) as usize]);
            let channel = [0, 1, 2, 1][color];
            let signal = scene[index as usize * 3 + channel] / WB[color] * SCENE_WHITE;
            let noise = (hash(x, y) - 0.5) * 0.004 * (signal + 0.01).sqrt();
            (BLACK + (signal + noise).clamp(0.0, 1.0) * (WHITE - BLACK)).round() as u16
        })
        .collect::<Vec<_>>();
    LoadedRaw {
        cfa_kind,
        raw_pixels: pixels.into(),
        scene_linear_raster: None,
        color_indices: CompactPixelMap::repeating(WIDTH, HEIGHT, period, period, pattern),
        wb_coeffs: WB,
        black_levels: [BLACK; 4],
        black_levels_per_pixel: CompactPixelMap::repeating(WIDTH, HEIGHT, 1, 1, vec![BLACK]),
        white_levels: [WHITE; 4],
        ..raster
    }
}

const XTRANS: [[u8; 6]; 6] = [
    [1, 1, 0, 1, 1, 2],
    [1, 1, 2, 1, 1, 0],
    [2, 0, 1, 0, 2, 1],
    [1, 1, 2, 1, 1, 0],
    [1, 1, 0, 1, 1, 2],
    [0, 2, 1, 2, 0, 1],
];

/// Scene-linear Rec.2020 at a pixel, in four bands from top to bottom: an
/// exposure ramp from -8 to +4 EV around mid grey, saturated and pale colour
/// patches, gratings that get finer to the right, and a hue sweep that fades
/// to black with bright specular spots.
fn scene(x: u32, y: u32) -> [f32; 3] {
    let u = (x as f32 + 0.5) / WIDTH as f32;
    let v = (y as f32 + 0.5) / HEIGHT as f32;
    let band = (v * 4.0) as u32;
    let local_v = v * 4.0 - band as f32;
    match band {
        0 => [0.18 * (12.0 * u - 8.0).exp2(); 3],
        1 => {
            let hue = (u * 12.0).floor() / 12.0;
            let saturation = if local_v < 0.5 { 0.9 } else { 0.35 };
            hue_color(hue, saturation, 0.25)
        }
        2 => {
            let frequency = 2.0 + 60.0 * u * u;
            let wave = (std::f32::consts::TAU * frequency * u).sin();
            let edge = if (x / 24 + y / 12).is_multiple_of(2) {
                1.0
            } else {
                0.6
            };
            let value = 0.18 * edge * (1.0 + 0.8 * wave);
            [value, value * 0.95, value * 0.85]
        }
        _ => {
            let spot = ((x % 72) as f32 - 36.0).hypot((y % 48) as f32 - 24.0) < 3.0;
            if spot {
                [6.0, 5.5, 4.0]
            } else {
                hue_color(u, 0.6, 0.4 * (1.0 - local_v))
            }
        }
    }
}

/// A colour of `hue` (0 to 1 around the wheel), `saturation` and luminance
/// near `level`.
fn hue_color(hue: f32, saturation: f32, level: f32) -> [f32; 3] {
    let channel = |offset: f32| {
        let angle = std::f32::consts::TAU * (hue - offset);
        level * (1.0 + saturation * angle.cos())
    };
    [channel(0.0), channel(1.0 / 3.0), channel(2.0 / 3.0)]
}

/// Deterministic noise in 0..1 for a pixel.
fn hash(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2C1B_3C6D);
    value ^= value >> 12;
    (value & 0xFFFF) as f32 / 65535.0
}

fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: LoadedRaw,
    edits: EditState,
    path: &Path,
) {
    let receiver = spawn_tiled_export(
        ExportFormat::Png,
        TiledExportJob {
            device: device.clone(),
            queue: queue.clone(),
            raw: Arc::new(raw),
            geometry: edits.geometry.sanitized(),
            exposure: edits.exposure,
            masks: Arc::unwrap_or_clone(edits.masks),
            remove: Arc::unwrap_or_clone(edits.remove),
            look: None,
            files: ExportFiles::Single(ExportTarget::File(path.to_path_buf())),
            tile_spec: TileSpec::default(),
            settings: ExportSettings {
                keep_metadata: false,
                bit_depth: ExportBitDepth::Sixteen,
                ..ExportSettings::default()
            },
            metadata: ExportMetadata::default(),
            cancellation: Arc::new(AtomicBool::new(false)),
            program_prewarm: None,
        },
    );
    loop {
        match receiver.recv() {
            Ok(ExportEvent::Finished(result)) => {
                result.unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                return;
            }
            Ok(_) => {}
            Err(_) => panic!("{}: the export stopped without finishing", path.display()),
        }
    }
}

/// The largest change of a square's mean in any channel, in output units;
/// 1 when the sizes differ.
fn largest_block_difference(expected: &Rgb16Image, actual: &Rgb16Image) -> f32 {
    if expected.dimensions() != actual.dimensions() {
        return 1.0;
    }
    let (width, height) = actual.dimensions();
    let mut largest = 0.0_f32;
    for top in (0..height).step_by(BLOCK_EDGE as usize) {
        for left in (0..width).step_by(BLOCK_EDGE as usize) {
            let mut sums = [0.0_f64; 3];
            let mut count = 0.0;
            for y in top..(top + BLOCK_EDGE).min(height) {
                for x in left..(left + BLOCK_EDGE).min(width) {
                    let (Rgb(expected), Rgb(actual)) =
                        (expected.get_pixel(x, y), actual.get_pixel(x, y));
                    for (sum, channel) in sums.iter_mut().zip(0..3) {
                        *sum += f64::from(actual[channel]) - f64::from(expected[channel]);
                    }
                    count += 1.0;
                }
            }
            for sum in sums {
                largest = largest.max((sum.abs() / count / 65535.0) as f32);
            }
        }
    }
    largest
}

/// Each channel's difference, sixteen times brighter.
fn difference_image(expected: &Rgb16Image, actual: &Rgb16Image) -> RgbImage {
    RgbImage::from_fn(actual.width(), actual.height(), |x, y| {
        let (Rgb(expected), Rgb(actual)) = match expected.get_pixel_checked(x, y) {
            Some(expected) => (expected, actual.get_pixel(x, y)),
            None => return Rgb([255; 3]),
        };
        Rgb(std::array::from_fn(|channel| {
            let difference = f32::from(expected[channel].abs_diff(actual[channel])) / 65535.0;
            (difference * 16.0 * 255.0).min(255.0) as u8
        }))
    })
}
