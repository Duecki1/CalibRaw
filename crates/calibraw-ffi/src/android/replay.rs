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
