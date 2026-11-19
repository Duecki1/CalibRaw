Based on crates.io naga 29.0.4. The HLSL writer in src/back/hlsl/help.rs
uses the layer-count output of GetDimensions for array textures. Storage
arrays have no mip-count output. Sampled arrays report their layer count
before the mip count. Glow and Lens Blur use both queries. Recheck this
patch when updating wgpu.

The SPIR-V writer uses OpImageQuerySize for layer counts on storage and
multisampled images. It emits aggregate layout decorations only for types
reachable from uniform buffers, storage buffers and immediates. Naga shares
one type ID across address spaces, so keep buffer aggregate types out of
mutable shader locals and workgroup variables. When updating Naga, check the
generated binaries with spirv-val, including the RAW finishing pass, preview
histogram, Dehaze, Relight, image lights, Glow and Lens Blur.

The SPIR-V image bounds check uses the coordinate component type for its
constant one. Unsigned texture coordinates require unsigned constituents
in the vector subtracted from the image dimensions.
