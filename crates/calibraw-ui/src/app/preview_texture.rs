//! Presentation of GPU processing output as egui textures.
//!
//! `calibraw-gpu` renders into plain wgpu textures and knows nothing about
//! egui. This module registers a pipeline's output with the egui renderer and
//! owns that registration: dropping it releases the texture at the start of
//! the next frame, after the current frame can no longer paint it.

use crate::pipeline::RawGpuPipeline;
use eframe::{egui, egui_wgpu, wgpu};
use std::sync::{Arc, Mutex, PoisonError};

/// Texture registrations waiting to be released by the egui renderer.
///
/// Releasing a texture while the current frame may still paint it is unsafe,
/// so dropped registrations queue their ID here and the UI thread frees them
/// at the start of the next frame ([`TextureRetirement::release`]).
#[derive(Clone)]
pub(crate) struct TextureRetirement {
    retired: Arc<Mutex<Vec<egui::TextureId>>>,
    repaint: egui::Context,
}

impl TextureRetirement {
    pub(crate) fn new(repaint: egui::Context) -> Self {
        Self {
            retired: Arc::default(),
            repaint,
        }
    }

    fn retire(&self, texture: egui::TextureId) {
        let mut retired = self.retired.lock().unwrap_or_else(PoisonError::into_inner);
        if !retired.contains(&texture) {
            retired.push(texture);
        }
        drop(retired);
        // A frame must run to release the texture even if nothing else changes.
        self.repaint.request_repaint();
    }

    /// Frees every retired registration. Call on the UI thread before painting.
    pub(crate) fn release(&self, renderer: &mut egui_wgpu::Renderer) {
        let retired =
            std::mem::take(&mut *self.retired.lock().unwrap_or_else(PoisonError::into_inner));
        for texture in retired {
            renderer.free_texture(&texture);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.retired
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
}

/// A native texture registered with the egui renderer. Dropping it retires
/// the registration; [`RegisteredTexture::free_now`] releases it immediately.
pub(crate) struct RegisteredTexture {
    id: Option<egui::TextureId>,
    retirement: TextureRetirement,
}

impl RegisteredTexture {
    pub(crate) fn register(
        view: &wgpu::TextureView,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        retirement: &TextureRetirement,
    ) -> Self {
        let id = renderer.register_native_texture(device, view, wgpu::FilterMode::Linear);
        Self {
            id: Some(id),
            retirement: retirement.clone(),
        }
    }

    pub(crate) fn id(&self) -> egui::TextureId {
        self.id
            .expect("a registered texture keeps its ID until it is released")
    }

    /// Releases the registration now instead of at the next frame. Only for
    /// callers that must return GPU memory before allocating a replacement and
    /// know the texture is not painted in the current frame (Android).
    #[cfg(target_os = "android")]
    pub(crate) fn free_now(mut self, renderer: &mut egui_wgpu::Renderer) {
        if let Some(id) = self.id.take() {
            renderer.free_texture(&id);
        }
    }
}

impl Drop for RegisteredTexture {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.retirement.retire(id);
        }
    }
}

/// A processing pipeline whose output texture is shown in the UI.
pub(crate) struct PreviewPipeline {
    gpu: RawGpuPipeline,
    texture: RegisteredTexture,
}

impl PreviewPipeline {
    /// Registers `gpu`'s output with the renderer. Call on the UI thread.
    pub(crate) fn register(
        gpu: RawGpuPipeline,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        retirement: &TextureRetirement,
    ) -> Self {
        let texture = RegisteredTexture::register(gpu.output_view(), device, renderer, retirement);
        Self { gpu, texture }
    }

    pub(crate) fn gpu(&self) -> &RawGpuPipeline {
        &self.gpu
    }

    pub(crate) fn texture(&self) -> egui::TextureId {
        self.texture.id()
    }

    /// Stops presenting the pipeline; its texture is retired.
    pub(crate) fn into_gpu(self) -> RawGpuPipeline {
        let Self { gpu, texture } = self;
        drop(texture);
        gpu
    }

    /// Drops the pipeline and releases its texture immediately (see
    /// [`RegisteredTexture::free_now`]).
    #[cfg(target_os = "android")]
    pub(crate) fn free_now(self, renderer: &mut egui_wgpu::Renderer) {
        let Self { gpu, texture } = self;
        texture.free_now(renderer);
        drop(gpu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_deduplicates_and_requests_a_frame() {
        let context = egui::Context::default();
        let retirement = TextureRetirement::new(context);
        let texture = egui::TextureId::User(7);
        retirement.retire(texture);
        retirement.retire(texture);
        assert_eq!(
            *retirement.retired.lock().unwrap(),
            vec![egui::TextureId::User(7)]
        );
        assert!(!retirement.is_empty());
    }
}
