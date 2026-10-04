use anyhow::{anyhow, Context, Result};
use std::borrow::Cow;
use std::sync::Arc;

use super::*;

mod bind_groups;
mod layouts;
mod passes;
mod surfaces;
pub(super) use bind_groups::*;
pub(super) use layouts::*;
pub(super) use passes::*;
pub(super) use surfaces::*;

pub(super) fn tone_guide_axis_cell_count(origin: i32, extent: u32, cell_size: u32) -> u32 {
    let cell_size = cell_size.max(1);
    let phase = origin.rem_euclid(cell_size as i32) as u32;
    phase.saturating_add(extent).div_ceil(cell_size).max(1)
}

#[derive(Default)]
pub(super) struct ShaderSet {
    pub(super) highlight_module: Option<wgpu::ShaderModule>,
    pub(super) bayer_rcd_p1_module: Option<wgpu::ShaderModule>,
    pub(super) bayer_rcd_p2_module: Option<wgpu::ShaderModule>,
    pub(super) bayer_rcd_p3_module: Option<wgpu::ShaderModule>,
    pub(super) bayer_rcd_p4_module: Option<wgpu::ShaderModule>,
    pub(super) dual_demosaic_module: Option<wgpu::ShaderModule>,
    pub(super) xtrans_demosaic_module: Option<wgpu::ShaderModule>,
    pub(super) xtrans_finish_module: Option<wgpu::ShaderModule>,
    pub(super) color_denoise_module: Option<wgpu::ShaderModule>,
    pub(super) tone_analysis_module: Option<wgpu::ShaderModule>,
    pub(super) scene_adjustments_module: Option<wgpu::ShaderModule>,
    pub(super) creative_effects_module: Option<wgpu::ShaderModule>,
    pub(super) view_transform_module: Option<wgpu::ShaderModule>,
}

pub(super) fn load_shader_set(
    device: &wgpu::Device,
    has_program_template: bool,
    cfa_kind: CfaKind,
    demosaic_format: wgpu::TextureFormat,
    work_format: wgpu::TextureFormat,
) -> Result<ShaderSet> {
    if has_program_template {
        return Ok(ShaderSet::default());
    }
    let mut shader_manager =
        ShaderManager::new(work_format, cfa_kind).context("initialize WGSL shader composer")?;
    let mut load_shader = |shader: shaders::EntryShader| {
        if !shader.applies_to(cfa_kind) {
            return Ok(None);
        }
        let module_text = shader.source.module_text();
        let text = if shader.work_format {
            // The demosaic and work formats are the same texture format.
            debug_assert_eq!(demosaic_format, work_format);
            work_shader_source(&module_text, work_format)
                .with_context(|| format!("specialize {} work format", shader.label))?
        } else {
            Cow::Borrowed(module_text.as_ref())
        };
        shader_manager
            .create_shader_module(device, shader.label, text.as_ref(), shader.source.file_name)
            .map(Some)
    };
    Ok(ShaderSet {
        highlight_module: load_shader(shaders::HIGHLIGHTS_ENTRY)?,
        bayer_rcd_p1_module: load_shader(shaders::BAYER_RCD_P1_ENTRY)?,
        bayer_rcd_p2_module: load_shader(shaders::BAYER_RCD_P2_ENTRY)?,
        bayer_rcd_p3_module: load_shader(shaders::BAYER_RCD_P3_ENTRY)?,
        bayer_rcd_p4_module: load_shader(shaders::BAYER_RCD_P4_ENTRY)?,
        dual_demosaic_module: load_shader(shaders::DUAL_DEMOSAIC_ENTRY)?,
        xtrans_demosaic_module: load_shader(shaders::XTRANS_DEMOSAIC_ENTRY)?,
        xtrans_finish_module: load_shader(shaders::XTRANS_FINISH_ENTRY)?,
        color_denoise_module: load_shader(shaders::COLOR_DENOISE_ENTRY)?,
        tone_analysis_module: load_shader(shaders::TONE_ANALYSIS_ENTRY)?,
        scene_adjustments_module: load_shader(shaders::SCENE_ADJUSTMENTS_ENTRY)?,
        creative_effects_module: load_shader(shaders::CREATIVE_EFFECTS_ENTRY)?,
        view_transform_module: load_shader(shaders::VIEW_TRANSFORM_ENTRY)?,
    })
}

#[cfg(test)]
mod tone_grid_tests {
    use super::tone_guide_axis_cell_count;

    #[test]
    fn tone_guide_cell_count_tracks_global_origin_phase() {
        assert_eq!(tone_guide_axis_cell_count(0, 8, 4), 2);
        assert_eq!(tone_guide_axis_cell_count(2, 8, 4), 3);
        assert_eq!(tone_guide_axis_cell_count(-2, 8, 4), 3);
        assert_eq!(tone_guide_axis_cell_count(64, 10, 4), 3);
    }
}
