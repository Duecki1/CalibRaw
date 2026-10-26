//! The gestures `android-bench` plays, read from a scenario file.
//!
//! Coordinates are physical pixels of one device in one orientation, so a
//! scenario names the screen it was recorded on and the bench refuses any
//! other.

use crate::{Result, XtaskError};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    /// The device and app state the coordinates were recorded on.
    pub(super) device: String,
    /// The display in its current orientation, `[width, height]`, as
    /// `dumpsys window displays` reports it (`cur=1920x1200`).
    pub(super) screen: [u32; 2],
    /// The photo, relative to `CALIBRAW_BENCH_PHOTOS`.
    pub(super) photo: String,
    pub(super) photo_sha256: String,
    /// The photo's sidecar, relative to the scenario file. It is restored
    /// before every run, so each run starts from the same edit.
    pub(super) sidecar: String,
    pub(super) steps: Vec<Step>,
}

/// One measured interaction.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Step {
    /// Unique; `android-bench-compare` matches steps by name.
    pub(super) name: String,
    /// Measured, from the first action until the last one ends.
    pub(super) actions: Vec<Action>,
    /// Run unmeasured afterwards to put the app back, such as undo.
    #[serde(default)]
    pub(super) then: Vec<Action>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    /// Starts the app with GPU stage timing on. The first action of the
    /// scenario and only that one.
    Launch,
    Tap([u32; 2]),
    /// Two taps in one `adb shell` call, close enough to count as a
    /// double tap.
    DoubleTap([u32; 2]),
    Swipe {
        from: [u32; 2],
        to: [u32; 2],
        ms: u32,
    },
    SleepMs(u64),
    /// Saves the screen as `screens/<step>-<name>.png` in the run folder,
    /// for checking coordinates. Taking it costs the step about a second.
    Screenshot(String),
    /// Waits until the app logs a line containing `text`.
    WaitLog {
        text: String,
        timeout_s: u64,
    },
}

impl Scenario {
    pub(super) fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|error| XtaskError::new(format!("{}: {error}", path.display())))?;
        let scenario: Self = serde_json::from_str(&text)
            .map_err(|error| XtaskError::new(format!("{}: {error}", path.display())))?;
        scenario
            .validate()
            .map_err(|message| XtaskError::new(format!("{}: {message}", path.display())))?;
        Ok(scenario)
    }

    pub(super) fn sidecar_path(&self, scenario_path: &Path) -> PathBuf {
        scenario_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&self.sidecar)
    }

    /// The photo's file name, which is also its name in the app's library.
    pub(super) fn photo_name(&self) -> &str {
        self.photo.rsplit('/').next().unwrap_or(&self.photo)
    }

    fn validate(&self) -> std::result::Result<(), String> {
        if self.photo_name().is_empty()
            || !self
                .photo_name()
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            return Err(format!(
                "photo '{}' must end in a plain file name",
                self.photo
            ));
        }
        let mut names = HashSet::new();
        for step in &self.steps {
            if !is_plain_name(&step.name) || !names.insert(step.name.as_str()) {
                return Err(format!(
                    "step name '{}' is repeated or not a plain file name",
                    step.name
                ));
            }
            if step.actions.is_empty() {
                return Err(format!("step '{}' measures no action", step.name));
            }
        }
        let mut actions = self
            .steps
            .iter()
            .flat_map(|step| step.actions.iter().chain(&step.then));
        if actions.next() != Some(&Action::Launch) {
            return Err("the first action must be \"launch\"".to_owned());
        }
        for action in actions {
            match action {
                Action::Launch => return Err("only the first action may be \"launch\"".to_owned()),
                Action::Tap(point) | Action::DoubleTap(point) => self.check_point(*point)?,
                Action::Swipe { from, to, ms } => {
                    self.check_point(*from)?;
                    self.check_point(*to)?;
                    if *ms == 0 {
                        return Err("a swipe needs a duration".to_owned());
                    }
                }
                Action::SleepMs(_) => {}
                Action::Screenshot(name) => {
                    if !is_plain_name(name) {
                        return Err(format!("screenshot name '{name}' is not a plain file name"));
                    }
                }
                Action::WaitLog { text, timeout_s } => {
                    if text.is_empty() || *timeout_s == 0 {
                        return Err("wait_log needs a text and a timeout".to_owned());
                    }
                }
            }
        }
        Ok(())
    }

    fn check_point(&self, [x, y]: [u32; 2]) -> std::result::Result<(), String> {
        let [width, height] = self.screen;
        if x < width && y < height {
            Ok(())
        } else {
            Err(format!(
                "point {x},{y} is outside the {width}x{height} screen"
            ))
        }
    }
}

/// Letters, digits, `-` and `_`: safe in a file name on every platform.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
}
