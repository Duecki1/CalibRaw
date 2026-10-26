//! Every published model: what is downloaded, how its session is set up and
//! how long it stays loaded. Download URLs, SHA-256 pins and cache file names
//! are on-disk state; change them only together with a new artifact.

pub(crate) mod artifact;
pub(crate) mod install;

use crate::pipeline::BIG_LAMA_INPUT_EDGE;
use crate::placement::quirks::WebGpuCpuNodes;
use crate::placement::{CpuFallbackProfile, SessionOptions};
use crate::slot::ModelRetention;
use artifact::{DownloadOptions, ModelArtifact};
use install::ModelInstallSpec;
use std::time::Duration;

/// A published model; each has one [`ModelSpec`] and its own session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AiModel {
    BiRefNet,
    BiRefNetLow,
    BiRefNetFp16,
    BiRefNetLowFp16,
    SkySeg,
    Depth,
    SamEncoder,
    SamDecoder,
    BigLama,
    RawNindBayer,
    RawNindLinear,
    PersonDetector,
    BodyParts,
}

impl AiModel {
    pub(crate) const fn spec(self) -> &'static ModelSpec {
        match self {
            Self::BiRefNet => &BIREFNET,
            Self::BiRefNetLow => &BIREFNET_LOW,
            Self::BiRefNetFp16 => &BIREFNET_FP16,
            Self::BiRefNetLowFp16 => &BIREFNET_LOW_FP16,
            Self::SkySeg => &SKYSEG,
            Self::Depth => &DEPTH,
            Self::SamEncoder => &SAM_ENCODER,
            Self::SamDecoder => &SAM_DECODER,
            Self::BigLama => &BIG_LAMA,
            Self::RawNindBayer => &RAWNIND_BAYER,
            Self::RawNindLinear => &RAWNIND_LINEAR,
            Self::PersonDetector => &PERSON_DETECTOR,
            Self::BodyParts => &BODY_PARTS,
        }
    }
}

/// What a local model is used for, independent of where in the UI it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiFeature {
    Subject,
    Sky,
    SceneDepth,
    Object,
    Remove,
    Denoise,
    People,
}

impl AiFeature {
    pub const ALL: [Self; 7] = [
        Self::Subject,
        Self::Sky,
        Self::SceneDepth,
        Self::Object,
        Self::Remove,
        Self::Denoise,
        Self::People,
    ];
}

pub(crate) struct ModelSpec {
    pub(crate) id: AiModel,
    pub(crate) feature: AiFeature,
    pub(crate) artifact: ModelArtifact,
    pub(crate) source: ModelSource,
    pub(crate) input: InputShape,
    /// Session settings; `model_name` names the model in diagnostics, provider
    /// statuses and the GPU job record.
    pub(crate) session: SessionOptions,
    /// How long the session stays loaded where the platform keeps sessions
    /// warm; the model code applies the platform's limits.
    pub(crate) retention: ModelRetention,
}

impl ModelSpec {
    /// What the app shows about this model.
    pub(crate) const fn info(&self) -> ModelInfo {
        ModelInfo {
            name: self.artifact.name,
            download_label: match self.source {
                ModelSource::Download { progress_label, .. } => progress_label,
                ModelSource::Package { .. } => self.artifact.name,
            },
            cache_filename: self.artifact.cache_filename,
            download_bytes: self.artifact.bytes,
            sha256: self.artifact.sha256,
            input: self.input,
        }
    }
}

/// Where a model file comes from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ModelSource {
    /// Downloaded on its own from `ModelArtifact::url`.
    Download {
        options: DownloadOptions,
        progress_label: &'static str,
    },
    /// Extracted from `RAWNIND_PACKAGE`, where it is stored as `member`.
    Package { member: &'static str },
}

/// The input a model sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputShape {
    /// The photo or crop scaled to `edge` x `edge` pixels.
    Square { edge: u32 },
    /// `tile` x `tile` pixel tiles that overlap their neighbours by `overlap`
    /// pixels on each side.
    Tiled { tile: u32, overlap: u32 },
    /// Encoder features plus prompts in the encoder's `edge` x `edge` frame,
    /// and the previous mask at `mask_edge` x `mask_edge`.
    Prompted { edge: u32, mask_edge: u32 },
    /// The photo or crop scaled to `width` x `height` pixels, whatever its
    /// aspect ratio.
    Stretched { width: u32, height: u32 },
}

impl InputShape {
    /// Longest edge the model sees, in pixels: the whole input, one tile,
    /// or the frame prompts are placed in.
    pub const fn edge(self) -> u32 {
        match self {
            Self::Square { edge } | Self::Prompted { edge, .. } => edge,
            Self::Tiled { tile, .. } => tile,
            Self::Stretched { width, height } => {
                if width > height {
                    width
                } else {
                    height
                }
            }
        }
    }
}

/// What the app shows about a published model and where it caches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Name in consent text and progress.
    pub name: &'static str,
    /// Label of its download progress.
    pub download_label: &'static str,
    /// File name in the model folder.
    pub cache_filename: &'static str,
    pub download_bytes: u64,
    /// Lowercase hexadecimal SHA-256 of the file.
    pub sha256: &'static str,
    pub input: InputShape,
}

const MIB: u64 = 1 << 20;

const MASK_MODEL_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(45),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(60 * 60),
    attempts: 5,
    resume: true,
};
const SAM_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(45),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(30 * 60),
    attempts: 5,
    resume: true,
};
const BIG_LAMA_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(30),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(30 * 60),
    attempts: 5,
    resume: true,
};
const RAWNIND_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(45),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(30 * 60),
    attempts: 5,
    resume: true,
};

/// Peak GPU memory of inference at the model's input size, `mib` MiB, for
/// `SessionOptions::gpu_memory_bytes`. Measured on an NVIDIA GeForce RTX
/// 4070 Ti (driver 615.71, Linux, Vulkan) with ONNX Runtime 1.30.0 and its
/// WebGPU plugin 0.4.0, set up as `placement` sets up sessions (bucketed
/// buffer cache, BiRefNet's decoder splits on the CPU): the most GPU memory
/// the process held while creating the session and running it three times,
/// sampled through NVML every 5 ms, less the 15 MiB it held with only the
/// provider's device, rounded up to 100 MiB. Direct3D 12 and Metal are
/// assumed to need about as much. Placement adds a margin.
const fn measured_peak(mib: u64) -> u64 {
    mib * MIB
}

const fn birefnet_session(peak: u64) -> SessionOptions {
    SessionOptions::new("BiRefNet")
        .with_gpu_memory(peak)
        .with_webgpu_cpu_nodes(WebGpuCpuNodes::BiRefNetDecoderSplits)
}

/// The BiRefNet General-Lite subject model: 1024 x 1024 input, 224 MB.
/// `birefnet-lowQ.onnx` with its deformable convolutions rewritten by
/// `packaging/models/birefnet_gridsample.py`. The original export peaks near
/// 6.5 GB and overruns GPU memory under DirectML; this one peaks near 2 GB and
/// matches it to float32 rounding. The cache filename predates the rewrite;
/// the changed SHA-256 replaces older downloads.
const BIREFNET: ModelSpec = ModelSpec {
    id: AiModel::BiRefNet,
    feature: AiFeature::Subject,
    artifact: ModelArtifact {
        name: "BiRefNet General-Lite",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lowQ-gridsample.onnx"),
        sha256: "7c0194fad87cc56371054c14e3cff16ee40b095dcae1183321e6c4b5bb8e21d4",
        bytes: 223_665_266,
        cache_filename: "birefnet-general-lite.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "BiRefNet General-Lite",
    },
    input: InputShape::Square { edge: 1024 },
    // Measured 4557 MiB.
    // CPU peak: 5093 MiB with ONNX Runtime 1.24.2 on Apple Silicon,
    // sequential execution, two threads, no arena or weight prepacking and
    // memory patterns disabled. Rounded up; placement adds a margin before
    // admitting the session against iOS's process memory allowance.
    session: birefnet_session(measured_peak(4600)).with_cpu_memory(5300 * MIB),
    retention: ModelRetention::WhileWarm,
};

/// The same checkpoint exported at 768 x 768 by
/// `packaging/models/birefnet_export.py` and rewritten by
/// `birefnet_gridsample.py`, so its operators match `BIREFNET`'s. About half
/// the inference cost; boundaries are softer and small details may drop.
const BIREFNET_LOW: ModelSpec = ModelSpec {
    id: AiModel::BiRefNetLow,
    feature: AiFeature::Subject,
    artifact: ModelArtifact {
        name: "BiRefNet General-Lite 768",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lite-768-gridsample.onnx"),
        sha256: "f5c9e0044d82d9ce8023b2f3ce4b7879621361630da3ee6a78315f27f1238b85",
        bytes: 204_958_552,
        cache_filename: "birefnet-general-lite-768.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "BiRefNet General-Lite 768",
    },
    input: InputShape::Square { edge: 768 },
    // Measured 3018 MiB.
    // CPU peak: 4643 MiB under the same configuration as BIREFNET above.
    session: birefnet_session(measured_peak(3100)).with_cpu_memory(4800 * MIB),
    retention: ModelRetention::WhileWarm,
};

/// Whether subject masks expected on the GPU use the float16 BiRefNet
/// conversions below instead of float32: only once the parity harness
/// confirms them on the GPU provider. On the
/// WebGPU provider of `measured_peak`'s machine, Medium's masks reached an
/// IoU of 0.984 against the float32 CPU goldens and Low's 0.995, where the
/// harness asks for 0.995, for about 10% less time per mask.
pub(crate) const BIREFNET_FLOAT16_ON_GPU: bool = false;

/// `BIREFNET` converted by `packaging/models/birefnet_fp16.py` for GPU
/// sessions. Needs about two thirds of the GPU memory on WebGPU; on CUDA its
/// masks matched float32 with an IoU of 0.9989.
const BIREFNET_FP16: ModelSpec = ModelSpec {
    id: AiModel::BiRefNetFp16,
    feature: AiFeature::Subject,
    artifact: ModelArtifact {
        name: "BiRefNet General-Lite",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lowQ-gridsample-fp16.onnx"),
        sha256: "4966ed930390ec1570f4d6be7d003f2e6a275ffa26d3a8f34c80982a56b16ff3",
        bytes: 114_115_335,
        cache_filename: "birefnet-general-lite-fp16.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "BiRefNet General-Lite (GPU)",
    },
    input: InputShape::Square { edge: 1024 },
    // Measured 2886 MiB.
    session: birefnet_session(measured_peak(2900)),
    retention: ModelRetention::WhileWarm,
};

/// `BIREFNET_LOW` converted by `packaging/models/birefnet_fp16.py` for GPU
/// sessions; masks match float32 with an IoU of 0.9982.
const BIREFNET_LOW_FP16: ModelSpec = ModelSpec {
    id: AiModel::BiRefNetLowFp16,
    feature: AiFeature::Subject,
    artifact: ModelArtifact {
        name: "BiRefNet General-Lite 768",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lite-768-gridsample-fp16.onnx"),
        sha256: "d6d1c6dc4cb451fa1e56f3cbb1035dd803e6a4fc04e5c71ca0f373d0a71f7b44",
        bytes: 104_756_493,
        cache_filename: "birefnet-general-lite-768-fp16.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "BiRefNet General-Lite 768 (GPU)",
    },
    input: InputShape::Square { edge: 768 },
    // Measured 1713 MiB.
    session: birefnet_session(measured_peak(1800)),
    retention: ModelRetention::WhileWarm,
};

const SKYSEG: ModelSpec = ModelSpec {
    id: AiModel::SkySeg,
    feature: AiFeature::Sky,
    artifact: ModelArtifact {
        name: "SkySeg U2Net",
        url: Some(
            "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/skyseg/skyseg.onnx",
        ),
        sha256: "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
        bytes: 175_997_079,
        cache_filename: "skyseg-u2net-fp32.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "SkySeg U2Net",
    },
    input: InputShape::Square { edge: 320 },
    // Measured 650 MiB.
    session: SessionOptions::new("SkySeg U2Net").with_gpu_memory(measured_peak(700)),
    retention: ModelRetention::WhileWarm,
};

/// Depth Anything V2 Small on every platform. It predicts inverse depth
/// (high values near) from an NCHW image and returns `[1, height, width]`.
const DEPTH: ModelSpec = ModelSpec {
    id: AiModel::Depth,
    feature: AiFeature::SceneDepth,
    artifact: ModelArtifact {
        name: "Depth Anything V2 Small",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/da2/depth_anything_v2_small_fp32.onnx"),
        sha256: "afb6a5c28f3b6bf1618c6e43f02073ef9dfdc70e937502d51603e57b0a1df10c",
        bytes: 99_060_839,
        cache_filename: "depth_anything_v2_small_fp32.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "Depth Anything V2 Small",
    },
    input: InputShape::Square { edge: 518 },
    // Measured 530 MiB.
    session: SessionOptions::new("Depth Anything V2 Small").with_gpu_memory(measured_peak(600)),
    retention: ModelRetention::WhileWarm,
};

const SAM_ENCODER: ModelSpec = ModelSpec {
    id: AiModel::SamEncoder,
    feature: AiFeature::Object,
    artifact: ModelArtifact {
        name: "SAM 2.1 encoder",
        url: Some(concat!(
            "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/",
            "91085ce0ec322a4a7cbd20059688690218e52f9a/",
            "models/sam2/sam2.1-hiera-tiny.encoder.onnx"
        )),
        sha256: "667384d1e686de6828b841ac8a24db0fafa2b3452494225f82eeedac56141230",
        bytes: 109_471_931,
        cache_filename: "sam2.1-hiera-tiny.encoder.onnx",
    },
    source: ModelSource::Download {
        options: SAM_DOWNLOAD,
        progress_label: "SAM 2.1 encoder",
    },
    input: InputShape::Square { edge: 1024 },
    // Measured 2088 MiB.
    session: SessionOptions::new("SAM 2.1 encoder")
        .with_gpu_memory(measured_peak(2100))
        .with_cpu_fallback_profile(CpuFallbackProfile::SamEncoder),
    retention: ModelRetention::WhileWarm,
};

const SAM_DECODER: ModelSpec = ModelSpec {
    id: AiModel::SamDecoder,
    feature: AiFeature::Object,
    artifact: ModelArtifact {
        name: "SAM 2.1 decoder",
        url: Some(concat!(
            "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/",
            "91085ce0ec322a4a7cbd20059688690218e52f9a/",
            "models/sam2/sam2.1-hiera-tiny.decoder.onnx"
        )),
        sha256: "c40f5aa7d37b681cd500481a85d44839fd81c93dce1e86271a2c866470d22105",
        bytes: 16_519_561,
        cache_filename: "sam2.1-hiera-tiny.decoder.onnx",
    },
    source: ModelSource::Download {
        options: SAM_DOWNLOAD,
        progress_label: "SAM 2.1 decoder",
    },
    input: InputShape::Prompted {
        edge: SAM_ENCODER.input.edge(),
        mask_edge: 256,
    },
    // Measured 94 MiB.
    session: SessionOptions::new("SAM 2.1 decoder").with_gpu_memory(measured_peak(100)),
    retention: ModelRetention::WhileWarm,
};

/// Carve/LaMa-ONNX's port of the original PyTorch big-lama inpainting model.
const BIG_LAMA: ModelSpec = ModelSpec {
    id: AiModel::BigLama,
    feature: AiFeature::Remove,
    artifact: ModelArtifact {
        name: "Big-LaMa Places2 ONNX",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/91085ce0ec322a4a7cbd20059688690218e52f9a/models/lama/lama_fp32.onnx"),
        sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
        bytes: 208_044_816,
        cache_filename: "big-lama-places2-fp32-512.onnx",
    },
    source: ModelSource::Download {
        options: BIG_LAMA_DOWNLOAD,
        progress_label: "Big-LaMa Remove model",
    },
    input: InputShape::Square {
        edge: BIG_LAMA_INPUT_EDGE,
    },
    // Measured 773 MiB; the Remove job's own wgpu memory is not included.
    session: SessionOptions::new("Big-LaMa Remove").with_gpu_memory(measured_peak(800)),
    retention: ModelRetention::WhileWarm,
};

/// The package holding both RawNIND models. Its cache file is removed once
/// both are extracted.
pub(crate) const RAWNIND_PACKAGE: ModelInstallSpec = ModelInstallSpec {
    artifact: ModelArtifact {
        name: "RawNIND model package",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/91085ce0ec322a4a7cbd20059688690218e52f9a/models/rawnind/rawdenoise-nind.dtmodel"),
        sha256: "d71b5f1e727c85a359e6f74dca9e2016c9d8fc3e2f7ac3e9b347d80ceca969af",
        bytes: 57_700_134,
        cache_filename: "rawdenoise-nind.dtmodel",
    },
    download: RAWNIND_DOWNLOAD,
    progress_label: "RawNIND model package",
};

/// RawNIND tiles: 512 x 512 pixels with a 64-pixel overlap on each side.
const RAWNIND_INPUT: InputShape = InputShape::Tiled {
    tile: 512,
    overlap: 64,
};

const RAWNIND_BAYER: ModelSpec = ModelSpec {
    id: AiModel::RawNindBayer,
    feature: AiFeature::Denoise,
    artifact: ModelArtifact {
        name: "RawNIND Bayer model",
        url: None,
        sha256: "da27509dab6a2915da67e988acd86cf71f9d5bbc8d1aa0ed32933578a887b901",
        bytes: 31_056_425,
        cache_filename: "model_bayer.onnx",
    },
    source: ModelSource::Package {
        member: "rawdenoise-nind/model_bayer.onnx",
    },
    input: RAWNIND_INPUT,
    // Measured 310 MiB, one tile per run.
    session: SessionOptions::new("RawNIND Bayer").with_gpu_memory(measured_peak(400)),
    retention: ModelRetention::OneShot,
};

const RAWNIND_LINEAR: ModelSpec = ModelSpec {
    id: AiModel::RawNindLinear,
    feature: AiFeature::Denoise,
    artifact: ModelArtifact {
        name: "RawNIND linear model",
        url: None,
        sha256: "df957efadcc152c007d5d3b0917bdff9e41c0d4a0efe56584ef30b36393cd181",
        bytes: 31_053_823,
        cache_filename: "model_linear.onnx",
    },
    source: ModelSource::Package {
        member: "rawdenoise-nind/model_linear.onnx",
    },
    input: RAWNIND_INPUT,
    // Measured 290 MiB, one tile per run.
    session: SessionOptions::new("RawNIND linear").with_gpu_memory(measured_peak(300)),
    retention: ModelRetention::OneShot,
};

/// RF-DETR-Seg Medium (Apache-2.0), exported by
/// `packaging/models/rfdetr_seg_export.py`: finds the people in a photo and
/// outlines them coarsely, which decides whose a body part is. It takes the
/// photo stretched to 432 x 432 and returns, for 200 queries, a box, COCO
/// class logits and 108 x 108 mask logits over the whole photo. Medium
/// rather than Nano or Small: within 7 MB of their download and 0.1 s of
/// their CPU time, it found twice the people Small found in a street crowd.
const PERSON_DETECTOR: ModelSpec = ModelSpec {
    id: AiModel::PersonDetector,
    feature: AiFeature::People,
    artifact: ModelArtifact {
        name: "RF-DETR-Seg Medium",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/rfdetr/rfdetr-seg-medium.onnx"),
        sha256: "7978ac93bbdb40c8eef0c7074a5296e7b2fbc7962dcd8a6eb4bcc6e141e484dd",
        bytes: 130_088_310,
        cache_filename: "rfdetr-seg-medium.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "RF-DETR-Seg Medium",
    },
    input: InputShape::Square { edge: 432 },
    // Measured 403 MiB.
    session: SessionOptions::new("RF-DETR-Seg Medium").with_gpu_memory(measured_peak(500)),
    retention: ModelRetention::WhileWarm,
};

/// Sapiens2-0.4B body-part segmentation (Sapiens2 License), exported by
/// `packaging/models/sapiens2_seg_export.py`: 29 classes per pixel of one
/// person's crop, stretched to 768 x 1024. Its weights are stored as float16
/// and cast to float32 when the session is created, so it computes in
/// float32 from half the download; computing in float16 on WebGPU missed the
/// parity harness's IoU.
const BODY_PARTS: ModelSpec = ModelSpec {
    id: AiModel::BodyParts,
    feature: AiFeature::People,
    artifact: ModelArtifact {
        name: "Sapiens2 0.4B body parts",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/sapiens2/sapiens2-seg-0.4b-fp16-weights.onnx"),
        sha256: "d8350f109b1a91ae84680366d1863e5466e049c3e2b62c4c813b56930ac64e04",
        bytes: 819_707_563,
        cache_filename: "sapiens2-seg-0.4b-fp16-weights.onnx",
    },
    source: ModelSource::Download {
        options: MASK_MODEL_DOWNLOAD,
        progress_label: "Sapiens2 0.4B body parts",
    },
    input: InputShape::Stretched {
        width: 768,
        height: 1024,
    },
    // Measured 4531 MiB.
    session: SessionOptions::new("Sapiens2 0.4B body parts").with_gpu_memory(measured_peak(4600)),
    retention: ModelRetention::WhileWarm,
};
