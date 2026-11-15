use anyhow::{Context, Result};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use crate::gpu_errors::GpuErrorScopes;

const CALIBRAW_PIPELINE_CACHE_SCHEMA: &str = "wgpu29-v1";

#[derive(Clone)]
pub struct PersistentGpuPipelineCache {
    cache: wgpu::PipelineCache,
    path: Arc<PathBuf>,
    /// Whether a program compiled with the cache since it was last saved,
    /// shared by every clone.
    unsaved: Arc<AtomicBool>,
}

impl PersistentGpuPipelineCache {
    pub fn load_or_create(
        device: &wgpu::Device,
        adapter_info: &wgpu::AdapterInfo,
        cache_root: &Path,
    ) -> Result<Option<(Arc<Self>, usize)>> {
        if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return Ok(None);
        }
        let Some(adapter_key) = wgpu::util::pipeline_cache_key(adapter_info) else {
            return Ok(None);
        };

        let cache_dir = cache_root.join(format!(
            "wgpu-pipeline-cache-{CALIBRAW_PIPELINE_CACHE_SCHEMA}"
        ));
        let path = cache_dir.join(adapter_key);
        let cache_data = match fs::read(&path) {
            Ok(data) => Some(data),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                log::warn!(
                    "could not read persisted GPU pipeline cache {}: {error}",
                    path.display()
                );
                None
            }
        };
        let loaded_bytes = cache_data.as_ref().map(Vec::len).unwrap_or(0);

        let gpu_error_scopes = GpuErrorScopes::push(device);
        let cache = unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                label: Some("CalibRaw persistent GPU pipeline cache"),
                data: cache_data.as_deref(),
                fallback: true,
            })
        };
        gpu_error_scopes.finish("create persistent GPU pipeline cache")?;

        Ok(Some((
            Arc::new(Self {
                cache,
                path: Arc::new(path),
                unsaved: Arc::new(AtomicBool::new(false)),
            }),
            loaded_bytes,
        )))
    }

    pub fn raw(&self) -> &wgpu::PipelineCache {
        &self.cache
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Notes that a program was compiled with [`Self::raw`]; call it once the
    /// pipeline exists, so a save that sees the note includes it.
    pub(crate) fn note_compiled(&self) {
        self.unsaved.store(true, Ordering::Release);
    }

    /// [`Self::persist`] if a program compiled since the last save; `Ok(0)`
    /// otherwise, as the saved file holds everything already.
    pub fn persist_if_changed(&self) -> Result<usize> {
        if !self.unsaved.swap(false, Ordering::AcqRel) {
            return Ok(0);
        }
        self.persist().inspect_err(|_| self.note_compiled())
    }

    pub fn persist(&self) -> Result<usize> {
        let Some(data) = self.cache.get_data() else {
            return Ok(0);
        };
        calibraw_core::file_ops::write_bytes_atomically(self.path.as_path(), &data).with_context(
            || {
                format!(
                    "could not publish GPU pipeline cache {}",
                    self.path.display()
                )
            },
        )?;
        Ok(data.len())
    }
}
