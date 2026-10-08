mod base64_arc_bytes;
pub mod color_math;
pub mod diagnostics;
pub mod file_ops;
pub mod matrix;
mod migrations;
pub mod pipeline;
pub mod presets;
pub mod sidecar;
pub mod thumbnail_cache;

#[used]
pub static SOURCE_REVISION: &str = env!("CALIBRAW_SOURCE_REVISION");
