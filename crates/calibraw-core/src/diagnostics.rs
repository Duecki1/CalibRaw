//! The diagnostic report: an in-memory event ring for the running session and
//! a crash-safe copy on disk, so the events that led to a crash, a driver
//! reset or a kill are still there on the next start.
//!
//! # The on-disk copy
//!
//! [`persist_to`] starts one writer thread. [`record`] and [`trace`] only
//! format a line and hand it to that thread through a bounded channel with a
//! non-blocking send, so a caller never waits for the disk and a slow or full
//! disk drops lines instead of stalling a render. The thread appends whole
//! lines, and several queued lines in one `write`, to `session.log`. Nothing
//! is ever rewritten in place:
//!
//! * a line is complete only with its trailing `\n`, so a write cut short by
//!   a crash leaves one partial last line, which [`PreviousSession`] drops;
//! * `session.log` is renamed to `session.1.log` when it passes
//!   [`MAX_FILE_BYTES`], and a new session moves its predecessor's files to
//!   `previous.log` (and, when that one ended abnormally, `crash.log`) by
//!   writing a temporary file and renaming it, which is atomic;
//! * any I/O error ends persistence for the session and nothing else.
//!
//! Lines still queued when the process dies are lost, which is milliseconds
//! of events. [`flush`] waits (briefly) for the thread, and is called where
//! the process is about to end: a GPU device loss, a panic and a clean exit.

use crate::pipeline::{CompactPixelMap, LoadedRaw};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_EVENTS: usize = 300;
/// Lines the writer thread may have queued before new ones are dropped.
const QUEUE_LINES: usize = 4096;
/// Size at which `session.log` is rotated to `session.1.log`, so a session's
/// files hold at most about twice this.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// How long [`flush`] waits for the writer thread.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(400);
/// The last line of a session that ended by itself ([`mark_clean_exit`]).
const CLEAN_EXIT_LINE: &str = "session ended normally";
/// Written when the session's end was already shown to the user, as after a GPU
/// reset, whose restart has a notice of its own.
const REPORTED_LINE: &str = "end of session already reported to the user";
const FINGERPRINT_SAMPLES: usize = 4096;

struct DiagnosticState {
    started: Instant,
    device_info: Option<String>,
    gpu_info: Option<String>,
    events: VecDeque<String>,
}

impl DiagnosticState {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            device_info: None,
            gpu_info: None,
            events: VecDeque::with_capacity(MAX_EVENTS),
        }
    }

    fn push(&mut self, message: &str) {
        let line = self.line(message);
        send_to_writer(Message::Line(line.clone()));
        self.keep(line);
    }

    /// `message` with the time since the session started.
    fn line(&self, message: &str) -> String {
        let elapsed = self.started.elapsed();
        format!(
            "[+{:>5}.{:03}s] {}",
            elapsed.as_secs(),
            elapsed.subsec_millis(),
            message
        )
    }

    fn keep(&mut self, line: String) {
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(line);
    }
}

enum Message {
    Line(String),
    Flush(SyncSender<()>),
}

static WRITER: OnceLock<SyncSender<Message>> = OnceLock::new();
static PREVIOUS: OnceLock<PreviousSession> = OnceLock::new();
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Never blocks: a full queue or a stopped writer drops the line.
fn send_to_writer(message: Message) {
    if let Some(writer) = WRITER.get() {
        match writer.try_send(message) {
            Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
        }
    }
}

fn state() -> &'static Mutex<DiagnosticState> {
    static STATE: OnceLock<Mutex<DiagnosticState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(DiagnosticState::new()))
}

pub fn record(message: impl AsRef<str>) {
    let message = message.as_ref();
    if let Ok(mut state) = state().lock() {
        state.push(message);
    }
    log::info!("diagnostics: {message}");
}

/// Like [`record`], but only for the on-disk copy: for steps too frequent for
/// the 300 events of the report (one per export tile and stage), whose last
/// ones show where a session ended.
pub fn trace(message: impl AsRef<str>) {
    if WRITER.get().is_none() {
        return;
    }
    let message = message.as_ref();
    if let Ok(state) = state().lock() {
        send_to_writer(Message::Line(state.line(message)));
    }
}

/// Device details the platform reports (Android), shown in the diagnostics.
pub fn set_device_info(info: String) {
    if let Ok(mut state) = state().lock() {
        persist_block("device", &info);
        state.device_info = Some(info);
        state.push("Android device information captured");
    }
}

pub fn set_gpu_info(info: String) {
    if let Ok(mut state) = state().lock() {
        persist_block("gpu", &info);
        state.gpu_info = Some(info);
        state.push("GPU adapter information captured");
    }
}

/// Writes `info`, a block of `key=value` lines, as lines `<kind>: …`.
fn persist_block(kind: &str, info: &str) {
    for line in info.trim().lines() {
        send_to_writer(Message::Line(format!("{kind}: {line}")));
    }
}

/// Keeps this session's events in `directory` from now on, and reads what the
/// previous session left there ([`previous_session`]). Call once, early, with
/// a directory the app owns; a second call and any I/O failure do nothing.
///
/// Also installs a panic hook that writes the panic to the file and flushes it
/// before the previous hook runs.
pub fn persist_to(directory: &Path) {
    if WRITER.get().is_some() || fs::create_dir_all(directory).is_err() {
        return;
    }
    let previous = archive_previous_session(directory);
    let _ = PREVIOUS.set(previous);
    let _ = DIRECTORY.set(directory.to_path_buf());
    if let Ok(mut report) = PREVIOUS_REPORT.lock() {
        *report = build_previous_report(directory);
    }

    let (sender, receiver) = mpsc::sync_channel(QUEUE_LINES);
    if WRITER.set(sender).is_err() {
        return;
    }
    let path = directory.join(SESSION_FILE);
    let spawned = std::thread::Builder::new()
        .name("diagnostics-writer".to_owned())
        .spawn(move || write_session(&path, &receiver));
    if spawned.is_err() {
        return;
    }
    // The session's first lines, before anything is queued behind them.
    send_to_writer(Message::Line(format!(
        "CalibRaw {} revision={} target_os={} target_arch={} unix_time={}",
        env!("CARGO_PKG_VERSION"),
        crate::SOURCE_REVISION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )));
    if let Ok(state) = state().lock() {
        if let Some(info) = &state.device_info {
            persist_block("device", info);
        }
        if let Some(info) = &state.gpu_info {
            persist_block("gpu", info);
        }
        for line in &state.events {
            send_to_writer(Message::Line(line.clone()));
        }
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Not through `record`: the panic may have happened while this thread
        // held the report's lock.
        let thread = std::thread::current();
        send_to_writer(Message::Line(format!(
            "PANIC in thread {}: {info}",
            thread.name().unwrap_or("unnamed")
        )));
        flush();
        previous_hook(info);
    }));
}

/// Waits, for a moment, until every line recorded so far is on disk. For where
/// the process is about to end; a no-op without [`persist_to`].
pub fn flush() {
    let Some(writer) = WRITER.get() else {
        return;
    };
    let (done, wait) = mpsc::sync_channel(1);
    if writer.try_send(Message::Flush(done)).is_ok() {
        let _ = wait.recv_timeout(FLUSH_TIMEOUT);
    }
}

/// Marks the end of this session as already shown to the user, so the next start
/// does not tell them again ([`PreviousSession::is_noteworthy`]).
pub fn mark_reported() {
    send_to_writer(Message::Line(REPORTED_LINE.to_owned()));
    flush();
}

/// Marks the session as ended by itself, so the next start does not report it
/// as lost, and waits for it to reach the disk.
pub fn mark_clean_exit() {
    send_to_writer(Message::Line(CLEAN_EXIT_LINE.to_owned()));
    flush();
}

const SESSION_FILE: &str = "session.log";
const ROTATED_FILE: &str = "session.1.log";
const PREVIOUS_FILE: &str = "previous.log";
const CRASH_FILE: &str = "crash.log";

/// The writer thread: appends the queued lines of one `write` at a time.
fn write_session(path: &Path, receiver: &Receiver<Message>) {
    let Some(mut file) = open_append(path) else {
        return;
    };
    let mut size = file.metadata().map_or(0, |metadata| metadata.len());
    let mut batch = String::new();
    while let Ok(first) = receiver.recv() {
        let mut waiting = Vec::new();
        batch.clear();
        let mut message = Some(first);
        while let Some(next) = message.take().or_else(|| receiver.try_recv().ok()) {
            match next {
                Message::Line(line) => {
                    batch.push_str(&line.replace('\n', " "));
                    batch.push('\n');
                }
                Message::Flush(done) => waiting.push(done),
            }
            if batch.len() >= 64 * 1024 {
                break;
            }
        }
        if !batch.is_empty() {
            // One `write` of whole lines: a crash leaves at most a partial last line.
            if file.write_all(batch.as_bytes()).is_err() {
                return;
            }
            size += batch.len() as u64;
            if size >= MAX_FILE_BYTES {
                drop(file);
                let rotated = path.with_file_name(ROTATED_FILE);
                if fs::rename(path, &rotated).is_err() {
                    return;
                }
                let Some(reopened) = open_append(path) else {
                    return;
                };
                file = reopened;
                size = 0;
            }
        }
        for done in waiting {
            let _ = done.try_send(());
        }
    }
}

fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Moves what the last session wrote to `previous.log`, and to `crash.log`
/// too when it did not end normally, and returns it.
fn archive_previous_session(directory: &Path) -> PreviousSession {
    let mut text = String::new();
    for name in [ROTATED_FILE, SESSION_FILE] {
        if let Ok(bytes) = fs::read(directory.join(name)) {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    let previous = PreviousSession::from_log(&text);
    if !text.is_empty() {
        replace_file(&directory.join(PREVIOUS_FILE), previous.text.as_bytes());
        if !previous.ended_normally {
            replace_file(&directory.join(CRASH_FILE), previous.text.as_bytes());
        }
    }
    let _ = fs::remove_file(directory.join(ROTATED_FILE));
    let _ = fs::remove_file(directory.join(SESSION_FILE));
    previous
}

/// Replaces `path` by a file with `contents` through a temporary file and a
/// rename, so it holds the old or the new contents whatever happens.
fn replace_file(path: &Path, contents: &[u8]) {
    let temporary = path.with_extension("tmp");
    let written = File::create(&temporary).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()
    });
    if written.is_err() || fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

/// What an earlier session left on disk, and what the report makes of it.
#[derive(Clone, Debug)]
pub struct PreviousSession {
    /// The complete lines the session wrote.
    pub text: String,
    /// The session ended by itself ([`mark_clean_exit`]).
    pub ended_normally: bool,
}

impl PreviousSession {
    fn from_log(raw: &str) -> Self {
        // A crash can cut the last line short; only lines with their newline count.
        let complete = raw.rfind('\n').map_or("", |end| &raw[..=end]);
        let ended_normally = complete
            .lines()
            .next_back()
            .is_some_and(|line| line.ends_with(CLEAN_EXIT_LINE));
        Self {
            text: complete.to_owned(),
            ended_normally,
        }
    }

    /// Whether the session's end is worth telling the user about: a GPU
    /// device loss, a panic, or an export that never finished. A session
    /// merely closed by the system in the background is not.
    pub fn is_noteworthy(&self) -> bool {
        !self.ended_normally && !self.text.contains(REPORTED_LINE) && self.cause().is_some()
    }

    /// What the log says ended the session, in a sentence, or `None` if it
    /// shows nothing out of the ordinary.
    pub fn cause(&self) -> Option<String> {
        let lines: Vec<&str> = self.text.lines().collect();
        let find = |needle: &str| lines.iter().rposition(|line| line.contains(needle));
        let after = |index: usize| {
            lines[index]
                .split_once("] ")
                .map_or(lines[index], |parts| parts.1)
        };

        let export = self.running_export(&lines);
        let during = export
            .as_ref()
            .map_or_else(String::new, |step| format!(" while exporting ({step})"));
        if let Some(index) = find("PANIC in thread") {
            return Some(format!("CalibRaw panicked{during}: {}", after(index)));
        }
        if let Some(index) = find("GPU device lost: ") {
            let mut cause = format!(
                "The graphics driver reset the GPU{during}: {}",
                after(index)
            );
            if let Some(work) = find("GPU work in flight when the device was lost: ") {
                cause.push_str(&format!(". {}", after(work)));
            }
            return Some(cause);
        }
        export.map(|step| {
            format!(
                "The app ended while exporting ({step}), with no error recorded: the system ended it (often for memory) or it crashed in native code"
            )
        })
    }

    /// The step of an export that had started and neither finished nor failed.
    fn running_export(&self, lines: &[&str]) -> Option<String> {
        let started = lines
            .iter()
            .rposition(|line| line.contains("export worker started"))?;
        let ended = lines.iter().rposition(|line| {
            line.contains("export worker finished") || line.contains("export worker failed")
        });
        if ended.is_some_and(|ended| ended > started) {
            return None;
        }
        let step = lines[started..]
            .iter()
            .rev()
            .find(|line| line.contains("Export step: "))
            .map_or("before the first tile", |line| {
                line.split_once("Export step: ")
                    .map_or(*line, |parts| parts.1)
            });
        Some(step.to_owned())
    }
}

/// The session before this one, if [`persist_to`] found one.
pub fn previous_session() -> Option<&'static PreviousSession> {
    PREVIOUS.get().filter(|previous| !previous.text.is_empty())
}

/// The report of the last session that ended unexpectedly, built once when
/// [`persist_to`] finds it: the snapshot is taken every frame the report is
/// open, which must not read files or scan a log.
static PREVIOUS_REPORT: Mutex<Option<String>> = Mutex::new(None);

/// Lines of a previous session the report shows: its end is what matters.
const REPORT_LINES: usize = 80;
/// Export steps of a previous session the report shows.
const REPORT_STEPS: usize = 6;

/// `crash.log`, the last session that ended abnormally, which survives the
/// ordinary sessions after it.
fn last_abnormal_session(directory: &Path) -> Option<PreviousSession> {
    let bytes = fs::read(directory.join(CRASH_FILE)).ok()?;
    Some(PreviousSession::from_log(&String::from_utf8_lossy(&bytes)))
}

fn build_previous_report(directory: &Path) -> Option<String> {
    let (heading, session) = match previous_session() {
        Some(session) if !session.ended_normally => {
            ("Previous session (ended unexpectedly)", session.clone())
        }
        _ => ("Last unexpected end", last_abnormal_session(directory)?),
    };
    let mut output = String::new();
    let _ = writeln!(output, "\n[{heading}]");
    let cause = session.cause().unwrap_or_else(|| {
        "not recorded; the system may have ended the app in the background, or it crashed in native code"
            .to_owned()
    });
    let _ = writeln!(output, "cause: {cause}");
    // Steps repeat once per tile and stage; only the last ones say where it ended.
    let (steps, events): (Vec<&str>, Vec<&str>) = session
        .text
        .lines()
        .partition(|line| line.contains("Export step: "));
    let skipped = events.len().saturating_sub(REPORT_LINES);
    if skipped > 0 {
        let _ = writeln!(output, "... {skipped} earlier lines not shown");
    }
    for line in &events[skipped..] {
        output.push_str(line);
        output.push('\n');
    }
    if !steps.is_empty() {
        let _ = writeln!(output, "last export steps ({} in all):", steps.len());
        for line in &steps[steps.len().saturating_sub(REPORT_STEPS)..] {
            output.push_str(line);
            output.push('\n');
        }
    }
    Some(output)
}

fn append_previous_report(output: &mut String) {
    if let Ok(report) = PREVIOUS_REPORT.lock() {
        if let Some(report) = report.as_deref() {
            output.push_str(report);
        }
    }
}

pub fn clear() {
    if let Some(directory) = DIRECTORY.get() {
        let _ = fs::remove_file(directory.join(CRASH_FILE));
    }
    if let Ok(mut report) = PREVIOUS_REPORT.lock() {
        *report = None;
    }
    if let Ok(mut state) = state().lock() {
        state.events.clear();
        state.started = Instant::now();
        state.push("Diagnostic event log cleared");
    }
}

pub fn snapshot() -> String {
    let Ok(state) = state().lock() else {
        return "CalibRaw diagnostics are temporarily unavailable because the log is locked."
            .to_owned();
    };

    let mut output = String::with_capacity(12 * 1024);
    let _ = writeln!(output, "CalibRaw {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(output, "revision={}", crate::SOURCE_REVISION);
    let _ = writeln!(
        output,
        "target_os={} target_arch={} pointer_width={}bit",
        std::env::consts::OS,
        std::env::consts::ARCH,
        usize::BITS
    );

    if let Some(device) = &state.device_info {
        output.push_str("\n[Android device]\n");
        output.push_str(device.trim());
        output.push('\n');
    }

    if let Some(gpu) = &state.gpu_info {
        output.push_str("\n[GPU adapter]\n");
        output.push_str(gpu.trim());
        output.push('\n');
    }

    append_previous_report(&mut output);

    output.push_str("\n[Events]\n");
    if state.events.is_empty() {
        output.push_str("No diagnostic events yet. Open a RAW or start an export.\n");
    } else {
        for event in &state.events {
            output.push_str(event);
            output.push('\n');
        }
    }
    output
}

pub fn record_raw(label: &str, raw: &LoadedRaw) {
    let raw_signature = sampled_u16_fingerprint(&raw.raw_pixels);
    let cfa_signature = sampled_u8_fingerprint(&raw.color_indices);
    let black_signature = sampled_f32_fingerprint(&raw.black_levels_per_pixel);
    let cfa_sample_counts = sampled_cfa_counts(&raw.color_indices);

    record(format!(
        "{label}: camera=\"{} {}\" size={}x{} cfa={:?} layout={:?} raw_len={} cfa_len={} black_map_len={}",
        raw.camera_make,
        raw.camera_model,
        raw.width,
        raw.height,
        raw.cfa_kind,
        raw.pixel_layout(),
        raw.raw_pixels.len(),
        raw.color_indices.len(),
        raw.black_levels_per_pixel.len(),
    ));
    record(format!(
        "{label}: wb={:?} black={:?} white={:?}",
        raw.wb_coeffs, raw.black_levels, raw.white_levels
    ));
    record(format!(
        "{label}: cam_to_srgb=[{:?}, {:?}, {:?}]",
        raw.profile_color.cam_to_srgb[0],
        raw.profile_color.cam_to_srgb[1],
        raw.profile_color.cam_to_srgb[2]
    ));
    record(format!(
        "{label}: sampled_fingerprint raw={raw_signature:016x} cfa={cfa_signature:016x} black={black_signature:016x} sampled_cfa_counts={cfa_sample_counts:?}"
    ));
}

fn sample_indices(length: usize) -> impl Iterator<Item = usize> {
    let count = length.min(FINGERPRINT_SAMPLES);
    (0..count).map(move |sample| {
        if count <= 1 {
            0
        } else {
            sample.saturating_mul(length.saturating_sub(1)) / (count - 1)
        }
    })
}

fn sampled_fingerprint<T, Get, Bytes, ByteIter>(length: usize, get: Get, bytes: Bytes) -> u64
where
    Get: Fn(usize) -> T,
    Bytes: Fn(T) -> ByteIter,
    ByteIter: IntoIterator<Item = u8>,
{
    let mut hash = crate::fnv::OFFSET_BASIS;
    for index in sample_indices(length) {
        for byte in bytes(get(index)) {
            hash = crate::fnv::step(hash, byte);
        }
    }
    hash
}

fn sampled_u16_fingerprint(values: &[u16]) -> u64 {
    sampled_fingerprint(values.len(), |index| values[index], u16::to_le_bytes)
}

fn sampled_u8_fingerprint(values: &CompactPixelMap<u8>) -> u64 {
    sampled_fingerprint(values.len(), |index| values[index], std::iter::once)
}

fn sampled_f32_fingerprint(values: &CompactPixelMap<f32>) -> u64 {
    sampled_fingerprint(
        values.len(),
        |index| values[index].to_bits(),
        u32::to_le_bytes,
    )
}

fn sampled_cfa_counts(values: &CompactPixelMap<u8>) -> [usize; 4] {
    let mut counts = [0usize; 4];
    for index in sample_indices(values.len()) {
        if let Some(count) = counts.get_mut(usize::from(values[index])) {
            *count += 1;
        }
    }
    counts
}
