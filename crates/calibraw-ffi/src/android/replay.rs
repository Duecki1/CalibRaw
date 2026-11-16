use super::*;

/// Owns a codec on the replay worker; dropping it releases unfinished exports too.
pub struct ReplayVideoEncoder {
    app: AndroidApp,
    encoder: Global<JObject<'static>>,
}

impl ReplayVideoEncoder {
    pub fn start(
        app: &AndroidApp,
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, String> {
        let encoder = with_activity(app, |env, activity| {
            let path = env.new_string(path.to_string_lossy())?;
            let encoder = env.call_method(
                activity,
                jni::jni_str!("createReplayEncoder"),
                jni::jni_sig!((JString, i32, i32, i32) -> de.duecki.calibraw.ReplayVideoEncoder),
                &[JValue::Object(&path), JValue::Int(width as i32), JValue::Int(height as i32), JValue::Int(fps as i32)],
            )?.l()?;
            env.new_global_ref(encoder)
        }).map_err(|error| format!("Could not start Android replay encoder: {error:#}"))?;
        Ok(Self {
            app: app.clone(),
            encoder,
        })
    }

    /// The size to encode for a canvas of at most `width` x `height`: the request
    /// rounded down to the first H.264 encoder's alignment that it accepts, or the
    /// request itself when no encoder does.
    pub fn supported_size(
        app: &AndroidApp,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<(u32, u32), String> {
        let packed = with_activity(app, |env, activity| {
            env.call_method(
                activity,
                jni::jni_str!("supportedReplaySize"),
                jni::jni_sig!((i32, i32, i32) -> i64),
                &[
                    JValue::Int(width as i32),
                    JValue::Int(height as i32),
                    JValue::Int(fps as i32),
                ],
            )?
            .j()
        })
        .map_err(|error| format!("Could not query Android replay encoder sizes: {error:#}"))?;
        // Packed by `ReplayVideoEncoder.supportedSize` as (width << 32) | height.
        let (supported_width, supported_height) = ((packed >> 32) as u32, packed as u32);
        let valid = |value: u32, limit: u32| value > 0 && value <= limit && value.is_multiple_of(2);
        if valid(supported_width, width) && valid(supported_height, height) {
            Ok((supported_width, supported_height))
        } else {
            Err(format!(
                "Android returned an invalid replay size {supported_width}x{supported_height}"
            ))
        }
    }

    pub fn write_frame(&mut self, yuv: &[u8]) -> Result<(), String> {
        with_activity(&self.app, |env, _| {
            let bytes = env.byte_array_from_slice(yuv)?;
            env.call_method(
                &self.encoder,
                jni::jni_str!("writeFrame"),
                jni::jni_sig!((byte[]) -> void),
                &[JValue::Object(&bytes)],
            )?;
            Ok(())
        })
        .map_err(|error| format!("Could not encode Android replay frame: {error:#}"))
    }

    pub fn finish(self) -> Result<(), String> {
        with_activity(&self.app, |env, _| {
            env.call_method(
                &self.encoder,
                jni::jni_str!("finish"),
                jni::jni_sig!(() -> void),
                &[],
            )?;
            Ok(())
        })
        .map_err(|error| format!("Could not finish Android replay: {error:#}"))
    }
}

impl Drop for ReplayVideoEncoder {
    fn drop(&mut self) {
        let result = with_activity(&self.app, |env, _| {
            env.call_method(
                &self.encoder,
                jni::jni_str!("close"),
                jni::jni_sig!(() -> void),
                &[],
            )?;
            Ok(())
        });
        if let Err(error) = result {
            log::warn!("Could not release Android replay encoder: {error:#}");
        }
    }
}
