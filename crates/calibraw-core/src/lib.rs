mod base64_arc_bytes;
pub mod color_math;
pub mod diagnostics;
pub mod display_metadata_cache;
pub mod file_ops;
mod fnv;
pub mod luts;
pub mod matrix;
pub mod migrations;
pub mod pipeline;
pub mod presets;
pub mod serialized_reads;
pub mod sidecar;
pub mod system_memory;
pub mod thumbnail_cache;

#[used]
pub static SOURCE_REVISION: &str = env!("CALIBRAW_SOURCE_REVISION");
