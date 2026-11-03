// Temporary diagnostic: renders a RAW with a sidecar file. Not part of the change.
use calibraw_cli::pipeline::{
    load_raw_file, spawn_tiled_export, ExportBitDepth, ExportEvent, ExportFiles, ExportFormat,
    ExportMetadata, ExportSettings, ExportTarget, TileSpec, TiledExportJob,
};
use calibraw_gpu::{request_headless_device, wgpu, HeadlessAdapter, HeadlessDeviceRequest};
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};

fn main() {
    let mut args = std::env::args().skip(1);
    let raw_path = PathBuf::from(args.next().expect("raw"));
    let raw = Arc::new(load_raw_file(&raw_path).expect("load raw"));
    let device = request_headless_device(&HeadlessDeviceRequest {
        label: "sidecar render",
        adapter: HeadlessAdapter::PreferredOrSoftware(wgpu::PowerPreference::HighPerformance),
        texture_dimension: calibraw_cli::pipeline::export_mask_atlas_edge(raw.width, raw.height),
    })
    .expect("device");
    let metadata = ExportMetadata::from_raw(&raw, None);
    for pair in args.collect::<Vec<_>>().chunks(2) {
        let bytes = std::fs::read(&pair[0]).expect("read sidecar");
        let edits = calibraw_core::sidecar::decode(&bytes).expect("decode").edits;
        let receiver = spawn_tiled_export(
            ExportFormat::Png,
            TiledExportJob {
                device: device.device.clone(),
                queue: device.queue.clone(),
                raw: raw.clone(),
                geometry: edits.geometry.sanitized(),
                exposure: edits.exposure,
                masks: Arc::unwrap_or_clone(edits.masks),
                remove: Arc::unwrap_or_clone(edits.remove),
                look: None,
                files: ExportFiles::Single(ExportTarget::File(PathBuf::from(&pair[1]))),
                tile_spec: TileSpec::default(),
                settings: ExportSettings {
                    keep_metadata: false,
                    bit_depth: ExportBitDepth::Sixteen,
                    ..ExportSettings::default()
                },
                metadata: metadata.clone(),
                cancellation: Arc::new(AtomicBool::new(false)),
                program_prewarm: None,
            },
        );
        loop {
            match receiver.recv().expect("export stopped") {
                ExportEvent::Finished(result) => {
                    result.expect("export");
                    eprintln!("wrote {}", pair[1]);
                    break;
                }
                _ => {}
            }
        }
    }
}
