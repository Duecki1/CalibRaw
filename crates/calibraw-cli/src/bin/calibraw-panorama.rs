//! Headless panorama merge: stitches RAW frames into a floating-point TIFF as the
//! library's Panorama merge does, and prints the merge's diagnostics (keypoints,
//! verified pairs, alignment residual, exposure gains, projection and size).

use anyhow::{anyhow, bail, Context, Result};
use calibraw_core::pipeline::panorama::ProjectionChoice;
use calibraw_gpu::pipeline::{merge_panorama, MergeOptions, PanoramaOptions};
use calibraw_gpu::{
    request_headless_device, wgpu, HeadlessAdapter, HeadlessDevice, HeadlessDeviceError,
    HeadlessDeviceRequest,
};
use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Instant;

/// Merges render their sources in tiles of 1024 pixels; this leaves room for their
/// mask atlases.
const TEXTURE_DIMENSION: u32 = 4096;

struct Args {
    output: PathBuf,
    inputs: Vec<PathBuf>,
    options: MergeOptions,
    projection: ProjectionChoice,
}

fn main() {
    env_logger::init();
    if let Err(error) = run() {
        eprintln!("{}", calibraw_core::diagnostics::snapshot());
        eprintln!("calibraw-panorama: {error:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let HeadlessDevice { device, queue, .. } = request_headless_device(&HeadlessDeviceRequest {
        label: "calibraw headless panorama merge",
        adapter: HeadlessAdapter::PreferredOrSoftware(wgpu::PowerPreference::HighPerformance),
        texture_dimension: TEXTURE_DIMENSION,
    })
    .map_err(|error| match error {
        HeadlessDeviceError::TextureTooLarge { .. } => {
            anyhow!("the adapter's textures are too small")
        }
        HeadlessDeviceError::NoAdapter(error) => anyhow!("request a wgpu adapter: {error}"),
        HeadlessDeviceError::Device(error) => anyhow!("request a wgpu device: {error}"),
    })?;
    let started = Instant::now();
    let report = |message: String| {
        eprintln!("[{:7.1} s] {message}", started.elapsed().as_secs_f32());
        true
    };
    let staging = args
        .output
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let master = merge_panorama(
        &device,
        &queue,
        &args.inputs,
        args.options,
        PanoramaOptions {
            projection: args.projection,
        },
        None,
        None,
        &staging,
        report,
    )?;
    let file =
        File::create(&args.output).with_context(|| format!("create {}", args.output.display()))?;
    master.write_tiff(BufWriter::new(file), report)?;
    eprintln!(
        "Wrote {} ({} × {}{}{}) in {:.1} s",
        args.output.display(),
        master.width(),
        master.height(),
        if master.reduced() { ", reduced" } else { "" },
        if master.lens_corrected() {
            ", lens corrected"
        } else {
            ""
        },
        started.elapsed().as_secs_f32()
    );
    println!("{}", calibraw_core::diagnostics::snapshot());
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut output = None;
    let mut inputs = Vec::new();
    let mut options = MergeOptions::default();
    let mut projection = ProjectionChoice::Auto;
    let mut values = env::args().skip(1);
    while let Some(value) = values.next() {
        match value.as_str() {
            "--output" | "-o" => {
                output = Some(PathBuf::from(
                    values.next().context("--output needs a path")?,
                ))
            }
            "--projection" => {
                projection = match values.next().as_deref() {
                    Some("auto") => ProjectionChoice::Auto,
                    Some("spherical") => ProjectionChoice::Spherical,
                    Some("cylindrical") => ProjectionChoice::Cylindrical,
                    Some("perspective") => ProjectionChoice::Perspective,
                    other => bail!("unknown projection {other:?}"),
                }
            }
            "--no-lens-correction" => options.lens_correction = false,
            "--remove-hot-pixels" => options.remove_hot_pixels = true,
            "--denoise" => options.denoise = true,
            "--help" | "-h" => {
                eprintln!(
                    "Usage: calibraw-panorama --output OUT.tif [--projection auto|spherical|cylindrical|perspective] \
                     [--no-lens-correction] [--remove-hot-pixels] [--denoise] PHOTO..."
                );
                std::process::exit(0);
            }
            _ => inputs.push(PathBuf::from(value)),
        }
    }
    Ok(Args {
        output: output.context("--output is required")?,
        inputs,
        options,
        projection,
    })
}
