//! Starting the desktop program.

use crate::platform::PlatformCaps;

pub(crate) fn main() -> eframe::Result {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(code) = run_onnx_runtime_probe_cli(&args) {
        std::process::exit(code);
    }
    run_desktop()
}

fn run_onnx_runtime_probe_cli(args: &[String]) -> Option<i32> {
    if args.len() != 4 || args.get(1).map(String::as_str) != Some("--calibraw-onnx-runtime-probe") {
        return None;
    }
    let path = std::path::Path::new(&args[2]);
    let probe = calibraw_ai::Sha256::parse(&args[3])
        .and_then(|sha256| calibraw_ai::runtime::run_runtime_probe_process(path, &sha256));
    match probe {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("CalibRaw ONNX Runtime probe failed: {error:#}");
            Some(2)
        }
    }
}

fn desktop_icon() -> std::sync::Arc<eframe::egui::IconData> {
    static ICON: std::sync::OnceLock<std::sync::Arc<eframe::egui::IconData>> =
        std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!(
            "../../../../../packaging/icons/calibraw-256.png"
        ))
        .expect("embedded desktop icon must be a valid PNG")
        .into_rgba8();
        let (width, height) = image.dimensions();
        std::sync::Arc::new(eframe::egui::IconData {
            rgba: image.into_raw(),
            width,
            height,
        })
    })
    .clone()
}

fn run_desktop() -> eframe::Result {
    env_logger::init();
    #[cfg(target_os = "windows")]
    super::nvidia_vrr::keep_gsync_off_for_window();

    if std::env::var_os("CALIBRAW_GPU_STAGE_TIMING").is_some_and(|value| value == "1") {
        calibraw_gpu::stage_timing::request();
    }
    let mut options = crate::native_options(PlatformCaps::DESKTOP);
    options.viewport = eframe::egui::ViewportBuilder::default()
        .with_app_id(if super::portal::is_flatpak() {
            "io.github.Duecki1.CalibRaw"
        } else {
            "de.duecki.calibraw"
        })
        .with_inner_size([1280.0, 720.0])
        .with_min_inner_size([480.0, 480.0])
        .with_icon(desktop_icon());

    eframe::run_native(
        "CalibRaw",
        options,
        Box::new(|cc| Ok(crate::app::create_app(cc, super::DesktopPlatform::new()))),
    )
}
