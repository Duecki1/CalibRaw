#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(code) = calibraw::run_onnx_runtime_probe_cli(&args) {
        std::process::exit(code);
    }
    calibraw::run_desktop()
}

#[cfg(target_os = "android")]
fn main() {}
