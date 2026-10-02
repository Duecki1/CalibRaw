use super::*;
use std::ffi::OsString;
use std::io::Write as _;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

pub(super) struct ReplayFrameWriter {
    child: Option<std::process::Child>,
    stdin: Option<std::process::ChildStdin>,
}

impl ReplayFrameWriter {
    pub(super) fn start(path: &Path, width: u32, height: u32) -> Result<Self, String> {
        let ffmpeg = ffmpeg_program();
        let size = format!("{width}x{height}");
        let fps = REPLAY_FPS.to_string();
        let mut command = Command::new(&ffmpeg);
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgb24")
            .arg("-s")
            .arg(size)
            .arg("-r")
            .arg(&fps)
            .arg("-i")
            .arg("pipe:0")
            .arg("-an")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("medium")
            .arg("-crf")
            .arg("23")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-movflags")
            .arg("+faststart")
            .arg("-f")
            .arg("mp4")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x08000000);

        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not start FFmpeg ({:?}). Install FFmpeg with H.264/libx264 support or set CALIBRAW_FFMPEG: {error}",
                ffmpeg
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "FFmpeg did not provide a video input pipe".to_owned())?;
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
        })
    }

    pub(super) fn write_frame(&mut self, rgb: &[u8]) -> Result<(), String> {
        self.stdin
            .as_mut()
            .ok_or_else(|| "FFmpeg video input pipe is closed".to_owned())?
            .write_all(rgb)
            .map_err(|error| format!("Could not send replay frame to FFmpeg: {error}"))
    }

    pub(super) fn finish(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        let output = self
            .child
            .take()
            .ok_or_else(|| "FFmpeg encoder is closed".to_owned())?
            .wait_with_output()
            .map_err(|error| format!("Could not finish FFmpeg replay encoding: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            Err(format!("FFmpeg exited with {}", output.status))
        } else {
            Err(format!("FFmpeg failed: {detail}"))
        }
    }

    pub(super) fn cancel(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ReplayFrameWriter {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn ffmpeg_program() -> OsString {
    std::env::var_os("CALIBRAW_FFMPEG")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("ffmpeg"))
}

pub(super) fn ensure_ffmpeg_available() -> Result<(), String> {
    let ffmpeg = ffmpeg_program();
    let output = Command::new(&ffmpeg)
        .arg("-hide_banner")
        .arg("-encoders")
        .output()
        .map_err(|error| {
            format!(
                "Could not start FFmpeg ({:?}). Install FFmpeg with H.264/libx264 support or set CALIBRAW_FFMPEG: {error}",
                ffmpeg
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "FFmpeg encoder probe exited with {}",
            output.status
        ));
    }
    let encoders = String::from_utf8_lossy(&output.stdout);
    if !encoders.contains("libx264") {
        return Err(
            "FFmpeg is available but does not provide the libx264 H.264 encoder".to_owned(),
        );
    }
    Ok(())
}
