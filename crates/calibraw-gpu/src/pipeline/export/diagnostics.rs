//! Diagnostic export runs selected explicitly through a file beside the journal.

use super::{ExportTileInputs, GpuParams, LoadedRaw, RawStageStore, TilePlan};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(super) struct ExportProbe {
    directory: PathBuf,
    mode: ProbeMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeMode {
    Raw,
    Stages,
    Synchronous,
    Observe,
}

impl ExportProbe {
    /// `export-probe` contains `raw` (before analysis) or `stages` (after
    /// exact whole-image analysis/maps). Neither mode publishes an image.
    pub(super) fn requested() -> Result<Option<Self>> {
        let Some(root) = calibraw_core::diagnostics::storage_directory() else {
            return Ok(None);
        };
        let marker = root.join("export-probe");
        let mode = match std::fs::read_to_string(&marker) {
            Ok(mode) => mode,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("read export probe configuration"),
        };
        let mode = match mode.trim() {
            "raw" => ProbeMode::Raw,
            "stages" => ProbeMode::Stages,
            "sync" => ProbeMode::Synchronous,
            "observe" => ProbeMode::Observe,
            mode => {
                anyhow::bail!(
                    "unknown export probe mode {mode:?}; expected raw, stages, sync or observe"
                )
            }
        };
        let directory = root.join("export-probe-capture");
        std::fs::create_dir_all(&directory).context("create export probe capture directory")?;
        Ok(Some(Self { directory, mode }))
    }

    pub(super) fn raw_only(&self) -> bool {
        self.mode == ProbeMode::Raw
    }

    pub(super) fn synchronous(&self) -> bool {
        self.mode == ProbeMode::Synchronous
    }

    pub(super) fn captures_stages(&self) -> bool {
        self.mode == ProbeMode::Stages
    }

    pub(super) fn run(
        &self,
        inputs: &ExportTileInputs<'_>,
        plan: &TilePlan,
        scratch: &mut LoadedRaw,
        store: Option<&RawStageStore<'_>>,
    ) -> Result<()> {
        let index = plan.tiles.len() - 1;
        // TilePlan is known non-empty before any export pipeline is built.
        let tile = plan.tiles[index];
        let params: GpuParams = inputs.upload(tile, index, scratch)?;
        std::fs::write(
            self.directory.join("parameters.txt"),
            format!(
                "source={}x{} tile={tile:?}\nformat=unpadded RGB little-endian f32\n",
                plan.full_width, plan.full_height,
            ),
        )?;
        inputs
            .pipeline
            .capture_export_parameters(&params, &self.directory)?;
        calibraw_core::diagnostics::record(format!(
            "Export probe tile {}/{}: {tile:?}, raw_only={}",
            index + 1,
            plan.tile_count(),
            self.raw_only()
        ));
        inputs.load_scene(tile, index, &params, store)?;
        let core = [
            tile.local_core_x,
            tile.local_core_y,
            tile.core_width,
            tile.core_height,
        ];
        if self.raw_only() {
            inputs.pipeline.capture_export_raw(
                inputs.device,
                inputs.queue,
                &self.directory,
                core,
            )?;
        } else {
            inputs.pipeline.capture_export_output(
                inputs.device,
                inputs.queue,
                &params,
                &self.directory,
                core,
            )?;
        }
        calibraw_core::diagnostics::flush();
        anyhow::bail!("Export diagnostic capture completed; remove export-probe to export normally")
    }
}
