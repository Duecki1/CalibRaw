//! Camera match diagnostics: fits the camera look of every RAW in a folder
//! and prints the held-out CIEDE2000 of each model stage, the alignment and
//! the guard's verdict.

use anyhow::{anyhow, bail, Context, Result};
use calibraw_cli::pipeline::camera_look::{
    fit_camera_look, CameraLookFit, CameraLookFitError, FitStage, FitVerdict,
};
use calibraw_cli::pipeline::{
    build_proxy, is_camera_raw_path, load_raw_embedded_thumbnail, load_raw_file, ExposureParams,
    GpuParams, MaskStack, PipelineOptions, ProcessingQuality, ProxySpec, RawGpuPipeline,
    RawGpuProgramTemplate, RawThumbnail,
};
use calibraw_core::luts::ResolvedLook;
use calibraw_gpu::{request_headless_device, wgpu, HeadlessAdapter, HeadlessDeviceRequest};
use std::cell::Cell;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The preview edge the fit asks for, as the editor will.
const PREVIEW_EDGE: u32 = 512;

const STAGES: [FitStage; 7] = [
    FitStage::Canonical,
    FitStage::Exposure,
    FitStage::Curve,
    FitStage::Matrix,
    FitStage::Residual,
    FitStage::BakedMatrix,
    FitStage::BakedResidual,
];

struct Args {
    folders: Vec<PathBuf>,
    dump: Option<PathBuf>,
}

fn main() {
    env_logger::init();
    if let Err(error) = run() {
        eprintln!("calibraw-camera-match: {error:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let gpu = request_headless_device(&HeadlessDeviceRequest {
        label: "calibraw camera match diagnostics",
        adapter: HeadlessAdapter::PreferredOrSoftware(wgpu::PowerPreference::HighPerformance),
        texture_dimension: 2048,
    })
    .map_err(|error| anyhow!("request a wgpu device: {error:?}"))?;
    if let Some(dump) = &args.dump {
        std::fs::create_dir_all(dump).with_context(|| format!("create {}", dump.display()))?;
    }
    let mut harness = Harness {
        device: gpu.device,
        queue: gpu.queue,
        templates: Vec::new(),
        dump: args.dump,
    };

    print!(
        "{:<34} {:>9} {:>6} {:>5} {:>6}",
        "file", "preview", "scale", "corr", "EV"
    );
    for stage in STAGES {
        print!(" {:>11}", stage.label());
    }
    println!(
        " {:>5} {:>5} {:>8} {:>16}",
        "folds", "fullF", "verdict", "setup/renders/fit ms"
    );
    println!("{:>69}  (held-out CIEDE2000 mean/p90)", "");

    let mut fits = Vec::new();
    let mut failures = Vec::new();
    for folder in &args.folders {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
            .with_context(|| format!("read {}", folder.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file() && is_camera_raw_path(path))
            .collect();
        paths.sort();
        for path in paths {
            let name = display_name(&path);
            match harness.fit(&path) {
                Ok((fit, preview, timing)) => {
                    print_row(&name, &fit, &preview, &timing);
                    fits.push((name, fit));
                }
                Err(error) => {
                    println!("{name:<34} skipped: {error:#}");
                    failures.push(name);
                }
            }
        }
    }
    print_summary(&fits, failures.len());
    Ok(())
}

/// Milliseconds spent preparing the render graph, rendering for the fit
/// and fitting.
struct Timing {
    setup_ms: u128,
    render_ms: u128,
    renders: u32,
    fit_ms: u128,
}

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    templates: Vec<RawGpuProgramTemplate>,
    dump: Option<PathBuf>,
}

impl Harness {
    fn fit(&mut self, path: &Path) -> Result<(CameraLookFit, RawThumbnail, Timing)> {
        let preview =
            load_raw_embedded_thumbnail(path, PREVIEW_EDGE).context("read the embedded preview")?;
        let full = load_raw_file(path).context("decode the RAW")?;
        let started = Instant::now();
        let exposure = ExposureParams::default().source_reference();
        if full.uses_highlight_model(&exposure) {
            full.prime_highlight_model(&exposure);
        }
        let raw = if full.width.max(full.height) > PREVIEW_EDGE {
            build_proxy(
                &full,
                ProxySpec {
                    max_edge: PREVIEW_EDGE,
                },
            )
        } else {
            full
        };
        let masks = MaskStack::default();
        let params = GpuParams::new(&exposure, &masks, &raw);
        let quality = ProcessingQuality::Preview;
        let template = self
            .templates
            .iter()
            .find(|template| template.fits(&raw, quality))
            .cloned();
        let mut options = PipelineOptions::new(quality).mask_atlas_edge(64);
        if let Some(template) = &template {
            options = options.programs(template);
        }
        let pipeline = RawGpuPipeline::new(&self.device, &self.queue, &raw, &params, options)
            .context("build the render graph")?;
        if template.is_none() {
            self.templates.push(pipeline.program_template());
        }
        let render = |ev: f32, look: Option<&ResolvedLook>| -> Result<RawThumbnail> {
            let params = GpuParams::new(&exposure, &masks, &raw)
                .with_profile_exposure_offset(ev)
                .with_look_lut(look);
            pipeline.recompute(&self.queue, &self.device, &params);
            let rgba = pipeline.read_output_region_blocking(
                &self.device,
                &self.queue,
                0,
                0,
                pipeline.width,
                pipeline.height,
            )?;
            Ok(RawThumbnail {
                width: pipeline.width,
                height: pipeline.height,
                rgba,
            })
        };
        let setup = started.elapsed();
        let render_time = Cell::new(Duration::ZERO);
        let renders = Cell::new(0_u32);
        let fit_started = Instant::now();
        let fit = fit_camera_look(&preview, |ev| {
            let render_started = Instant::now();
            let image = render(ev, None);
            render_time.set(render_time.get() + render_started.elapsed());
            renders.set(renders.get() + 1);
            image
        })
        .map_err(|error| match error {
            CameraLookFitError::Render(error) => error,
            other => anyhow!(other),
        })?;
        let timing = Timing {
            setup_ms: setup.as_millis(),
            render_ms: render_time.get().as_millis(),
            renders: renders.get(),
            fit_ms: (fit_started.elapsed() - render_time.get()).as_millis(),
        };
        if let Some(directory) = &self.dump {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("photo");
            let lut = fit.full_look.resolved(None);
            save_png(&preview, &directory.join(format!("{stem}-1-camera.png")))?;
            save_png(
                &render(0.0, None)?,
                &directory.join(format!("{stem}-2-canonical.png")),
            )?;
            save_png(
                &render(0.0, Some(&lut))?,
                &directory.join(format!("{stem}-3-matched.png")),
            )?;
        }
        Ok((fit, preview, timing))
    }
}

fn save_png(image: &RawThumbnail, path: &Path) -> Result<()> {
    image::save_buffer(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
    )
    .with_context(|| format!("write {}", path.display()))
}

fn display_name(path: &Path) -> String {
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    let mut label = format!("{parent}/{name}");
    if label.chars().count() > 34 {
        label = label.chars().skip(label.chars().count() - 34).collect();
    }
    label
}

fn print_row(name: &str, fit: &CameraLookFit, preview: &RawThumbnail, timing: &Timing) {
    let ev = fit.full_look.exposure_ev();
    print!(
        "{name:<34} {:>9} {:>6.3} {:>5.3} {:>+6.2}",
        format!("{}x{}", preview.width, preview.height),
        fit.alignment.scale,
        fit.alignment.correlation,
        ev
    );
    for stage in STAGES {
        match fit.score(stage) {
            Some(score) => print!(" {:>5.2}/{:<5.2}", score.mean, score.p90),
            None => print!(" {:>11}", "-"),
        }
    }
    println!(
        " {:>5} {:>5} {:>8} {:>16}",
        fit.curve_matrix_bake.folded_cells,
        fit.full_bake.folded_cells,
        verdict_label(fit.verdict),
        format!(
            "{}/{}x{}/{}",
            timing.setup_ms, timing.renders, timing.render_ms, timing.fit_ms
        )
    );
}

fn verdict_label(verdict: FitVerdict) -> &'static str {
    match verdict {
        FitVerdict::Full => "full",
        FitVerdict::CurveAndMatrix => "c+m",
        FitVerdict::Rejected => "none",
    }
}

fn print_summary(fits: &[(String, CameraLookFit)], failures: usize) {
    println!();
    println!("{} fitted, {failures} skipped", fits.len());
    if fits.is_empty() {
        return;
    }
    println!("stage        mean of means   median of means   mean p90");
    for stage in STAGES {
        let mut means: Vec<f32> = fits
            .iter()
            .filter_map(|(_, fit)| fit.score(stage).map(|score| score.mean))
            .collect();
        let p90: Vec<f32> = fits
            .iter()
            .filter_map(|(_, fit)| fit.score(stage).map(|score| score.p90))
            .collect();
        let mean = means.iter().sum::<f32>() / means.len().max(1) as f32;
        means.sort_by(f32::total_cmp);
        let median = means.get(means.len() / 2).copied().unwrap_or(0.0);
        let p90_mean = p90.iter().sum::<f32>() / p90.len().max(1) as f32;
        println!(
            "{:<12} {mean:>13.2} {median:>17.2} {p90_mean:>10.2}",
            stage.label()
        );
    }
    let gain = |fit: &CameraLookFit, from: FitStage, to: FitStage| {
        Some(fit.score(from)?.mean - fit.score(to)?.mean)
    };
    let mut look_gains: Vec<f32> = fits
        .iter()
        .filter_map(|(_, fit)| gain(fit, FitStage::Canonical, FitStage::BakedMatrix))
        .collect();
    let mut residual_gains: Vec<f32> = fits
        .iter()
        .filter_map(|(_, fit)| gain(fit, FitStage::BakedMatrix, FitStage::BakedResidual))
        .collect();
    look_gains.sort_by(f32::total_cmp);
    residual_gains.sort_by(f32::total_cmp);
    println!("gain canonical -> baked c+m   : {}", quantiles(&look_gains));
    println!(
        "gain baked c+m -> baked full  : {}",
        quantiles(&residual_gains)
    );
    let verdicts = [
        FitVerdict::Full,
        FitVerdict::CurveAndMatrix,
        FitVerdict::Rejected,
    ];
    for verdict in verdicts {
        let count = fits
            .iter()
            .filter(|(_, fit)| fit.verdict == verdict)
            .count();
        println!("verdict {:<5}: {count}", verdict_label(verdict));
    }
}

fn quantiles(sorted: &[f32]) -> String {
    if sorted.is_empty() {
        return "-".to_owned();
    }
    let at = |share: f32| sorted[((sorted.len() - 1) as f32 * share).round() as usize];
    format!(
        "min {:.2}  p10 {:.2}  median {:.2}  p90 {:.2}  max {:.2}",
        at(0.0),
        at(0.1),
        at(0.5),
        at(0.9),
        at(1.0)
    )
}

fn parse_args() -> Result<Args> {
    let mut folders = Vec::new();
    let mut dump = None;
    let mut values = env::args().skip(1);
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--dump" => {
                dump = Some(PathBuf::from(
                    values.next().context("--dump requires a directory")?,
                ))
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other if other.starts_with("--") => bail!("unknown argument {other:?}; use --help"),
            folder => folders.push(PathBuf::from(folder)),
        }
    }
    if folders.is_empty() {
        print_help();
        bail!("missing folder");
    }
    Ok(Args { folders, dump })
}

fn print_help() {
    println!(concat!(
        "CalibRaw camera match diagnostics\n\n",
        "Usage: calibraw-camera-match FOLDER... [--dump DIRECTORY]\n\n",
        "Fits the camera look of every RAW in each folder against its embedded\n",
        "preview and prints the held-out CIEDE2000 (mean/p90) after each model\n",
        "stage, the alignment, the folded cells of the baked tables and the\n",
        "guard's verdict. --dump writes the camera preview, the canonical render\n",
        "and the render with the full model for each photo. Sidecars are\n",
        "neither read nor written."
    ));
}
