//! Opt-in export captures. Ordinary rendering never waits for these readbacks.

use super::{
    begin_rgba32_texture_region_rgb_readback, create_gpu_buffer, map_read_async, wait_for_mapping,
    GpuParams, GpuWait, OutputPart, RawGpuPipeline, TextureReadbackRegion, TONE_STATS_SIZE_BYTES,
};
use anyhow::{Context, Result};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EffectCompileMode {
    #[default]
    Normal,
    Generic,
    Specialized,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CompileProbe {
    pub(super) effects: EffectCompileMode,
    pub(super) serial: bool,
    pub(super) no_cache: bool,
}

/// Read once before compilation. The optional `gpu-compile-probe` file beside
/// the journal accepts `generic` or `specialized`, plus `serial`/`no-cache`.
/// Removing it and restarting restores normal compilation without deleting
/// the application's pipeline cache, settings or photo edits.
pub(super) fn compile_probe() -> CompileProbe {
    static PROBE: OnceLock<CompileProbe> = OnceLock::new();
    *PROBE.get_or_init(|| {
        let text = match std::env::var("CALIBRAW_GPU_COMPILE_PROBE") {
            Ok(text) => text,
            Err(_) => {
                let Some(directory) = calibraw_core::diagnostics::storage_directory() else {
                    return CompileProbe::default();
                };
                let path = directory.join("gpu-compile-probe");
                match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(error) => {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            log::warn!("could not read {}: {error}", path.display());
                        }
                        return CompileProbe::default();
                    }
                }
            }
        };
        let mut probe = CompileProbe::default();
        for word in text.split_whitespace() {
            match word {
                "generic" => probe.effects = EffectCompileMode::Generic,
                "specialized" => probe.effects = EffectCompileMode::Specialized,
                "serial" => probe.serial = true,
                "no-cache" => probe.no_cache = true,
                _ => {
                    log::warn!(
                        "unknown GPU compilation probe option {word:?}; using normal compilation"
                    );
                    return CompileProbe::default();
                }
            }
        }
        calibraw_core::diagnostics::record(format!("GPU compilation probe active: {probe:?}"));
        probe
    })
}

/// Unpadded row-major RGB, little-endian f32; no header. Shared by stage and
/// failure captures so invalid pixels are retained without sanitizing them.
fn write_rgb32f(path: &Path, rgb: &[f32]) -> Result<()> {
    let mut output = BufWriter::new(std::fs::File::create(path)?);
    for value in rgb {
        output.write_all(&value.to_le_bytes())?;
    }
    output.flush().context("write diagnostic pixels")
}

pub(super) fn retain_invalid_readback(rgb: &[f32], width: u32, height: u32, label: &str) {
    let Some(directory) = calibraw_core::diagnostics::storage_directory() else {
        return;
    };
    let result = write_rgb32f(&directory.join("invalid-readback.rgb32f"), rgb).and_then(|()| {
        std::fs::write(directory.join("invalid-readback.txt"), format!(
            "width={width}\nheight={height}\nchannels=RGB\nformat=little-endian f32\nstage={label}\n"
        )).context("write invalid-readback metadata")
    });
    if let Err(error) = result {
        log::warn!("could not retain invalid GPU readback: {error:#}");
    }
}

impl RawGpuPipeline {
    /// Inspect the existing textures after a completed readback fails its
    /// finite check. No stage is rerun: this retains the failure's GPU state.
    pub(crate) fn capture_failed_export(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        core: [u32; 4],
    ) {
        let Some(root) = calibraw_core::diagnostics::storage_directory() else {
            return;
        };
        if crate::gpu_errors::device_was_lost() {
            return;
        }
        let directory = root.join("failed-export-capture");
        if let Err(error) = std::fs::create_dir_all(&directory) {
            log::warn!("could not create failed export capture: {error}");
            return;
        }
        for (stage, texture) in [
            ("raw-scene", &self.scene_texture),
            ("work-1", &self._tex1),
            ("work-2", &self._tex2),
            ("display-linear", &self.display_linear_texture),
        ] {
            if let Err(error) =
                self.capture_export_texture(device, queue, &directory, core, stage, texture)
            {
                log::warn!("could not capture failed export {stage}: {error:#}");
                break;
            }
        }
    }

    /// Save packed parameters without formatting whole image/model buffers.
    pub(crate) fn capture_export_parameters(
        &self,
        params: &GpuParams,
        directory: &Path,
    ) -> Result<()> {
        for (name, bytes) in [
            ("camera.bin", bytemuck::bytes_of(&params.camera)),
            ("scene-tone.bin", params.scene_tone_bytes()),
            ("effects.bin", params.effects_bytes()),
            ("mask-slots.bin", bytemuck::cast_slice(params.mask_slots())),
        ] {
            std::fs::write(directory.join(name), bytes)?;
        }
        std::fs::write(
            directory.join("uniforms.txt"),
            format!(
                "camera={:#?}\nscene_tone={:#?}\neffects={:#?}\nmask_slots={:#?}\n",
                params.camera,
                params.scene_tone,
                params.effects,
                params.mask_slots(),
            ),
        )?;
        Ok(())
    }

    /// Retains the scene after RAW development, including the halo. Summaries
    /// distinguish the requested core from padding that will not be exported.
    pub(crate) fn capture_export_raw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        directory: &Path,
        core: [u32; 4],
    ) -> Result<()> {
        self.capture_export_texture(
            device,
            queue,
            directory,
            core,
            "raw-scene",
            &self.scene_texture,
        )
    }

    /// Diagnostic split of the standard export stages, with the same whole-
    /// image statistics already installed. Pass order and arithmetic remain
    /// unchanged; extra waits deliberately isolate their producing stage.
    pub(crate) fn capture_export_output(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &GpuParams,
        directory: &Path,
        core: [u32; 4],
    ) -> Result<()> {
        self.capture_export_raw(device, queue, directory, core)?;
        let statistics = create_gpu_buffer(
            device,
            "export diagnostic tone statistics",
            TONE_STATS_SIZE_BYTES,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("export diagnostic tone statistics"),
        });
        encoder.copy_buffer_to_buffer(
            &self.tone_stats_buffer,
            0,
            &statistics,
            0,
            TONE_STATS_SIZE_BYTES,
        );
        let submission = queue.submit(Some(encoder.finish()));
        let receiver = map_read_async(&statistics);
        wait_for_mapping(
            device,
            submission,
            receiver,
            "export probe tone statistics",
            "export probe",
            GpuWait::Limited,
        )?;
        {
            let mapped = statistics.get_mapped_range(..);
            std::fs::write(directory.join("tone-statistics.bin"), &mapped)?;
        }
        statistics.unmap();
        self.upload_params(queue, params);
        self.prepare_tiered_effects(queue, params, OutputPart::Scene);
        self.forget_output();
        let mut encoder = self.work(device, queue, "export diagnostic tone guide");
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        encoder.submit(None);

        let mut encoder = self.work(device, queue, "export diagnostic scene preparation");
        self.encode_pass(&mut encoder, self.indices.adjustment_prepare_pass_index);
        encoder.submit(None);
        self.capture_export_texture(
            device,
            queue,
            directory,
            core,
            "prepared-scene",
            &self._tex1,
        )?;

        let mut encoder = self.work(device, queue, "export diagnostic tone response");
        self.encode_pass(&mut encoder, self.indices.adjustment_tone_pass_index);
        encoder.submit(None);
        self.capture_export_texture(device, queue, directory, core, "tone-output", &self._tex2)?;

        // The complete standard path supplies effects and the view transform;
        // it rebuilds the same guide and scene input rather than relying on
        // the diagnostic split's texture/cache state.
        self.dispatch_export_tile_from_scene(queue, device, params);
        self.capture_export_texture(
            device,
            queue,
            directory,
            core,
            "display-linear",
            &self.display_linear_texture,
        )?;
        let [x, y, width, height] = core;
        let rgb = self
            .begin_display_linear_region_readback(device, queue, x, y, width, height)?
            .finish(device, GpuWait::Limited)?;
        write_rgb32f(&directory.join("display-core.rgb32f"), &rgb)?;
        calibraw_core::diagnostics::record(format!(
            "Export probe production core readback: {width}x{height}, all finite"
        ));
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_export_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        directory: &Path,
        core: [u32; 4],
        stage: &str,
        texture: &wgpu::Texture,
    ) -> Result<()> {
        anyhow::ensure!(
            texture.format() == wgpu::TextureFormat::Rgba32Float,
            "export probe requires RGBA32Float"
        );
        let (_, row_bytes) = super::padded_bytes_per_row(self.width, 16, "export diagnostic")?;
        let bytes = u64::from(row_bytes) * u64::from(self.height);
        anyhow::ensure!(
            bytes <= super::MAX_RGBA32_READBACK_CHUNK_BYTES,
            "diagnostic texture exceeds the bounded readback limit"
        );
        let _reservation = super::GpuBudgetReservation::acquire_bytes(
            device,
            "export diagnostic readback",
            bytes,
        )?;
        let rgb = begin_rgba32_texture_region_rgb_readback(
            device,
            queue,
            texture,
            TextureReadbackRegion::full(self.width, self.height, "export diagnostic capture"),
        )?
        .read_rgb(device, "export diagnostic capture", stage, GpuWait::Limited)?;
        write_rgb32f(&directory.join(format!("{stage}.rgb32f")), &rgb)?;

        let [x, y, width, height] = core;
        let mut core_invalid = 0;
        let mut all_invalid = 0;
        let mut first = None;
        let mut minimum = f32::INFINITY;
        let mut maximum = f32::NEG_INFINITY;
        for (index, &value) in rgb.iter().enumerate() {
            let pixel = index / 3;
            let px = pixel as u32 % self.width;
            let py = pixel as u32 / self.width;
            let in_core = px >= x && px < x + width && py >= y && py < y + height;
            if !value.is_finite() {
                all_invalid += 1;
                if in_core {
                    core_invalid += 1;
                    first.get_or_insert((px, py, index % 3, value.to_bits()));
                }
            } else if in_core {
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
        }
        calibraw_core::diagnostics::record(format!(
            "Export probe {stage}: texture={}x{} core={core:?} nonfinite_core={core_invalid} nonfinite_total={all_invalid} first_core={first:?} finite_range=[{minimum:?},{maximum:?}]",
            self.width, self.height,
        ));
        // Compare the production-sized rectangle with the same texels of a
        // full-texture copy, while no rendering changes this texture. This
        // isolates copy pitch/extent and mapping from shader arithmetic.
        let core_rgb = begin_rgba32_texture_region_rgb_readback(
            device,
            queue,
            texture,
            TextureReadbackRegion {
                origin: [x, y],
                extent: [width, height],
                texture_extent: [self.width, self.height],
                label: "export diagnostic core comparison",
            },
        )?
        .read_rgb(
            device,
            "export diagnostic core comparison",
            stage,
            GpuWait::Limited,
        )?;
        write_rgb32f(&directory.join(format!("{stage}-core.rgb32f")), &core_rgb)?;
        let mut differences = 0;
        let mut first_difference = None;
        for (index, &actual) in core_rgb.iter().enumerate() {
            let px = index / 3 % width as usize;
            let py = index / 3 / width as usize;
            let channel = index % 3;
            let source = ((y as usize + py) * self.width as usize + x as usize + px) * 3 + channel;
            let expected = rgb[source];
            if actual.to_bits() != expected.to_bits() {
                differences += 1;
                first_difference.get_or_insert((
                    px,
                    py,
                    channel,
                    expected.to_bits(),
                    actual.to_bits(),
                ));
            }
        }
        calibraw_core::diagnostics::record(format!("Export probe {stage} core-copy comparison: {width}x{height} different_channels={differences} first={first_difference:?}"));
        Ok(())
    }
}
