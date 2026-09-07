//! Run with `cargo run --release -p calibraw-core --example raw_open_bench -- photo.ARW`.
//! Times CPU opening stages and fingerprints corrected pixels/highlight results.
use calibraw_core::pipeline::*;
use std::{path::Path, time::Instant};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("provide a RAW path");
    let total = Instant::now();
    let stage = Instant::now();
    let raw = load_raw_file(Path::new(&path))?;
    println!(
        "decode: {:.3}s ({}x{})",
        stage.elapsed().as_secs_f64(),
        raw.width,
        raw.height
    );
    let stage = Instant::now();
    let catalog = lensfun_catalog(&raw);
    println!("lens catalog: {:.3}s", stage.elapsed().as_secs_f64());
    let stage = Instant::now();
    let raw = if let Some(lens) = catalog.auto_match {
        println!("lens: {}", lens.label());
        apply_lensfun_correction(&raw, &lens)?
    } else {
        raw
    };
    println!("lens correction: {:.3}s", stage.elapsed().as_secs_f64());
    let stage = Instant::now();
    let chroma = raw.inpaint_opposed_chroma_for_exposure(&ExposureParams::scene_referred_default());
    println!(
        "highlight analysis: {:.3}s; chroma: {chroma:?}",
        stage.elapsed().as_secs_f64()
    );
    let stage = Instant::now();
    let proxy = build_proxy(&raw, ProxySpec { max_edge: 800 });
    println!(
        "proxy: {:.3}s ({}x{})",
        stage.elapsed().as_secs_f64(),
        proxy.width,
        proxy.height
    );
    println!("CPU open total: {:.3}s", total.elapsed().as_secs_f64());
    let fingerprint = raw
        .raw_pixels
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, &value| {
            (hash ^ u64::from(value)).wrapping_mul(0x100000001b3)
        });
    println!("corrected pixel fingerprint: {fingerprint:016x}");
    Ok(())
}
