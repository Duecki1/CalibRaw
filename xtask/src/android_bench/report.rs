//! What one bench run records (`android-bench.json` in its run folder), and
//! the comparison of two runs.

use super::metrics::{FrameStats, MemorySnapshot, StageStats};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BenchReport {
    pub(super) label: String,
    pub(super) calibraw_revision: String,
    pub(super) package: String,
    /// The scenario file, relative to the workspace.
    pub(super) scenario: String,
    pub(super) device: DeviceInfo,
    pub(super) started_unix_seconds: u64,
    pub(super) steps: Vec<StepReport>,
    /// The step during which the app stopped running, if it did; later
    /// steps did not run.
    pub(super) app_died_during: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DeviceInfo {
    pub(super) model: String,
    pub(super) android: String,
    pub(super) total_memory_mib: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StepReport {
    pub(super) name: String,
    /// When the step started, in device seconds since the Unix epoch, as
    /// `logcat.txt` stamps its lines.
    pub(super) started_at: f64,
    /// From the first measured action until the last one ended.
    pub(super) duration_ms: u64,
    pub(super) frames: Option<FrameStats>,
    /// Keyed by stage and size, such as `Output 652x978`.
    pub(super) gpu_stages: BTreeMap<String, StageStats>,
    /// The highest RSS and lowest `MemAvailable` sampled about once a
    /// second during the step.
    pub(super) peak_rss_mib: Option<u64>,
    pub(super) lowest_available_mib: Option<u64>,
    /// After the step, before its unmeasured actions.
    pub(super) memory_after: MemorySnapshot,
    /// The app's "Returned freed memory" diagnostics.
    pub(super) heap_purges: Vec<String>,
    /// Kills by the low-memory killer, of any process.
    pub(super) low_memory_kills: Vec<String>,
}

impl StepReport {
    /// The figures `android-bench-compare` sets side by side, by name.
    fn figures(&self) -> Vec<(String, f64)> {
        let mut figures = Vec::new();
        let mut push = |name: &str, value: Option<f64>| {
            if let Some(value) = value {
                figures.push((name.to_owned(), value));
            }
        };
        let frames = self.frames.as_ref();
        push("frames", frames.map(|frames| frames.total_frames as f64));
        push("fps", frames.map(|frames| frames.average_fps));
        push(
            "present2present p50 ms",
            frames.map(|frames| f64::from(frames.present_to_present_ms.p50)),
        );
        push(
            "present2present p90 ms",
            frames.map(|frames| f64::from(frames.present_to_present_ms.p90)),
        );
        push(
            "present2present max ms",
            frames.map(|frames| f64::from(frames.present_to_present_ms.max)),
        );
        push(
            "post2acquire p50 ms",
            frames.map(|frames| f64::from(frames.post_to_acquire_ms.p50)),
        );
        for (stage, stats) in &self.gpu_stages {
            push(&format!("{stage} count"), Some(stats.count as f64));
            push(&format!("{stage} median ms"), Some(stats.median_ms));
            push(&format!("{stage} per s"), stats.per_second);
        }
        let memory = &self.memory_after;
        let as_f64 = |value: Option<u64>| value.map(|value| value as f64);
        push("peak RSS MiB", as_f64(self.peak_rss_mib));
        push("lowest available MiB", as_f64(self.lowest_available_mib));
        push("RSS after MiB", as_f64(memory.rss_mib));
        push("swap after MiB", as_f64(memory.swap_mib));
        push("available after MiB", as_f64(memory.available_mib));
        push("GPU after MiB", as_f64(memory.gpu_mib));
        push("native heap after MiB", as_f64(memory.native_heap_mib));
        push(
            "native heap allocated after MiB",
            as_f64(memory.native_heap_allocated_mib),
        );
        push("low-memory kills", Some(self.low_memory_kills.len() as f64));
        figures
    }
}

/// The table `android-bench-compare` prints: every figure of every step of
/// `before` beside the same figure of `after`.
pub(super) fn comparison(before: &BenchReport, after: &BenchReport) -> String {
    let mut rows = vec![[
        "step".to_owned(),
        "figure".to_owned(),
        "before".to_owned(),
        "after".to_owned(),
        "change".to_owned(),
    ]];
    for step in &before.steps {
        let Some(after_step) = after.steps.iter().find(|other| other.name == step.name) else {
            rows.push([
                step.name.clone(),
                "(missing after)".to_owned(),
                "".to_owned(),
                "".to_owned(),
                "".to_owned(),
            ]);
            continue;
        };
        let after_figures: BTreeMap<String, f64> = after_step.figures().into_iter().collect();
        for (figure, old) in step.figures() {
            let new = after_figures.get(&figure).copied();
            let change = match new {
                Some(new) if new == old => "=".to_owned(),
                Some(new) if old != 0.0 => format!("{:+.0} %", (new - old) / old * 100.0),
                Some(new) => format!("{}{}", if new > old { "+" } else { "" }, number(new - old)),
                None => String::new(),
            };
            rows.push([
                step.name.clone(),
                figure,
                number(old),
                new.map_or_else(|| "—".to_owned(), number),
                change,
            ]);
        }
    }
    for step in &after.steps {
        if !before.steps.iter().any(|other| other.name == step.name) {
            rows.push([
                step.name.clone(),
                "(new)".to_owned(),
                "".to_owned(),
                "".to_owned(),
                "".to_owned(),
            ]);
        }
    }
    let mut widths = [0; 5];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut table = String::new();
    for row in &rows {
        let line = format!(
            "{:<w0$}  {:<w1$}  {:>w2$}  {:>w3$}  {:>w4$}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
        );
        table.push_str(line.trim_end());
        table.push('\n');
    }
    for (name, report) in [("before", before), ("after", after)] {
        if let Some(step) = &report.app_died_during {
            table.push_str(&format!("{name}: the app died during '{step}'\n"));
        }
    }
    table
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_bench::metrics::Percentiles;

    fn report(steps: Vec<StepReport>) -> BenchReport {
        BenchReport {
            label: "test".to_owned(),
            calibraw_revision: "abc".to_owned(),
            package: "de.duecki.calibraw.profiling".to_owned(),
            scenario: "xtask/android-bench/tab-m11.json".to_owned(),
            device: DeviceInfo {
                model: "TB330FU".to_owned(),
                android: "15".to_owned(),
                total_memory_mib: Some(3700),
            },
            started_unix_seconds: 0,
            steps,
            app_died_during: None,
        }
    }

    fn step(name: &str, fps: f64, output_ms: f64) -> StepReport {
        StepReport {
            name: name.to_owned(),
            started_at: 0.0,
            duration_ms: 3500,
            frames: Some(FrameStats {
                total_frames: 35,
                average_fps: fps,
                present_to_present_ms: Percentiles {
                    p50: 66,
                    p90: 66,
                    max: 90,
                },
                post_to_acquire_ms: Percentiles {
                    p50: 114,
                    p90: 118,
                    max: 126,
                },
            }),
            gpu_stages: BTreeMap::from([(
                "Output 652x978".to_owned(),
                StageStats {
                    count: 30,
                    median_ms: output_ms,
                    max_ms: output_ms,
                    per_second: Some(15.0),
                },
            )]),
            peak_rss_mib: Some(1000),
            lowest_available_mib: Some(800),
            memory_after: MemorySnapshot::default(),
            heap_purges: Vec::new(),
            low_memory_kills: Vec::new(),
        }
    }

    #[test]
    fn the_comparison_matches_steps_and_figures_by_name() {
        let before = report(vec![step("exposure", 16.0, 54.0), step("pan", 48.0, 50.0)]);
        let after = report(vec![step("exposure", 24.0, 54.0), step("zoom", 30.0, 40.0)]);
        let table = comparison(&before, &after);
        let rows: Vec<Vec<&str>> = table
            .lines()
            .map(|line| line.split_whitespace().collect())
            .collect();
        let row = |words: &[&str]| {
            rows.iter()
                .find(|row| row.starts_with(words))
                .map(|row| row[words.len()..].to_vec())
                .unwrap()
        };
        assert_eq!(row(&["exposure", "fps"]), ["16", "24", "+50", "%"]);
        assert_eq!(
            row(&["exposure", "Output", "652x978", "median", "ms"]),
            ["54", "54", "="]
        );
        assert_eq!(row(&["pan"]), ["(missing", "after)"]);
        assert_eq!(row(&["zoom"]), ["(new)"]);
    }

    #[test]
    fn reports_survive_a_json_round_trip() {
        let report = report(vec![step("exposure", 16.0, 54.0)]);
        let json = serde_json::to_string(&report).unwrap();
        let read: BenchReport = serde_json::from_str(&json).unwrap();
        assert_eq!(read.steps[0].figures(), report.steps[0].figures());
    }
}
