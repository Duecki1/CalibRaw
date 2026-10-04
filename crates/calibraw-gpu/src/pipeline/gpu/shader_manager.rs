use super::shaders::COMPOSABLE_MODULES;
use super::{work_shader_source, CfaKind};
use anyhow::{anyhow, Context, Result};
use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, ComposerError, NagaModuleDescriptor, ShaderLanguage,
    ShaderType,
};
use std::borrow::Cow;

const SHADER_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders/");

pub(super) struct ShaderManager {
    composer: Composer,
}

impl ShaderManager {
    /// Registers every composable module the pipeline for `cfa_kind` imports,
    /// with work-format markers specialized to `work_format`.
    pub(super) fn new(work_format: wgpu::TextureFormat, cfa_kind: CfaKind) -> Result<Self> {
        let mut manager = Self {
            composer: Composer::default(),
        };
        for module in &COMPOSABLE_MODULES {
            if module.sensor.is_some_and(|sensor| sensor != cfa_kind) {
                continue;
            }
            let text = module.source.module_text();
            let text = if module.work_format {
                Cow::Owned(
                    work_shader_source(&text, work_format)
                        .with_context(|| {
                            format!("specialize composable module {}", module.source.file_name)
                        })?
                        .into_owned(),
                )
            } else {
                text
            };
            manager.register(&module.source.import_path(), module.source.file_name, &text)?;
        }
        Ok(manager)
    }

    fn register(&mut self, import_path: &str, file_name: &str, source: &str) -> Result<()> {
        let file_path = format!("{SHADER_ROOT}{file_name}");
        let result = self
            .composer
            .add_composable_module(ComposableModuleDescriptor {
                source,
                file_path: &file_path,
                language: ShaderLanguage::Wgsl,
                as_name: Some(import_path.to_owned()),
                ..Default::default()
            });
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(self.composer_error("register WGSL module", error)),
        }
    }

    pub(super) fn compose_naga_module(
        &mut self,
        source: &str,
        file_name: &str,
    ) -> Result<wgpu::naga::Module> {
        let file_path = format!("{SHADER_ROOT}{file_name}");
        let result = self.composer.make_naga_module(NagaModuleDescriptor {
            source,
            file_path: &file_path,
            shader_type: ShaderType::Wgsl,
            shader_defs: Default::default(),
            additional_imports: &[],
        });
        match result {
            Ok(module) => Ok(module),
            Err(error) => Err(self.composer_error("compose WGSL entrypoint", error)),
        }
    }

    pub(super) fn create_shader_module(
        &mut self,
        device: &wgpu::Device,
        label: &'static str,
        source: &str,
        file_name: &str,
    ) -> Result<wgpu::ShaderModule> {
        let started = std::time::Instant::now();
        let module = self
            .compose_naga_module(source, file_name)
            .with_context(|| format!("compose {label}"))?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
        });
        log::debug!(
            "GPU shader {label} prepared in {:.3}s",
            started.elapsed().as_secs_f64()
        );
        Ok(module)
    }

    fn composer_error(&self, operation: &str, error: ComposerError) -> anyhow::Error {
        anyhow!(
            "{operation} failed:\n{}",
            error.emit_to_string(&self.composer)
        )
    }
}
