// Temporary diagnostic: runs the depth job on a photo and writes a sidecar
// with depth and normals and one with depth alone. Not part of the change.
use calibraw_ai::models::depth::SceneGeometryModel;
use calibraw_ai::models::masks::{spawn_ai_mask, AiMaskModel, AiMaskWorkerRequest};
use calibraw_ai::models::scene_geometry::SceneDepthQuality;
use calibraw_ai::pipeline::MaskImage;
use calibraw_ai::AiJobEvent;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [photo, models, quality, sidecar, with_normals, without_normals] = &args[..] else {
        panic!("photo models low|medium sidecar with without");
    };
    let photo = image::open(photo).unwrap().to_rgba8();
    let photo = image::imageops::resize(&photo, 2048, 2048 * photo.height() / photo.width(), image::imageops::FilterType::Triangle);
    let models = PathBuf::from(models);
    let quality = if quality == "medium" { SceneDepthQuality::Medium } else { SceneDepthQuality::Low };
    let events = spawn_ai_mask(
        AiMaskWorkerRequest {
            model: AiMaskModel::Depth,
            subject_quality: Default::default(),
            subject_precision: Default::default(),
            scene_geometry: Some(SceneGeometryModel { path: models.join(quality.model().cache_filename), quality }),
            model_path: models.join(calibraw_ai::models::depth::MODEL.cache_filename),
            allow_download: false,
            width: photo.width(),
            height: photo.height(),
            rgba: photo.into_raw(),
        },
        Arc::new(AtomicBool::new(false)),
    );
    let result = loop {
        if let AiJobEvent::Finished(result) = events.recv().unwrap() {
            break result.unwrap();
        }
    };
    let loaded = calibraw_core::sidecar::decode(&std::fs::read(sidecar).unwrap()).unwrap();
    for (path, normals) in [(with_normals, result.normals.clone()), (without_normals, None)] {
        let mut edits = loaded.edits.clone();
        let masks = Arc::make_mut(&mut edits.masks);
        masks.scene_depth = MaskImage::new(result.width, result.height, result.mask.clone());
        masks.scene_normals = normals;
        let bytes = calibraw_core::sidecar::encode_with_review(edits, loaded.review).unwrap();
        std::fs::write(path, bytes).unwrap();
        eprintln!("wrote {path}");
    }
}
