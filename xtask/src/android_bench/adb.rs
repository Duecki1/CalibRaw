//! The `adb` calls the bench makes, on the device `adb` picks: the only one
//! connected, or `ANDROID_SERIAL`.

use crate::process::{require_executable, run_checked, run_command_bytes, run_command_output};
use crate::{Result, XtaskError};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Clone)]
pub(super) struct Adb {
    program: PathBuf,
}

impl Adb {
    pub(super) fn find() -> Result<Self> {
        let program = require_executable(
            "adb",
            "adb was not found; add the Android SDK platform-tools to PATH",
        )?;
        let adb = Self { program };
        let state = run_command_output(adb.command().arg("get-state"), "adb get-state")?;
        if state.trim() != "device" {
            return Err(XtaskError::new(format!(
                "the device is not ready: adb get-state printed '{}'",
                state.trim()
            )));
        }
        Ok(adb)
    }

    fn command(&self) -> Command {
        Command::new(&self.program)
    }

    /// Runs `command` in the device's shell and returns what it printed. The
    /// command is shell syntax: callers only interpolate numbers and
    /// validated package names into it.
    pub(super) fn shell(&self, command: &str) -> Result<String> {
        run_command_output(
            self.command().args(["shell", command]),
            &format!("adb shell {command}"),
        )
    }

    pub(super) fn push(&self, local: &Path, remote: &str) -> Result<()> {
        run_command_output(
            self.command().arg("push").arg(local).arg(remote),
            &format!("adb push {}", local.display()),
        )
        .map(drop)
    }

    pub(super) fn install(&self, apk: &Path) -> Result<()> {
        run_checked(
            self.command().args(["install", "-r"]).arg(apk),
            "adb install",
        )
    }

    /// Saves the screen as a PNG.
    pub(super) fn screenshot(&self, path: &Path) -> Result<()> {
        let png = run_command_bytes(
            self.command().args(["exec-out", "screencap", "-p"]),
            "adb exec-out screencap",
        )?;
        fs::write(path, png)?;
        Ok(())
    }

    /// The device's clock, in seconds since the Unix epoch; logcat stamps
    /// its lines with the same clock.
    pub(super) fn device_time(&self) -> Result<f64> {
        let nanoseconds = self.shell("date +%s%N")?;
        nanoseconds
            .trim()
            .parse::<u128>()
            .map(|nanoseconds| nanoseconds as f64 / 1e9)
            .map_err(|_| {
                XtaskError::new(format!(
                    "the device clock printed '{}', not nanoseconds",
                    nanoseconds.trim()
                ))
            })
    }

    /// Streams the device log from `since` (device seconds) into `output`
    /// until the returned child is killed.
    pub(super) fn spawn_logcat(&self, since: f64, output: File) -> Result<Child> {
        self.command()
            .args(["logcat", "-v", "epoch", "-T", &format!("{since:.3}")])
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| XtaskError::new(format!("could not start adb logcat: {error}")))
    }

    /// The process ID of `package`, if it runs.
    pub(super) fn pid_of(&self, package: &str) -> Result<Option<u32>> {
        let output = self.shell(&format!("pidof {package} || true"))?;
        Ok(output
            .split_whitespace()
            .next()
            .and_then(|pid| pid.parse().ok()))
    }
}
