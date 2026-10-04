//! Presentation-independent application workflows.
//!
//! Services take plain inputs, run on worker threads and report progress
//! through callbacks. They must not use egui or eframe (`cargo xtask
//! arch-check` enforces this); the `app` module turns their progress into
//! repaints and displayed state.

pub(crate) mod replay;
