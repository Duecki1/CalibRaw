//! `cargo xtask android-bench` and `android-bench-compare`: play a gesture
//! scenario on a connected Android device and record what it cost.
//!
//! A run installs the profiling build, restores the scenario's photo and
//! sidecar into that app's library, launches it with GPU stage timing on and
//! plays the steps. Each step's figures come from SurfaceFlinger timestats
//! (enabled for that step only), the app's `GPU stage` diagnostics, memory
//! sampled during the step and read after it, and the low-memory killer's
//! log lines. The report goes to a new run folder under
//! `$CALIBRAW_BASELINE_DIR`, beside the full device log of the run.

mod adb;
mod metrics;
mod report;
mod scenario;

use crate::baseline::{create_run_folder, sha256_hex, unix_seconds};
use crate::process::{display_relative, next_value, require_file, rooted, workspace_root};
use crate::{Result, XtaskError};
use adb::Adb;
use metrics::{frame_stats, memory_command, memory_snapshot, stage_stats, LogLine};
use report::{comparison, BenchReport, DeviceInfo, StepReport};
use scenario::{Action, Scenario, Step};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_PACKAGE: &str = "de.duecki.calibraw.profiling";
const DEFAULT_SCENARIO: &str = "xtask/android-bench/tab-m11.json";
const DEFAULT_APK: &str = "android/app/build/outputs/apk/profiling/app-profiling.apk";
const ACTIVITY: &str = "de.duecki.calibraw.CalibRawActivity";
/// The launch extra `CalibRawActivity.gpuStageTimingRequested` reads.
const GPU_STAGE_TIMING_EXTRA: &str = "de.duecki.calibraw.GPU_STAGE_TIMING";
/// The release app, which a run warns about because it holds memory too.
const RELEASE_PACKAGE: &str = "de.duecki.calibraw";
const REPORT_FILE: &str = "android-bench.json";
const LOG_FILE: &str = "logcat.txt";
/// How often memory is sampled during a step.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

struct RunOptions {
    label: String,
    scenario: PathBuf,
    package: String,
    /// `None` with `--no-install`.
    apk: Option<PathBuf>,
}

impl RunOptions {
    /// `LABEL [--scenario PATH] [--package NAME] [--apk PATH | --no-install]`
    fn parse(args: &[OsString]) -> Result<Self> {
        let mut label = None;
        let mut scenario = PathBuf::from(DEFAULT_SCENARIO);
        let mut package = DEFAULT_PACKAGE.to_owned();
        let mut apk = Some(PathBuf::from(DEFAULT_APK));
        let mut index = 0;
        while index < args.len() {
            match args[index].to_string_lossy().as_ref() {
                "--scenario" => scenario = next_value(args, &mut index, "--scenario")?.into(),
                "--package" => {
                    package = next_value(args, &mut index, "--package")?
                        .to_string_lossy()
                        .into_owned();
                }
                "--apk" => apk = Some(next_value(args, &mut index, "--apk")?.into()),
                "--no-install" => apk = None,
                value if value.starts_with('-') => {
                    return Err(XtaskError::usage(format!(
                        "unknown android-bench option: {value}"
                    )));
                }
                value if label.is_none() => label = Some(value.to_owned()),
                _ => return Err(XtaskError::usage("android-bench accepts one LABEL")),
            }
            index += 1;
        }
        // The package goes into device shell commands.
        if package.is_empty()
            || !package
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._".contains(character))
        {
            return Err(XtaskError::usage(format!(
                "'{package}' is not an Android package name"
            )));
        }
        Ok(Self {
            label: label.ok_or_else(|| XtaskError::usage("android-bench requires a LABEL"))?,
            scenario,
            package,
            apk,
        })
    }
}

/// `cargo xtask android-bench LABEL [--scenario PATH] [--package NAME]
/// [--apk PATH | --no-install]`
pub(crate) fn run(args: Vec<OsString>) -> Result<()> {
    let options = RunOptions::parse(&args)?;
    let root = workspace_root();
    let scenario_path = rooted(&root, &options.scenario);
    let scenario = Scenario::load(&scenario_path)?;
    let photo = bench_photo(&scenario)?;
    let sidecar = scenario.sidecar_path(&scenario_path);
    require_file(&sidecar)?;

    let adb = Adb::find()?;
    check_screen(&adb, &scenario)?;
    if let Some(apk) = &options.apk {
        let apk = rooted(&root, apk);
        require_file(&apk).map_err(|_| {
            XtaskError::new(format!(
                "{} is missing; build it with ./gradlew assembleProfiling, or pass --no-install",
                apk.display()
            ))
        })?;
        adb.install(&apk)?;
    }
    let package = options.package.as_str();
    if adb
        .shell(&format!("pm path {package} || true"))?
        .trim()
        .is_empty()
    {
        return Err(XtaskError::new(format!("{package} is not installed")));
    }
    if package != RELEASE_PACKAGE && adb.pid_of(RELEASE_PACKAGE)?.is_some() {
        eprintln!(
            "warning: {RELEASE_PACKAGE} is running; its memory counts against this run's figures"
        );
    }
    stage_library(&adb, package, &scenario, &photo, &sidecar)?;
    let device = device_info(&adb)?;

    let run_folder = create_run_folder(&root, &options.label)?;
    let mut report = BenchReport {
        label: options.label.clone(),
        calibraw_revision: run_folder.revision.clone(),
        package: package.to_owned(),
        scenario: display_relative(&root, &scenario_path),
        device,
        started_unix_seconds: unix_seconds(),
        steps: Vec::new(),
        app_died_during: None,
    };
    let mut bench = Bench::start(adb, package, &run_folder.path)?;
    let outcome = bench.run_steps(&scenario.steps, &mut report);
    bench.finish();

    let report_path = run_folder.path.join(REPORT_FILE);
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    print_summary(&report);
    println!("Report: {}", report_path.display());
    outcome?;
    match &report.app_died_during {
        Some(step) => Err(XtaskError::new(format!(
            "{package} stopped running during '{step}'"
        ))),
        None => Ok(()),
    }
}

/// `cargo xtask android-bench-compare BEFORE AFTER`, each a run folder or
/// its report.
pub(crate) fn compare(args: Vec<OsString>) -> Result<()> {
    let [before, after] = args.as_slice() else {
        return Err(XtaskError::usage(
            "android-bench-compare expects BEFORE and AFTER run folders",
        ));
    };
    print!(
        "{}",
        comparison(
            &load_report(before.as_ref())?,
            &load_report(after.as_ref())?
        )
    );
    Ok(())
}

fn load_report(path: &Path) -> Result<BenchReport> {
    let path = if path.is_dir() {
        path.join(REPORT_FILE)
    } else {
        path.to_path_buf()
    };
    let text = fs::read_to_string(&path)
        .map_err(|error| XtaskError::new(format!("{}: {error}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|error| XtaskError::new(format!("{}: {error}", path.display())))
}

/// The scenario's photo under `CALIBRAW_BENCH_PHOTOS`, checked against its
/// pinned hash so every run measures the same file.
fn bench_photo(scenario: &Scenario) -> Result<PathBuf> {
    let photos = env::var_os("CALIBRAW_BENCH_PHOTOS")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            XtaskError::usage("set CALIBRAW_BENCH_PHOTOS to the folder holding the Demo photos")
        })?;
    let photo = Path::new(&photos).join(&scenario.photo);
    let bytes = fs::read(&photo)
        .map_err(|error| XtaskError::new(format!("{}: {error}", photo.display())))?;
    if sha256_hex(&bytes) != scenario.photo_sha256 {
        return Err(XtaskError::new(format!(
            "{} is not the photo the scenario pins (SHA-256 {})",
            photo.display(),
            scenario.photo_sha256
        )));
    }
    Ok(photo)
}

/// Refuses a display whose size or orientation differs from the
/// scenario's, where every coordinate would miss.
fn check_screen(adb: &Adb, scenario: &Scenario) -> Result<()> {
    let displays = adb.shell("dumpsys window displays")?;
    let current = displays
        .split_whitespace()
        .find_map(|word| word.strip_prefix("cur="))
        .unwrap_or("unknown");
    let [width, height] = scenario.screen;
    if current == format!("{width}x{height}") {
        Ok(())
    } else {
        Err(XtaskError::new(format!(
            "the display is {current}, but the scenario was recorded at {width}x{height} ({}); \
             rotate the device or record a scenario for it",
            scenario.device
        )))
    }
}

/// Puts the photo and the scenario's sidecar into the app's library, so the
/// first library tile opens them with the same edit every run.
fn stage_library(
    adb: &Adb,
    package: &str,
    scenario: &Scenario,
    photo: &Path,
    sidecar: &Path,
) -> Result<()> {
    // The app must not write the sidecar while it is replaced.
    adb.shell(&format!("am force-stop {package}"))?;
    let library = format!("/sdcard/Android/media/{package}/.library");
    let listing = adb.shell(&format!(
        "if [ -d {library} ]; then ls -a {library}; else echo '<missing>'; fi"
    ))?;
    if listing.trim() == "<missing>" {
        return Err(XtaskError::new(format!(
            "{library} does not exist yet; open {package} once and finish onboarding"
        )));
    }
    let name = scenario.photo_name();
    let companions = format!("{name}.");
    let others: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|entry| {
            !entry.is_empty()
                && !entry.starts_with('.')
                && *entry != name
                && !entry.starts_with(&companions)
        })
        .collect();
    if !others.is_empty() {
        return Err(XtaskError::new(format!(
            "the scenario opens the first library tile, so {library} may only hold {name}; \
             it also holds {}",
            others.join(", ")
        )));
    }
    let remote_photo = format!("{library}/{name}");
    let remote_hash = adb.shell(&format!("sha256sum {remote_photo} 2>/dev/null || true"))?;
    if remote_hash.split_whitespace().next() != Some(scenario.photo_sha256.as_str()) {
        adb.push(photo, &remote_photo)?;
    }
    // The app's sidecar convention: the photo's name plus `.calibraw`.
    adb.push(sidecar, &format!("{remote_photo}.calibraw"))
}

fn device_info(adb: &Adb) -> Result<DeviceInfo> {
    let output = adb.shell(
        "getprop ro.product.model; getprop ro.build.version.release; grep MemTotal /proc/meminfo",
    )?;
    let mut lines = output.lines().map(str::trim);
    Ok(DeviceInfo {
        model: lines.next().unwrap_or_default().to_owned(),
        android: lines.next().unwrap_or_default().to_owned(),
        total_memory_mib: lines.next().and_then(|line| {
            let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kib / 1024)
        }),
    })
}

fn print_summary(report: &BenchReport) {
    for step in &report.steps {
        let mut line = format!("{:<20}", step.name);
        if let Some(frames) = &step.frames {
            line.push_str(&format!(
                " {:>5.1} fps (p90 {} ms)",
                frames.average_fps, frames.present_to_present_ms.p90
            ));
        }
        for (stage, stats) in &step.gpu_stages {
            line.push_str(&format!(
                " | {stage} {:.1} ms ×{}",
                stats.median_ms, stats.count
            ));
        }
        if let Some(rss) = step.memory_after.rss_mib {
            line.push_str(&format!(" | RSS {rss} MiB"));
        }
        if !step.low_memory_kills.is_empty() {
            line.push_str(&format!(" | {} LMK kills", step.low_memory_kills.len()));
        }
        println!("{line}");
    }
}

/// A run in progress: the device log, the memory sampler and the app's
/// process once launched.
struct Bench {
    adb: Adb,
    package: String,
    run_folder: PathBuf,
    log: DeviceLog,
    sampler: MemorySampler,
    pid: Option<u32>,
}

impl Bench {
    fn start(adb: Adb, package: &str, run_folder: &Path) -> Result<Self> {
        Ok(Self {
            log: DeviceLog::start(&adb, &run_folder.join(LOG_FILE))?,
            sampler: MemorySampler::start(adb.clone(), package.to_owned()),
            adb,
            package: package.to_owned(),
            run_folder: run_folder.to_path_buf(),
            pid: None,
        })
    }

    /// Runs `steps` into `report` until one fails or the app dies.
    fn run_steps(&mut self, steps: &[Step], report: &mut BenchReport) -> Result<()> {
        for step in steps {
            let measured = self.measure(step)?;
            report.steps.push(measured);
            if self.pid.is_some() && self.adb.pid_of(&self.package)? != self.pid {
                report.app_died_during = Some(step.name.clone());
                return Ok(());
            }
            if !step.then.is_empty() {
                let start = self.adb.device_time()?;
                for action in &step.then {
                    self.perform(&step.name, action, start)?;
                }
            }
        }
        Ok(())
    }

    fn measure(&mut self, step: &Step) -> Result<StepReport> {
        self.adb
            .shell("dumpsys SurfaceFlinger --timestats -enable -clear")?;
        self.sampler.take_peak();
        let start = self.adb.device_time()?;
        let started = Instant::now();
        for action in &step.actions {
            self.perform(&step.name, action, start)?;
        }
        let end = self.adb.device_time()?;
        let duration_ms = started.elapsed().as_millis() as u64;
        let timestats = self.adb.shell("dumpsys SurfaceFlinger --timestats -dump")?;
        self.adb
            .shell("dumpsys SurfaceFlinger --timestats -disable")?;
        let peak = self.sampler.take_peak();
        let memory_after = memory_snapshot(&self.adb.shell(&memory_command(&self.package, true))?);

        self.log.poll()?;
        let pid = self.pid.unwrap_or(0);
        let lines: Vec<&LogLine> = self.log.between(start, end).collect();
        Ok(StepReport {
            name: step.name.clone(),
            started_at: start,
            duration_ms,
            frames: frame_stats(&timestats, &format!("{}/{ACTIVITY}", self.package)),
            gpu_stages: stage_stats(lines.iter().copied(), pid),
            peak_rss_mib: peak.rss_mib,
            lowest_available_mib: peak.lowest_available_mib,
            memory_after,
            heap_purges: lines
                .iter()
                .filter_map(|line| line.diagnostics(pid))
                .filter(|message| message.starts_with("Returned freed memory"))
                .map(str::to_owned)
                .collect(),
            low_memory_kills: lines
                .iter()
                .filter(|line| line.is_low_memory_kill())
                .map(|line| line.message.clone())
                .collect(),
        })
    }

    /// Performs `action` of the step `step`, which started at `start`
    /// (device seconds; `wait_log` only accepts lines logged since then).
    fn perform(&mut self, step: &str, action: &Action, start: f64) -> Result<()> {
        match action {
            Action::Launch => self.launch(),
            Action::Tap([x, y]) => self.adb.shell(&format!("input tap {x} {y}")).map(drop),
            Action::DoubleTap([x, y]) => self
                .adb
                .shell(&format!("input tap {x} {y}; input tap {x} {y}"))
                .map(drop),
            Action::Swipe {
                from: [x1, y1],
                to: [x2, y2],
                ms,
            } => self
                .adb
                .shell(&format!("input swipe {x1} {y1} {x2} {y2} {ms}"))
                .map(drop),
            Action::SleepMs(milliseconds) => {
                thread::sleep(Duration::from_millis(*milliseconds));
                Ok(())
            }
            Action::Screenshot(name) => {
                let screens = self.run_folder.join("screens");
                fs::create_dir_all(&screens)?;
                self.adb
                    .screenshot(&screens.join(format!("{step}-{name}.png")))
            }
            Action::WaitLog { text, timeout_s } => {
                self.wait_for_log(step, text, Duration::from_secs(*timeout_s), start)
            }
        }
    }

    fn launch(&mut self) -> Result<()> {
        let package = &self.package;
        self.adb.shell("input keyevent KEYCODE_WAKEUP")?;
        let output = self.adb.shell(&format!(
            "am start -W -n {package}/{ACTIVITY} --ez {GPU_STAGE_TIMING_EXTRA} true"
        ))?;
        if output.contains("Error") {
            return Err(XtaskError::new(format!(
                "could not start {package}: {}",
                output.trim()
            )));
        }
        self.pid = self.adb.pid_of(package)?;
        if self.pid.is_none() {
            return Err(XtaskError::new(format!("{package} did not start")));
        }
        Ok(())
    }

    /// Returns once the app logged `text` since `start`, or stopped running
    /// (which the step then reports).
    fn wait_for_log(
        &mut self,
        step: &str,
        text: &str,
        timeout: Duration,
        start: f64,
    ) -> Result<()> {
        let pid = self.pid.unwrap_or(0);
        let deadline = Instant::now() + timeout;
        let mut next_alive_check = Instant::now();
        loop {
            self.log.poll()?;
            if self.log.between(start, f64::INFINITY).any(|line| {
                line.pid == pid && line.tag == "CalibRaw" && line.message.contains(text)
            }) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(XtaskError::new(format!(
                    "step '{step}': the app did not log '{text}' within {} s",
                    timeout.as_secs()
                )));
            }
            if now >= next_alive_check {
                if self.adb.pid_of(&self.package)? != self.pid {
                    return Ok(());
                }
                next_alive_check = now + Duration::from_secs(1);
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    /// Stops the app so it does not hold memory after the run.
    fn finish(&self) {
        if let Err(error) = self.adb.shell(&format!("am force-stop {}", self.package)) {
            eprintln!("warning: could not stop {}: {error}", self.package);
        }
    }
}

/// The device log of the run, streamed by `adb logcat` into a file in the
/// run folder and parsed as it grows.
struct DeviceLog {
    logcat: Child,
    reader: File,
    partial: String,
    lines: Vec<LogLine>,
}

impl DeviceLog {
    fn start(adb: &Adb, path: &Path) -> Result<Self> {
        let since = adb.device_time()?;
        let logcat = adb.spawn_logcat(since, File::create(path)?)?;
        Ok(Self {
            logcat,
            reader: File::open(path)?,
            partial: String::new(),
            lines: Vec::new(),
        })
    }

    /// Reads the lines logcat wrote since the last poll.
    fn poll(&mut self) -> Result<()> {
        let mut bytes = Vec::new();
        self.reader.read_to_end(&mut bytes)?;
        self.partial.push_str(&String::from_utf8_lossy(&bytes));
        if let Some(end) = self.partial.rfind('\n') {
            let complete: String = self.partial.drain(..=end).collect();
            self.lines
                .extend(complete.lines().filter_map(LogLine::parse));
        }
        Ok(())
    }

    /// Lines logged from `start` to `end` (device seconds).
    fn between(&self, start: f64, end: f64) -> impl Iterator<Item = &LogLine> {
        self.lines
            .iter()
            .filter(move |line| line.time >= start && line.time <= end)
    }
}

impl Drop for DeviceLog {
    fn drop(&mut self) {
        let _ = self.logcat.kill();
        let _ = self.logcat.wait();
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MemoryPeak {
    rss_mib: Option<u64>,
    lowest_available_mib: Option<u64>,
}

/// Samples the app's RSS and the device's `MemAvailable` on a thread, so a
/// step's peak is seen while its actions block.
struct MemorySampler {
    stop: Arc<AtomicBool>,
    peak: Arc<Mutex<MemoryPeak>>,
    thread: Option<JoinHandle<()>>,
}

impl MemorySampler {
    fn start(adb: Adb, package: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(Mutex::new(MemoryPeak::default()));
        let thread = {
            let stop = Arc::clone(&stop);
            let peak = Arc::clone(&peak);
            thread::spawn(move || {
                let command = memory_command(&package, false);
                while !stop.load(Ordering::Relaxed) {
                    // A failed sample only loses that sample.
                    if let Ok(output) = adb.shell(&command) {
                        let sample = memory_snapshot(&output);
                        let mut peak = peak.lock().unwrap_or_else(PoisonError::into_inner);
                        peak.rss_mib = peak.rss_mib.max(sample.rss_mib);
                        peak.lowest_available_mib =
                            match (peak.lowest_available_mib, sample.available_mib) {
                                (Some(lowest), Some(available)) => Some(lowest.min(available)),
                                (lowest, available) => lowest.or(available),
                            };
                    }
                    thread::sleep(SAMPLE_INTERVAL);
                }
            })
        };
        Self {
            stop,
            peak,
            thread: Some(thread),
        }
    }

    /// The peak since the last call.
    fn take_peak(&self) -> MemoryPeak {
        std::mem::take(&mut *self.peak.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Drop for MemorySampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RunOptions;
    use std::ffi::OsString;

    fn options(args: &[&str]) -> crate::Result<RunOptions> {
        RunOptions::parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn options_default_to_the_profiling_build_and_the_tablet_scenario() {
        let defaults = options(&["before"]).unwrap();
        assert_eq!(defaults.label, "before");
        assert_eq!(defaults.package, "de.duecki.calibraw.profiling");
        assert!(defaults.apk.is_some());
        let custom = options(&["x", "--no-install", "--package", "de.duecki.calibraw"]).unwrap();
        assert!(custom.apk.is_none());
        assert_eq!(custom.package, "de.duecki.calibraw");
    }

    #[test]
    fn options_refuse_what_the_device_shell_could_misread() {
        assert!(options(&[]).is_err());
        assert!(options(&["a", "b"]).is_err());
        assert!(options(&["a", "--package", "x; reboot"]).is_err());
        assert!(options(&["a", "--frames"]).is_err());
    }
}
