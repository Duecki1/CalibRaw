use super::*;

#[derive(Clone, Copy)]
pub(super) struct TextureReadbackRegion {
    pub origin: [u32; 2],
    pub extent: [u32; 2],
    pub texture_extent: [u32; 2],
    pub label: &'static str,
}

impl TextureReadbackRegion {
    pub(super) fn full(width: u32, height: u32, label: &'static str) -> Self {
        Self {
            origin: [0, 0],
            extent: [width, height],
            texture_extent: [width, height],
            label,
        }
    }

    fn validate(self, format: &'static str) -> Result<()> {
        let [x, y] = self.origin;
        let [width, height] = self.extent;
        let [texture_width, texture_height] = self.texture_extent;
        let right = x
            .checked_add(width)
            .ok_or_else(|| anyhow!("GPU readback rectangle overflows horizontally"))?;
        let bottom = y
            .checked_add(height)
            .ok_or_else(|| anyhow!("GPU readback rectangle overflows vertically"))?;
        if width == 0 || height == 0 || right > texture_width || bottom > texture_height {
            return Err(anyhow!("invalid GPU {format} readback rectangle"));
        }
        Ok(())
    }
}

pub(super) fn read_rgba8_texture_region_blocking(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    region: TextureReadbackRegion,
) -> Result<Vec<u8>> {
    region.validate("RGBA8")?;
    let [width, height] = region.extent;
    let label = region.label;

    let (readback, unpadded_bytes_per_row, padded_bytes_per_row) =
        create_readback_buffer(device, width, height, 4, "RGBA8", label)?;
    let submission = submit_texture_to_buffer_copy(
        device,
        queue,
        texture,
        region.origin,
        region.extent,
        &readback,
        padded_bytes_per_row,
        label,
    );
    let receiver = map_read_async(&readback);
    wait_for_mapping(device, submission, receiver, "thumbnail", "thumbnail")?;

    let mapped = readback.get_mapped_range(..);
    let rgba_len = usize::try_from(
        u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| anyhow!("GPU RGBA8 output size overflows"))?,
    )
    .map_err(|_| anyhow!("GPU RGBA8 output size does not fit in usize"))?;
    let padded_row = usize::try_from(padded_bytes_per_row)
        .map_err(|_| anyhow!("GPU RGBA8 padded row size does not fit in usize"))?;
    let unpadded_row = usize::try_from(unpadded_bytes_per_row)
        .map_err(|_| anyhow!("GPU RGBA8 row size does not fit in usize"))?;
    let row_count = usize::try_from(height)
        .map_err(|_| anyhow!("GPU RGBA8 row count does not fit in usize"))?;
    let mut rgba = vec![0u8; rgba_len];
    for row in 0..row_count {
        let source = row
            .checked_mul(padded_row)
            .ok_or_else(|| anyhow!("GPU RGBA8 source row offset overflows"))?;
        let source_end = source
            .checked_add(unpadded_row)
            .ok_or_else(|| anyhow!("GPU RGBA8 source row end overflows"))?;
        let destination = row
            .checked_mul(unpadded_row)
            .ok_or_else(|| anyhow!("GPU RGBA8 destination row offset overflows"))?;
        let destination_end = destination
            .checked_add(unpadded_row)
            .ok_or_else(|| anyhow!("GPU RGBA8 destination row end overflows"))?;
        let source_slice = mapped
            .get(source..source_end)
            .ok_or_else(|| anyhow!("GPU RGBA8 mapped row is shorter than requested"))?;
        let destination_slice = rgba
            .get_mut(destination..destination_end)
            .ok_or_else(|| anyhow!("GPU RGBA8 output row is outside its allocation"))?;
        destination_slice.copy_from_slice(source_slice);
    }
    drop(mapped);
    readback.unmap();
    Ok(rgba)
}

pub struct PendingRgba32Readback {
    readback: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    receiver: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
}

impl PendingRgba32Readback {
    pub fn finish(self, device: &wgpu::Device) -> Result<Vec<f32>> {
        self.finish_with_context(
            device,
            "pipelined export",
            "export",
            "display-linear export readback",
        )
    }

    fn finish_with_context(
        self,
        device: &wgpu::Device,
        operation: &str,
        label: &str,
        output_label: &str,
    ) -> Result<Vec<f32>> {
        wait_for_mapping(device, self.submission, self.receiver, operation, label)?;

        let mapped = self.readback.get_mapped_range(..);
        let capacity = usize::try_from(
            u64::from(self.width)
                .checked_mul(u64::from(self.height))
                .and_then(|value| value.checked_mul(3))
                .ok_or_else(|| anyhow!("GPU RGBA32F output size overflows"))?,
        )
        .map_err(|_| anyhow!("GPU RGBA32F output size does not fit in usize"))?;
        let padded_row = usize::try_from(self.padded_bytes_per_row)
            .map_err(|_| anyhow!("GPU RGBA32F padded row size does not fit in usize"))?;
        let row_bytes = usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(16))
            .ok_or_else(|| anyhow!("GPU RGBA32F mapped row byte count overflows"))?;
        let row_count = usize::try_from(self.height)
            .map_err(|_| anyhow!("GPU RGBA32F row count does not fit in usize"))?;
        let mut rgb = Vec::with_capacity(capacity);
        for row in 0..row_count {
            let row_start = row
                .checked_mul(padded_row)
                .ok_or_else(|| anyhow!("GPU RGBA32F mapped row offset overflows"))?;
            let row_end = row_start
                .checked_add(row_bytes)
                .ok_or_else(|| anyhow!("GPU RGBA32F mapped row end overflows"))?;
            let row_slice = mapped
                .get(row_start..row_end)
                .ok_or_else(|| anyhow!("GPU RGBA32F mapped row is shorter than requested"))?;
            for pixel in row_slice.chunks_exact(16) {
                for channel in 0..3 {
                    let offset = channel * 4;
                    let bytes = <[u8; 4]>::try_from(&pixel[offset..offset + 4])
                        .map_err(|_| anyhow!("GPU RGBA32F channel has an invalid width"))?;
                    rgb.push(f32::from_le_bytes(bytes));
                }
            }
        }
        drop(mapped);
        self.readback.unmap();
        if rgb.iter().any(|value| !value.is_finite()) {
            return Err(anyhow!("{output_label} contains NaN or infinity"));
        }
        Ok(rgb)
    }
}

pub(super) fn begin_rgba32_texture_region_rgb_readback(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    region: TextureReadbackRegion,
) -> Result<PendingRgba32Readback> {
    region.validate("RGBA32F")?;
    let [width, height] = region.extent;
    let label = region.label;

    let (readback, _, padded_bytes_per_row) =
        create_readback_buffer(device, width, height, 16, "RGBA32F", label)?;
    let submission = submit_texture_to_buffer_copy(
        device,
        queue,
        texture,
        region.origin,
        region.extent,
        &readback,
        padded_bytes_per_row,
        label,
    );
    let receiver = map_read_async(&readback);
    Ok(PendingRgba32Readback {
        readback,
        submission,
        receiver,
        width,
        height,
        padded_bytes_per_row,
    })
}

pub(super) const MAX_RGBA32_READBACK_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

fn rgba32_readback_rows_per_chunk(width: u32) -> Result<u32> {
    if width == 0 {
        return Err(anyhow!("GPU RGBA32F readback width is zero"));
    }
    let (_, padded_bytes_per_row) = padded_bytes_per_row(width, 16, "RGBA32F")?;
    let rows = MAX_RGBA32_READBACK_CHUNK_BYTES / u64::from(padded_bytes_per_row);
    if rows == 0 {
        return Err(anyhow!(
            "one GPU RGBA32F readback row ({padded_bytes_per_row} bytes) exceeds the chunk limit"
        ));
    }
    Ok(rows.min(u64::from(u32::MAX)) as u32)
}

pub(super) fn read_rgba32_texture_region_rgb_blocking(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    region: TextureReadbackRegion,
) -> Result<Vec<f32>> {
    region.validate("RGBA32F")?;
    let [width, height] = region.extent;

    let rows_per_chunk = rgba32_readback_rows_per_chunk(width)?;
    let capacity = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| anyhow!("GPU RGBA32F readback output size overflows"))?;
    let mut rgb = Vec::with_capacity(capacity);
    let mut row_offset = 0u32;

    while row_offset < height {
        let chunk_height = rows_per_chunk.min(height - row_offset);
        let pending = begin_rgba32_texture_region_rgb_readback(
            device,
            queue,
            texture,
            TextureReadbackRegion {
                origin: [region.origin[0], region.origin[1] + row_offset],
                extent: [width, chunk_height],
                ..region
            },
        )?;
        rgb.extend(pending.finish_with_context(
            device,
            "scene",
            "scene",
            "scene texture readback",
        )?);
        row_offset += chunk_height;
    }

    Ok(rgb)
}

pub(super) fn read_rgba32_texture_rgb_blocking(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    label: &'static str,
) -> Result<Vec<f32>> {
    read_rgba32_texture_region_rgb_blocking(
        device,
        queue,
        texture,
        TextureReadbackRegion::full(width, height, label),
    )
}

/// Reads one RGB pixel from a floating point texture, supporting both preview
/// (RGBA16F) and export (RGBA32F) processing surfaces.
pub(super) fn read_float_texture_pixel_blocking(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    x: u32,
    y: u32,
) -> Result<[f32; 3]> {
    if x >= texture.width() || y >= texture.height() {
        return Err(anyhow!(
            "GPU float readback pixel is outside texture bounds"
        ));
    }
    let bytes_per_pixel = match format {
        wgpu::TextureFormat::Rgba16Float => 8u32,
        wgpu::TextureFormat::Rgba32Float => 16u32,
        other => return Err(anyhow!("unsupported GPU float readback format {other:?}")),
    };
    let padded_bytes_per_row = 256u32;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("calibraw float pixel readback"),
        size: u64::from(padded_bytes_per_row),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let submission = submit_texture_to_buffer_copy(
        device,
        queue,
        texture,
        [x, y],
        [1, 1],
        &readback,
        padded_bytes_per_row,
        "calibraw float pixel readback",
    );
    let receiver = map_read_async(&readback);
    wait_for_mapping(device, submission, receiver, "float pixel", "float pixel")?;

    let mapped = readback.get_mapped_range(..);
    let pixel = match mapped.get(..bytes_per_pixel as usize) {
        Some(pixel) => pixel,
        None => {
            drop(mapped);
            readback.unmap();
            return Err(anyhow!("GPU float pixel readback buffer is truncated"));
        }
    };
    let mut rgb = [0.0; 3];
    for (channel, value) in rgb.iter_mut().enumerate() {
        let offset = channel * (bytes_per_pixel as usize / 4);
        *value = if bytes_per_pixel == 8 {
            let bytes = <[u8; 2]>::try_from(&pixel[offset..offset + 2])
                .map_err(|_| anyhow!("GPU RGBA16F channel has an invalid width"))?;
            half::f16::from_bits(u16::from_le_bytes(bytes)).to_f32()
        } else {
            let bytes = <[u8; 4]>::try_from(&pixel[offset..offset + 4])
                .map_err(|_| anyhow!("GPU RGBA32F channel has an invalid width"))?;
            f32::from_le_bytes(bytes)
        };
    }
    drop(mapped);
    readback.unmap();
    if rgb.iter().any(|value| !value.is_finite()) {
        return Err(anyhow!("float pixel readback contains NaN or infinity"));
    }
    Ok(rgb)
}

/// Returns `(unpadded, padded)` bytes per row for a texture-to-buffer copy of
/// `width` texels, padded to wgpu's 256-byte `COPY_BYTES_PER_ROW_ALIGNMENT`.
/// `format` names the texel format in overflow errors.
fn padded_bytes_per_row(width: u32, bytes_per_pixel: u32, format: &str) -> Result<(u32, u32)> {
    let unpadded_bytes_per_row = width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| anyhow!("GPU {format} row byte count overflows"))?;
    let padded_bytes_per_row = unpadded_bytes_per_row
        .checked_add(255)
        .map(|value| value / 256 * 256)
        .ok_or_else(|| anyhow!("GPU {format} padded row byte count overflows"))?;
    Ok((unpadded_bytes_per_row, padded_bytes_per_row))
}

/// Creates a mappable buffer holding `height` padded rows of `width` texels.
/// Returns the buffer with its `(unpadded, padded)` bytes per row.
fn create_readback_buffer(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
    format: &str,
    label: &'static str,
) -> Result<(wgpu::Buffer, u32, u32)> {
    if width == 0 || height == 0 {
        return Err(anyhow!("GPU {format} readback dimensions must be non-zero"));
    }
    let (unpadded_bytes_per_row, padded_bytes_per_row) =
        padded_bytes_per_row(width, bytes_per_pixel, format)?;
    let size = u64::from(padded_bytes_per_row)
        .checked_mul(u64::from(height))
        .ok_or_else(|| anyhow!("GPU {format} readback buffer size overflows"))?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Ok((buffer, unpadded_bytes_per_row, padded_bytes_per_row))
}

/// Records and submits a copy of the `extent` texels at `origin` (mip 0, layer
/// 0) into `buffer`, one row every `bytes_per_row` bytes.
#[allow(clippy::too_many_arguments)]
fn submit_texture_to_buffer_copy(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    [x, y]: [u32; 2],
    [width, height]: [u32; 2],
    buffer: &wgpu::Buffer,
    bytes_per_row: u32,
    label: &str,
) -> wgpu::SubmissionIndex {
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()))
}

/// Requests a read mapping of the whole `buffer`; the result arrives on the
/// returned channel once the device is polled (see `wait_for_mapping`).
fn map_read_async(
    buffer: &wgpu::Buffer,
) -> std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    receiver
}

fn wait_for_mapping(
    device: &wgpu::Device,
    submission: wgpu::SubmissionIndex,
    receiver: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    operation: &str,
    label: &str,
) -> Result<()> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|error| anyhow!("GPU poll failed during {operation} readback: {error}"))?;
    receiver
        .recv()
        .map_err(|_| anyhow!("GPU {label} readback callback was dropped"))?
        .map_err(|error| anyhow!("GPU {label} readback mapping failed: {error}"))
}

impl RawGpuPipeline {
    pub fn output_snapshot(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> GpuOutputSnapshot {
        let texture = create_processing_texture(
            device,
            texture_size(self.width, self.height),
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            "calibraw output snapshot",
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw output snapshot encoder"),
        });
        encoder.copy_texture_to_texture(
            copy_texture(&self.out_texture),
            copy_texture(&texture),
            texture_size(self.width, self.height),
        );
        queue.submit(Some(encoder.finish()));
        GpuOutputSnapshot {
            texture,
            width: self.width,
            height: self.height,
        }
    }

    pub fn read_output_region_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>> {
        read_rgba8_texture_region_blocking(
            device,
            queue,
            &self.out_texture,
            TextureReadbackRegion {
                origin: [x, y],
                extent: [width, height],
                texture_extent: [self.width, self.height],
                label: "calibraw tiled export readback",
            },
        )
    }

    /// Read the display color before global point-color adjustments.
    pub fn read_point_color_sample_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &GpuParams,
        x: u32,
        y: u32,
    ) -> Result<[f32; 3]> {
        self.read_point_color_sample_with_mode(device, queue, params, x, y, false)
    }

    /// Read the display color at the input to local point-color adjustments.
    pub fn read_local_point_color_sample_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &GpuParams,
        x: u32,
        y: u32,
    ) -> Result<[f32; 3]> {
        self.read_point_color_sample_with_mode(device, queue, params, x, y, true)
    }

    pub(super) fn read_point_color_sample_with_mode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &GpuParams,
        x: u32,
        y: u32,
        after_global: bool,
    ) -> Result<[f32; 3]> {
        anyhow::ensure!(
            x < self.width && y < self.height,
            "point color sample is outside the image"
        );
        let mut sample_params = params.clone();
        sample_params.scene_tone.point_color_meta[2] = if after_global { 2 } else { 1 };
        self.upload_params(queue, &sample_params);
        let render = |label| {
            let mut encoder = device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
            if params.needs_blur_passes() {
                self.encode_bound_pass(&mut encoder, &self.post_blur_render_pass, label);
            } else {
                self.encode_pass(&mut encoder, self.indices.adjustment_render_pass_index);
            }
            queue.submit(Some(encoder.finish()));
        };
        render("calibraw point color sample");
        let result = read_float_texture_pixel_blocking(
            device,
            queue,
            &self.display_linear_texture,
            self.scene_format,
            x,
            y,
        );
        // Restore even on a mapping failure; visualization must never remain in
        // the shared export/display attachment after a sampler operation.
        self.upload_params(queue, params);
        render("calibraw restore point color preview");
        result
    }

    pub fn begin_display_linear_region_readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<PendingRgba32Readback> {
        if self.scene_format != wgpu::TextureFormat::Rgba32Float {
            return Err(anyhow!(
                "display-linear export readback requires ProcessingQuality::High (RGBA32Float)"
            ));
        }
        begin_rgba32_texture_region_rgb_readback(
            device,
            queue,
            &self.display_linear_texture,
            TextureReadbackRegion {
                origin: [x, y],
                extent: [width, height],
                texture_extent: [self.width, self.height],
                label: "calibraw pipelined display-linear export readback",
            },
        )
    }

    pub fn read_display_linear_region_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<f32>> {
        if self.scene_format != wgpu::TextureFormat::Rgba32Float {
            return Err(anyhow!(
                "display-linear export readback requires ProcessingQuality::High (RGBA32Float)"
            ));
        }
        read_rgba32_texture_region_rgb_blocking(
            device,
            queue,
            &self.display_linear_texture,
            TextureReadbackRegion {
                origin: [x, y],
                extent: [width, height],
                texture_extent: [self.width, self.height],
                label: "calibraw display-linear export readback",
            },
        )
    }

    pub fn read_scene_texture_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<f32>> {
        if self.scene_format != wgpu::TextureFormat::Rgba32Float {
            return Err(anyhow!(
                "scene texture readback requires ProcessingQuality::High (RGBA32Float), got {:?}",
                self.scene_format
            ));
        }
        read_rgba32_texture_rgb_blocking(
            device,
            queue,
            &self.scene_texture,
            self.width,
            self.height,
            "calibraw scene texture readback",
        )
    }

    pub fn render_camera_scene_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &GpuParams,
    ) -> Result<Vec<f32>> {
        if self.scene_format != wgpu::TextureFormat::Rgba32Float {
            return Err(anyhow!(
                "camera scene readback requires ProcessingQuality::High (RGBA32Float)"
            ));
        }
        self.upload_params(queue, params);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw camera scene readback encoder"),
        });
        self.encode_raw_stage(&mut encoder, params);
        queue.submit(Some(encoder.finish()));
        self.read_scene_texture_blocking(device, queue)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        rgba32_readback_rows_per_chunk, TextureReadbackRegion, MAX_RGBA32_READBACK_CHUNK_BYTES,
    };

    #[test]
    fn rgba32_readback_chunks_stay_below_the_safe_buffer_budget() {
        let width = 8_256u32;
        let rows = rgba32_readback_rows_per_chunk(width).unwrap();
        let padded = (width * 16).div_ceil(256) * 256;
        assert!(u64::from(rows) * u64::from(padded) <= MAX_RGBA32_READBACK_CHUNK_BYTES);
        assert!(rows > 0);
    }

    #[test]
    fn rgba32_readback_rejects_zero_width() {
        assert!(rgba32_readback_rows_per_chunk(0).is_err());
    }

    #[test]
    fn texture_readback_region_validates_bounds_and_overflow() {
        let valid = TextureReadbackRegion {
            origin: [7, 11],
            extent: [13, 17],
            texture_extent: [20, 28],
            label: "valid",
        };
        assert!(valid.validate("test").is_ok());

        let out_of_bounds = TextureReadbackRegion {
            extent: [14, 17],
            ..valid
        };
        assert!(out_of_bounds.validate("test").is_err());

        let overflowing = TextureReadbackRegion {
            origin: [u32::MAX, 0],
            extent: [2, 1],
            texture_extent: [u32::MAX, 1],
            label: "overflow",
        };
        assert!(overflowing.validate("test").is_err());
    }

    #[test]
    fn texture_readback_region_rejects_empty_extent() {
        let region = TextureReadbackRegion {
            origin: [0, 0],
            extent: [0, 1],
            texture_extent: [1, 1],
            label: "empty",
        };
        assert!(region.validate("test").is_err());
    }
}
