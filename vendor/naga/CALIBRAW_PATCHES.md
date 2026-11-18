Based on crates.io naga 29.0.4. The HLSL writer in src/back/hlsl/help.rs
uses the layer-count output of GetDimensions for array textures. Storage
arrays have no mip-count output. Sampled arrays report their layer count
before the mip count. Glow and Lens Blur use both queries. Recheck this
patch when updating wgpu.
