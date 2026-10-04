//! The background-task notification.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TaskNotificationPayload {
    title: String,
    pub(super) phase: String,
    detail: String,
    progress_percent: i32,
    indeterminate: bool,
    queued_count: i32,
}

pub(super) struct TaskNotificationState {
    activity_key: usize,
    payload: TaskNotificationPayload,
    posted_at: Instant,
}

pub(super) static TASK_NOTIFICATION_STATE: Mutex<Option<TaskNotificationState>> = Mutex::new(None);
const TASK_NOTIFICATION_MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

pub fn update_background_task_notification(
    app: &AndroidApp,
    title: &str,
    phase: &str,
    detail: Option<&str>,
    progress_percent: i32,
    indeterminate: bool,
    queued_count: usize,
) -> Result<(), String> {
    let activity_key = app.activity_as_ptr() as usize;
    let payload = TaskNotificationPayload {
        title: title.to_owned(),
        phase: phase.to_owned(),
        detail: detail.unwrap_or_default().to_owned(),
        progress_percent: progress_percent.clamp(0, 100),
        indeterminate,
        queued_count: i32::try_from(queued_count).unwrap_or(i32::MAX),
    };
    if let Ok(current) = TASK_NOTIFICATION_STATE.lock() {
        if let Some(current) = current
            .as_ref()
            .filter(|current| current.activity_key == activity_key)
        {
            if current.payload == payload {
                return Ok(());
            }
            let same_operation = current.payload.title == payload.title
                && current.payload.phase == payload.phase
                && current.payload.indeterminate == payload.indeterminate
                && current.payload.queued_count == payload.queued_count;
            if same_operation && current.posted_at.elapsed() < TASK_NOTIFICATION_MIN_UPDATE_INTERVAL
            {
                return Ok(());
            }
        }
    }

    with_activity(app, |env, activity| {
        let title = env.new_string(&payload.title)?;
        let phase = env.new_string(&payload.phase)?;
        let detail = env.new_string(&payload.detail)?;
        env.call_method(
            activity,
            jni::jni_str!("updateBackgroundTaskNotification"),
            jni::jni_sig!((JString, JString, JString, i32, i32, i32) -> void),
            &[
                JValue::Object(&title),
                JValue::Object(&phase),
                JValue::Object(&detail),
                JValue::Int(payload.progress_percent),
                JValue::Int(if payload.indeterminate { 1 } else { 0 }),
                JValue::Int(payload.queued_count),
            ],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not update Android task notification: {error:#}"))?;

    if let Ok(mut current) = TASK_NOTIFICATION_STATE.lock() {
        *current = Some(TaskNotificationState {
            activity_key,
            payload,
            posted_at: Instant::now(),
        });
    }
    Ok(())
}

pub fn clear_background_task_notification(app: &AndroidApp) -> Result<(), String> {
    let had_notification = TASK_NOTIFICATION_STATE
        .lock()
        .map(|current| current.is_some())
        .unwrap_or(true);
    if !had_notification {
        return Ok(());
    }
    with_activity(app, |env, activity| {
        env.call_method(
            activity,
            jni::jni_str!("clearBackgroundTaskNotification"),
            jni::jni_sig!(() -> void),
            &[],
        )?;
        Ok(())
    })
    .map_err(|error| format!("could not clear Android task notification: {error:#}"))?;
    if let Ok(mut current) = TASK_NOTIFICATION_STATE.lock() {
        *current = None;
    }
    Ok(())
}
