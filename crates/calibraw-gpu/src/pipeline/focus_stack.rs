//! Focus stacking of RAW frames into a floating-point master.
use super::raw_merge::{ensure_distinct_sources, merge_sources, MergeSource, MergedImage};
use super::*;
use anyhow::{ensure, Context, Result};
use calibraw_core::pipeline::focus_stack::FocusStack;
use std::path::PathBuf;
use std::sync::Arc;

/// Frames are taken in file name order, the focus order of a camera's bracket; the middle
/// one is the reference, which halves the largest focus-breathing difference to align.
/// The callback reports progress and returns false to cancel between frames/tiles.
pub fn stack_focus(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: &[PathBuf],
    program_prewarm: Option<Arc<GpuProgramPrewarm>>,
    mut progress: impl FnMut(String) -> bool,
) -> Result<MergedImage> {
    ensure_distinct_sources(paths, "frame")?;
    let mut frames: Vec<_> = paths
        .iter()
        .map(|path| MergeSource {
            path,
            exposure: 1.0,
            light: 1.0,
        })
        .collect();
    frames.sort_by(|a, b| a.path.cmp(b.path));
    let reference_index = frames.len() / 2;
    frames.swap(0, reference_index);
    let mut stack = None;
    let camera = merge_sources(
        device,
        queue,
        &frames,
        program_prewarm,
        "frame",
        &mut progress,
        |camera, frame| {
            let stack = match &mut stack {
                Some(stack) => stack,
                None => stack.insert(FocusStack::new(camera.width, camera.height)?),
            };
            stack.add(frame.rgb, frame.registration)
        },
    )?;
    ensure!(
        progress("Preparing floating-point master".to_owned()),
        "Merge cancelled"
    );
    let mut rgb = stack.context("Focus stack is empty")?.finish()?;
    camera.to_working_rec2020(&mut rgb);
    Ok(MergedImage {
        width: camera.width,
        height: camera.height,
        rgb,
    })
}
