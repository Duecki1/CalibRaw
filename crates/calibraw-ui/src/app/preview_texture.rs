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
    /// know the texture is not painted in the current frame (constrained memory).
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
    pub(crate) fn free_now(self, renderer: &mut egui_wgpu::Renderer) {
        let Self { gpu, texture } = self;
        texture.free_now(renderer);
        drop(gpu);
    }
}

/// A copy of a pipeline's output that the UI paints instead of the output
/// itself, refreshed only when a render has completed
/// ([`ShownTexture::show`]). A worker can then render into the pipeline while
/// the canvas keeps the last complete picture, never a half-written one.
pub(crate) struct ShownTexture {
    texture: wgpu::Texture,
    registration: RegisteredTexture,
}

impl ShownTexture {
    /// An empty copy of `size` pixels, registered with the renderer. Call on
    /// the UI thread.
    pub(crate) fn new(
        size: [u32; 2],
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        retirement: &TextureRetirement,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw shown preview"),
            size: wgpu::Extent3d {
                width: size[0].max(1),
                height: size[1].max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The format of `RawGpuPipeline::output_view`.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let registration = RegisteredTexture::register(&view, device, renderer, retirement);
        Self {
            texture,
            registration,
        }
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }

    pub(crate) fn texture(&self) -> egui::TextureId {
        self.registration.id()
    }

    /// Copies `gpu`'s output, which must be this copy's size, once the work
    /// queued so far has run: what the canvas paints from then on.
    pub(crate) fn show(&self, gpu: &RawGpuPipeline, device: &wgpu::Device, queue: &wgpu::Queue) {
        debug_assert_eq!(self.size(), [gpu.width, gpu.height]);
        gpu.copy_output_to(device, queue, &self.texture);
    }

    /// Releases the registration now (see [`RegisteredTexture::free_now`]).
    pub(crate) fn free_now(self, renderer: &mut egui_wgpu::Renderer) {
        let Self {
            texture,
            registration,
        } = self;
        registration.free_now(renderer);
        drop(texture);
    }
}
