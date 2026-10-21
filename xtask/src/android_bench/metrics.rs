//! Reading the device's reports: SurfaceFlinger timestats, `/proc` and
//! `dumpsys` memory figures, and logcat lines.
//!
//! Each parser takes the text a device printed and keeps only what the
//! report uses; anything it cannot read becomes `None`, so a format change
//! on another Android version loses a figure instead of the run.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Presented frames of the app's window during a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct FrameStats {
    pub(super) total_frames: u64,
    pub(super) average_fps: f64,
    /// Time between presented frames.
    pub(super) present_to_present_ms: Percentiles,
    /// Time from queueing a buffer until its GPU work finished.
    pub(super) post_to_acquire_ms: Percentiles,
}

/// Timestats histogram buckets, named by their lower bound in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Percentiles {
    pub(super) p50: u32,
    pub(super) p90: u32,
    pub(super) max: u32,
}

/// Reads the layer of `dump` (`dumpsys SurfaceFlinger --timestats -dump`)
/// whose name starts with `layer_prefix`. A relaunched activity gets a new
/// layer, so the one with the most frames is taken.
pub(super) fn frame_stats(dump: &str, layer_prefix: &str) -> Option<FrameStats> {
    let lines: Vec<&str> = dump.lines().collect();
    let mut best: Option<FrameStats> = None;
    let mut index = 0;
    while index < lines.len() {
        let Some(layer) = lines[index].strip_prefix("layerName = ") else {
            index += 1;
            continue;
        };
        let end = lines[index + 1..]
            .iter()
            .position(|line| line.starts_with("layerName = "))
            .map_or(lines.len(), |offset| index + 1 + offset);
        if layer.starts_with(layer_prefix) {
            if let Some(stats) = layer_stats(&lines[index + 1..end]) {
                if best
                    .as_ref()
                    .is_none_or(|best| stats.total_frames > best.total_frames)
                {
                    best = Some(stats);
                }
            }
        }
        index = end;
    }
    best
}

fn layer_stats(block: &[&str]) -> Option<FrameStats> {
    let field = |name: &str| {
        block
            .iter()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(" = "))
    };
    let histogram = |name: &str| -> Option<Percentiles> {
        let header = format!("{name} histogram");
        let position = block.iter().position(|line| line.starts_with(&header))?;
        percentiles(&histogram_buckets(block.get(position + 1)?))
    };
    Some(FrameStats {
        total_frames: field("totalFrames")?.trim().parse().ok()?,
        average_fps: field("averageFPS")?.trim().parse().ok()?,
        present_to_present_ms: histogram("present2present")?,
        post_to_acquire_ms: histogram("post2acquire")?,
    })
}

/// `0ms=0 1ms=3 ...` as `(bucket, count)` pairs.
fn histogram_buckets(line: &str) -> Vec<(u32, u64)> {
    line.split_whitespace()
        .filter_map(|pair| {
            let (bucket, count) = pair.split_once('=')?;
            Some((
                bucket.strip_suffix("ms")?.parse().ok()?,
                count.parse().ok()?,
            ))
        })
        .collect()
}

/// The buckets holding the median, the 90th percentile and the largest
/// sample; `None` for an empty histogram.
fn percentiles(buckets: &[(u32, u64)]) -> Option<Percentiles> {
    let total: u64 = buckets.iter().map(|(_, count)| count).sum();
    if total == 0 {
        return None;
    }
    let at = |fraction: f64| {
        let rank = (fraction * total as f64).ceil().max(1.0) as u64;
        let mut seen = 0;
        buckets
            .iter()
            .find(|(_, count)| {
                seen += count;
                seen >= rank
            })
            .map_or(0, |(bucket, _)| *bucket)
    };
    Some(Percentiles {
        p50: at(0.5),
        p90: at(0.9),
        max: at(1.0),
    })
}

/// Memory of the app and the device at one moment, in MiB.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct MemorySnapshot {
    pub(super) rss_mib: Option<u64>,
    pub(super) swap_mib: Option<u64>,
    /// The device's `MemAvailable`.
    pub(super) available_mib: Option<u64>,
    /// The app's GPU memory (`dumpsys gpu --gpumem`).
    pub(super) gpu_mib: Option<u64>,
    /// Native heap mapped and allocated (`dumpsys meminfo`): the difference
    /// is freed memory the process still holds.
    pub(super) native_heap_mib: Option<u64>,
    pub(super) native_heap_allocated_mib: Option<u64>,
    pub(super) total_pss_mib: Option<u64>,
}

/// The device command that prints what [`memory_snapshot`] reads, for the
/// running process of `package`. `meminfo` adds `dumpsys meminfo`, which
/// takes about a second.
pub(super) fn memory_command(package: &str, meminfo: bool) -> String {
    let mut command = format!(
        "grep MemAvailable /proc/meminfo; pid=$(pidof {package}); \
         if [ -n \"$pid\" ]; then grep -E '^Vm(RSS|Swap):' /proc/$pid/status"
    );
    if meminfo {
        command.push_str("; dumpsys gpu --gpumem | grep \"Proc $pid total\"; dumpsys meminfo $pid");
    }
    command.push_str("; fi");
    command
}

pub(super) fn memory_snapshot(output: &str) -> MemorySnapshot {
    let mut snapshot = MemorySnapshot::default();
    for line in output.lines() {
        let trimmed = line.trim();
        let kib = |rest: &str| rest.split_whitespace().next()?.parse::<u64>().ok();
        if let Some(rest) = trimmed.strip_prefix("VmRSS:") {
            snapshot.rss_mib = kib(rest).map(mib);
        } else if let Some(rest) = trimmed.strip_prefix("VmSwap:") {
            snapshot.swap_mib = kib(rest).map(mib);
        } else if let Some(rest) = trimmed.strip_prefix("MemAvailable:") {
            snapshot.available_mib = kib(rest).map(mib);
        } else if let Some(rest) = trimmed.strip_prefix("TOTAL PSS:") {
            snapshot.total_pss_mib = kib(rest).map(mib);
        } else if trimmed.starts_with("Proc ") {
            // `Proc 21490 total: 56979456`, in bytes.
            snapshot.gpu_mib = trimmed
                .rsplit_once(':')
                .and_then(|(_, bytes)| bytes.trim().parse::<u64>().ok())
                .map(|bytes| bytes / (1024 * 1024));
        } else if let Some(rest) = trimmed.strip_prefix("Native Heap ") {
            // The meminfo table row; its last columns are Heap Size, Alloc
            // and Free. The App Summary row reads "Native Heap:" instead.
            let numbers: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            if let [.., size, allocated, _free] = numbers[..] {
                snapshot.native_heap_mib = Some(mib(size));
                snapshot.native_heap_allocated_mib = Some(mib(allocated));
            }
        }
    }
    snapshot
}

fn mib(kib: u64) -> u64 {
    kib / 1024
}

/// One line of `adb logcat -v epoch`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LogLine {
    /// Seconds since the Unix epoch, on the device's clock.
    pub(super) time: f64,
    pub(super) pid: u32,
    pub(super) tag: String,
    pub(super) message: String,
}

impl LogLine {
    /// `1729462538.377  1244  1327 I Tag: message`.
    pub(super) fn parse(line: &str) -> Option<Self> {
        let mut rest = line.trim_start();
        let mut field = || {
            let (value, tail) = rest.split_once(char::is_whitespace)?;
            rest = tail.trim_start();
            Some(value)
        };
        let time = field()?.parse().ok()?;
        let pid = field()?.parse().ok()?;
        let _thread = field()?;
        let _level = field()?;
        let (tag, message) = rest.split_once(": ")?;
        Some(Self {
            time,
            pid,
            tag: tag.trim_end().to_owned(),
            message: message.to_owned(),
        })
    }

    /// The app's `calibraw_core::diagnostics::record` lines, without the
    /// module path the Android logger puts first and the `diagnostics: `
    /// prefix the recorder adds.
    pub(super) fn diagnostics(&self, pid: u32) -> Option<&str> {
        if self.pid != pid || self.tag != "CalibRaw" {
            return None;
        }
        self.message
            .strip_prefix("calibraw_core::diagnostics: ")?
            .strip_prefix("diagnostics: ")
    }

    /// A kill by the low-memory killer, of any process.
    pub(super) fn is_low_memory_kill(&self) -> bool {
        self.tag == "lowmemorykiller" && self.message.starts_with("Kill '")
    }
}

/// GPU time of one processing stage over a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct StageStats {
    pub(super) count: usize,
    pub(super) median_ms: f64,
    pub(super) max_ms: f64,
    /// How often the stage ran while it was running, from its first to its
    /// last line; `None` below two runs.
    pub(super) per_second: Option<f64>,
}

/// `GPU stage Output 652x978: 54.21 ms in 3 passes (...)` as the stage
/// name and its milliseconds (`calibraw_gpu::stage_timing`).
fn stage_time(diagnostics: &str) -> Option<(&str, f64)> {
    let rest = diagnostics.strip_prefix("GPU stage ")?;
    let (stage, rest) = rest.split_once(": ")?;
    let milliseconds = rest.split_once(" ms")?.0.parse().ok()?;
    Some((stage, milliseconds))
}

/// Stage statistics from the app's (`pid`) log lines.
pub(super) fn stage_stats<'a>(
    lines: impl IntoIterator<Item = &'a LogLine>,
    pid: u32,
) -> BTreeMap<String, StageStats> {
    let mut stages: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for line in lines {
        if let Some((stage, milliseconds)) = line.diagnostics(pid).and_then(stage_time) {
            stages
                .entry(stage.to_owned())
                .or_default()
                .push((line.time, milliseconds));
        }
    }
    stages
        .into_iter()
        .map(|(stage, runs)| {
            let mut times: Vec<f64> = runs.iter().map(|(_, milliseconds)| *milliseconds).collect();
            times.sort_by(f64::total_cmp);
            let span = runs.last().map_or(0.0, |last| last.0) - runs[0].0;
            let stats = StageStats {
                count: runs.len(),
                median_ms: times[times.len() / 2],
                max_ms: times[times.len() - 1],
                per_second: (runs.len() > 1 && span > 0.0).then(|| (runs.len() - 1) as f64 / span),
            };
            (stage, stats)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 s exposure drag on the Tab M11 (v1.3.1), shortened.
    const TIMESTATS: &str = "\
layerName = com.android.systemui/StatusBar#12
totalFrames = 400
averageFPS = 90.000
present2present histogram is as below:
11ms=400
post2acquire histogram is as below:
1ms=400
layerName = de.duecki.calibraw/de.duecki.calibraw.CalibRawActivity#5367
packageName =
totalFrames = 35
droppedFrames = 0
averageFPS = 16.432
present2present histogram is as below:
0ms=0 1ms=0 22ms=3 23ms=0 58ms=9 62ms=0 66ms=22 70ms=0 90ms=1 94ms=0 1000ms=0
latch2present histogram is as below:
0ms=0 26ms=1 38ms=3 106ms=1 118ms=10 126ms=9 130ms=10 150ms=1
post2acquire histogram is as below:
0ms=0 102ms=4 106ms=0 114ms=20 118ms=10 122ms=0 126ms=1
";

    #[test]
    fn frame_stats_read_the_app_layer() {
        let stats = frame_stats(
            TIMESTATS,
            "de.duecki.calibraw/de.duecki.calibraw.CalibRawActivity",
        )
        .unwrap();
        assert_eq!(stats.total_frames, 35);
        assert_eq!(stats.average_fps, 16.432);
        assert_eq!(
            stats.present_to_present_ms,
            Percentiles {
                p50: 66,
                p90: 66,
                max: 90
            }
        );
        assert_eq!(
            stats.post_to_acquire_ms,
            Percentiles {
                p50: 114,
                p90: 118,
                max: 126
            }
        );
        // The release app's layer is not the profiling app's.
        assert!(frame_stats(
            TIMESTATS,
            "de.duecki.calibraw.profiling/de.duecki.calibraw.CalibRawActivity"
        )
        .is_none());
    }

    #[test]
    fn percentiles_pick_the_bucket_holding_the_rank() {
        let buckets = [(10, 5), (20, 4), (30, 1)];
        assert_eq!(
            percentiles(&buckets),
            Some(Percentiles {
                p50: 10,
                p90: 20,
                max: 30
            })
        );
        assert_eq!(percentiles(&[(10, 0)]), None);
    }

    #[test]
    fn memory_snapshot_reads_proc_gpumem_and_meminfo() {
        let output = "\
MemAvailable:     893440 kB
VmRSS:\t 1032340 kB
VmSwap:\t   14369 kB
Proc 24588 total: 371851264
Applications Memory Usage (in Kilobytes):
** MEMINFO in pid 24588 [de.duecki.calibraw] **
                   Pss  Private  Private  SwapPss      Rss     Heap     Heap     Heap
                 Total    Dirty    Clean    Dirty    Total     Size    Alloc     Free
                ------   ------   ------   ------   ------   ------   ------   ------
  Native Heap   585615   585568       12    11720   586504   619768   215529   404238
  Dalvik Heap      568      556        0      181     1012    26580     2004    24576
    GL mtrack   354576   354576        0        0   354576
 App Summary
         Native Heap:   585568                         586504
           TOTAL PSS:   982157            TOTAL RSS:  1032340       TOTAL SWAP PSS:    14369
";
        assert_eq!(
            memory_snapshot(output),
            MemorySnapshot {
                rss_mib: Some(1008),
                swap_mib: Some(14),
                available_mib: Some(872),
                gpu_mib: Some(354),
                native_heap_mib: Some(605),
                native_heap_allocated_mib: Some(210),
                total_pss_mib: Some(959),
            }
        );
        // Without the app only the device figure is there.
        assert_eq!(
            memory_snapshot("MemAvailable:     893440 kB\n"),
            MemorySnapshot {
                available_mib: Some(872),
                ..MemorySnapshot::default()
            }
        );
    }

    #[test]
    fn log_lines_split_into_their_fields() {
        let line = LogLine::parse(
            "1792586698.721   808   808 I lowmemorykiller: Kill 'de.duecki.calibraw' (8126), \
             uid 10210, oom_score_adj 0 to free 1249968kB rss, 2044096kB swap; reason: device is not responding",
        )
        .unwrap();
        assert_eq!((line.time, line.pid), (1792586698.721, 808));
        assert!(line.is_low_memory_kill());
        let zygote =
            LogLine::parse("  1792586700.438   758   758 I Zygote  : Process 8126 exited").unwrap();
        assert_eq!(zygote.tag, "Zygote");
        assert_eq!(zygote.message, "Process 8126 exited");
        let stage = LogLine::parse(
            "         1792589101.094 23271 23303 I CalibRaw: calibraw_core::diagnostics: \
             diagnostics: GPU stage Output 652x978: 54.02 ms in 3 passes (prepare_scene_node 4.08)",
        )
        .unwrap();
        assert_eq!(
            stage.diagnostics(23271).and_then(stage_time),
            Some(("Output 652x978", 54.02))
        );
        assert!(LogLine::parse("--------- beginning of main").is_none());
    }

    #[test]
    fn stage_stats_group_the_apps_stage_lines() {
        let line = |time: f64, pid: u32, message: &str| LogLine {
            time,
            pid,
            tag: "CalibRaw".to_owned(),
            message: message.to_owned(),
        };
        let lines = [
            line(10.0, 7, "calibraw_core::diagnostics: diagnostics: GPU stage Output 652x978: 54.21 ms in 3 passes (prepare_scene_node 33.94, apply_scene_tone_node 20.07, view 0.20)"),
            line(10.1, 7, "calibraw_core::diagnostics: diagnostics: GPU stage Output 652x978: 70.00 ms in 3 passes (a 1.00)"),
            line(10.2, 7, "calibraw_core::diagnostics: diagnostics: GPU stage Output 652x978: 60.00 ms in 3 passes (a 1.00)"),
            line(10.3, 7, "calibraw_core::diagnostics: diagnostics: GPU stage Raw 652x978: 400.00 ms in 1 pass and 3 untimed (raw 1.00)"),
            line(10.4, 8, "calibraw_core::diagnostics: diagnostics: GPU stage Output 652x978: 900.00 ms in 3 passes (a 1.00)"),
            line(10.5, 7, "calibraw_core::diagnostics: diagnostics: Returned freed memory: resident 1717 -> 417 MiB in 362 ms"),
        ];
        let stats = stage_stats(&lines, 7);
        // Only the diagnostics recorder's lines count.
        let other = line(
            10.6,
            7,
            "calibraw_gpu::x: diagnostics: GPU stage Raw 1x1: 1.00 ms in 1 pass (a 1.00)",
        );
        assert_eq!(other.diagnostics(7), None);
        assert_eq!(stats.len(), 2);
        let output = &stats["Output 652x978"];
        assert_eq!(
            (output.count, output.median_ms, output.max_ms),
            (3, 60.0, 70.0)
        );
        assert!((output.per_second.unwrap() - 10.0).abs() < 1e-6);
        assert_eq!(stats["Raw 652x978"].per_second, None);
    }
}
