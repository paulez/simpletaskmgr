use std::collections::VecDeque;

use log::warn;

use crate::cpu_status::{read_cpu0_freq_mhz, read_cpu_temp_c};
use crate::disk_status::{read_uptime_secs, DiskSample, DiskStatus};
use crate::gpu_status::GpuSample;

/// One system-wide usage sample.
///
/// `cpu`, `mem`, `gpu_use` and `gpu_vram` are percentages (0–100), the same
/// units the graph plots. The GPU fields (and `freq`/`temp`) are `Option`
/// because their source is platform-conditional — a host without a GPU or,
/// respectively, the `cpufreq`/`hwmon` interface reports `None` and the UI
/// reads those as a blank series. `disks` is a per-disk reading (empty on no
/// physical disk); it breaks `Copy`, so the history clones on access.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// System CPU utilization percent (0–100) for the interval since the
    /// previous sample.
    pub cpu: f64,
    /// Memory utilization percent ((MemTotal − MemAvailable) / MemTotal) for
    /// the instant this sample was taken (0–100).
    pub mem: f64,
    /// Current frequency of core 0 in MHz (core 0 is the representative "CPU
    /// frequency" source), or `None` when the `cpufreq` interface is absent.
    pub freq: Option<f64>,
    /// CPU temperature in °C read from the CPU's `hwmon` sensor, or `None`
    /// when no CPU sensor is available.
    pub temp: Option<f64>,
    /// GPU utilization percent (0–100) read from `rocm-smi` `card0`, or
    /// `None` when no GPU is available.
    pub gpu_use: Option<f64>,
    /// GPU VRAM allocation percent (0–100) read from `rocm-smi` `card0`, or
    /// `None` when no GPU is available.
    pub gpu_vram: Option<f64>,
    /// GPU edge temperature in °C read from `rocm-smi` `card0`, or `None`
    /// when no GPU is available.
    pub gpu_temp: Option<f64>,
    /// Per-physical-disk read/write throughput and utilization for this
    /// interval, one entry per physical disk. Empty when no physical disk is
    /// present; each field is `None` (and the UI shows a blank) when the
    /// reading is not measurable — a counter regression on any sample, or the
    /// first sample on a host without a readable `/proc/uptime`. The first
    /// sample otherwise reports a since-boot lifetime rate, so the series
    /// starts at the left edge instead of leaving a blank slot.
    pub disks: Vec<DiskSample>,
}

/// Baseline state used to compute a CPU% from `/proc/stat` tick deltas between
/// two samples (same "delta since last read" principle as `CpuTracker`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatBaseline {
    /// Sum of all fields of the aggregate `cpu` line at the last sample.
    pub last_total: u64,
    /// Idle time (field 5) of the aggregate `cpu` line at the last sample.
    pub last_idle: u64,
}

/// Parses one aggregate `cpu` line from `/proc/stat` into its total and idle
/// tick counts.
///
/// Field layout (space-separated, after the `cpu` label):
/// user nice system idle iowait irq softirq steal guest guest_nice.
/// `idle` here is fields 4 + 5 (idle + iowait), which is the conventional
/// "not doing work" time for utilization math.
///
/// Returns `None` if the line can't be parsed as at least five fields.
pub fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let user: u64 = fields.next()?.parse().ok()?;
    let nice: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    let idle: u64 = fields.next()?.parse().ok()?;
    let iowait: u64 = fields.next()?.parse().ok()?;
    let rest: u64 = fields.map(|f| f.parse::<u64>().unwrap_or(0)).sum();
    let total = user
        .checked_add(nice)?
        .checked_add(system)?
        .checked_add(idle)?
        .checked_add(iowait)?
        .checked_add(rest)?;
    let idle_total = idle.checked_add(iowait)?;
    Some((total, idle_total))
}

/// Computes CPU utilization percent from two aggregate `/proc/stat` readings.
///
/// `None` means the delta is unusable (the system was rebooted and counters
/// reset, or the values underflowed), mirroring the PID-reuse case in
/// `CpuTracker::calculate_cpu_percent`: the caller should discard the baseline
/// and treat the next interval as the first real one.
pub fn cpu_percent(last: &StatBaseline, total: u64, idle: u64) -> Option<f64> {
    let total_delta = total.checked_sub(last.last_total)?;
    let idle_delta = idle.checked_sub(last.last_idle)?;
    if total_delta == 0 {
        return Some(0.0);
    }
    let non_idle = total_delta.checked_sub(idle_delta)?;
    Some((non_idle as f64 / total_delta as f64) * 100.0)
}

/// Computes a system-wide since-boot average CPU% for the very first sample:
/// `(total - idle)` ticks divided by `uptime_secs * tps`. Mirrors
/// `CpuTracker::lifetime_avg_percent` (the "first frame" idea) and the disk
/// graph's `lifetime_rates`.
///
/// `None` when either input is `None` or non-positive — the first sample
/// can't be computed without a denominator, so the whole thing is *unknown*
/// rather than a spurious zero. The caller maps `None` to `0.0` for the
/// graph (matching the old behavior) and to a blank readout for the label.
pub fn lifetime_cpu_percent(
    total: u64,
    idle: u64,
    tps: u64,
    uptime_secs: Option<f64>,
) -> Option<f64> {
    let up = uptime_secs?;
    if up <= 0.0 || tps == 0 {
        return None;
    }
    let non_idle = total.checked_sub(idle)?;
    Some(100.0 * non_idle as f64 / (up * tps as f64))
}

/// Returns the integer value (in kB) of one `Key:` field in `/proc/meminfo`
/// text, or `None` when the field is absent. Shared by the percent and total
/// parsers below and by the per-row MEM% cell in `process_list`.
pub fn meminfo_field_kb(meminfo: &str, key: &str) -> Option<u64> {
    for line in meminfo.lines() {
        let label = format!("{key}:");
        let mut parts = line.split_whitespace();
        if parts.next() != Some(label.as_str()) {
            continue;
        }
        return parts.next()?.parse().ok();
    }
    None
}

/// Parses `/proc/meminfo` text and returns memory utilization percent,
/// `(MemTotal − MemAvailable) / MemTotal * 100`.
///
/// Returns `None` if either field is missing or `MemTotal` is zero.
pub fn meminfo_used_percent(meminfo: &str) -> Option<f64> {
    let total = meminfo_field_kb(meminfo, "MemTotal")?;
    let available = meminfo_field_kb(meminfo, "MemAvailable")?;
    if total == 0 {
        return None;
    }
    let used = total.checked_sub(available)?;
    Some((used as f64 / total as f64) * 100.0)
}

/// Reads and parses the live `/proc/meminfo`. Returns `None` (and logs a
/// warning) if the file is unreadable or lacks the required fields.
fn read_meminfo_percent() -> Option<f64> {
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => meminfo_used_percent(&text),
        Err(e) => {
            warn!("Can't read /proc/meminfo: {e:?}");
            None
        }
    }
}

/// Reads `MemTotal` from the live `/proc/meminfo`, in kB. Returns `None`
/// (and logs a warning) if the file is unreadable or lacks the field; every
/// row then shows a blank MEM% cell.
pub fn read_mem_total_kb() -> Option<u64> {
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => match meminfo_field_kb(&text, "MemTotal") {
            Some(kb) => Some(kb),
            None => {
                warn!("MemTotal: not found in /proc/meminfo");
                None
            }
        },
        Err(e) => {
            warn!("Can't read /proc/meminfo: {e:?}");
            None
        }
    }
}

/// Reads the live `/proc/meminfo` and returns total memory in **MB**
/// (`MemTotal kB / 1024`), or `None` (with a warning) if the file is
/// unreadable or lacks `MemTotal`. Used as the top of the memory-axis domain,
/// mirroring the role `read_max_freq_mhz` plays for the frequency axis.
pub fn read_mem_total_mb() -> Option<f64> {
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => meminfo_total_mb(&text),
        Err(e) => {
            warn!("Can't read /proc/meminfo: {e:?}");
            None
        }
    }
}

/// Total physical memory in **MB** (`MemTotal kB / 1024`), or `None` when
/// the field is missing or `MemTotal` is zero.
pub fn meminfo_total_mb(meminfo: &str) -> Option<f64> {
    let kb = meminfo_field_kb(meminfo, "MemTotal")?;
    if kb == 0 {
        return None;
    }
    Some(kb as f64 / 1024.0)
}

/// Holds a rolling history of system-wide CPU/memory samples so the UI can
/// plot them over time.
///
/// Call `push_sample()` once per refresh; it appends one sample and keeps at
/// most `MAX_HISTORY` of them, dropping the oldest. The UI reads `history()`
/// each tick and redraws the graph.
pub struct SystemMetrics {
    history: VecDeque<Sample>,
    cap: usize,
    stat_baseline: Option<StatBaseline>,
    disk: DiskStatus,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMetrics {
    /// The default number of samples kept in the rolling history.
    /// At the default 1.5s refresh this is about 3 minutes.
    pub const MAX_HISTORY: usize = 120;

    pub fn new() -> Self {
        Self {
            history: VecDeque::new(),
            cap: Self::MAX_HISTORY,
            stat_baseline: None,
            disk: DiskStatus::new(),
        }
    }

    /// Creates metrics with the given history cap (for tests).
    pub fn with_cap(cap: usize) -> Self {
        Self {
            history: VecDeque::new(),
            cap,
            stat_baseline: None,
            disk: DiskStatus::new(),
        }
    }

    /// The current history, oldest first.
    pub fn history(&self) -> Vec<Sample> {
        self.history.iter().cloned().collect()
    }

    /// The current system CPU% / memory% / frequency / temperature, the
    /// per-disk I/O reading, plus the last GPU reading (utilization, VRAM,
    /// temperature).
    ///
    /// CPU% for this interval is computed from the delta since the last call.
    /// The first call has no baseline, so instead of a flat `0.0` it returns
    /// a since-boot lifetime average (`total - idle` ticks / uptime — the
    /// "first frame" idea behind `CpuTracker::lifetime_avg_percent`) so the
    /// graph series starts at the left edge with a real value; this maps to
    /// `0.0` only when `/proc/uptime` is unreadable (rare on a real Linux
    /// host), preserving the old behavior in that edge case.
    /// Memory% is read directly from `/proc/meminfo`. Frequency and temperature
    /// are read from `sysfs`; on a read failure the previous value (or `None`
    /// for the first sample) is carried forward so the graph doesn't dip to
    /// zero spuriously. The three GPU fields are likewise carried forward from
    /// the previous sample when `gpu` is `None` — for the first call the value
    /// is `None`, and the caller on a host with no GPU can keep skipping the
    /// `rocm-smi` spawn by passing `None` every tick (see
    /// [`crate::gpu_status::gpu_available`]).
    ///
    /// The per-disk reading (read/write bytes-per-second and utilization) is
    /// sampled from `/proc/diskstats` each call inside the tracker — there is
    /// no new `push_sample` parameter (unlike the GPU, whose source is a
    /// process spawn the caller may skip). A device's first sample
    /// reports a since-boot lifetime rate (accumulated counters / uptime);
    /// subsequent samples measure the interval since the previous one. Both
    /// fall back to `None` when `/proc/uptime` is unreadable, in which
    /// case the UI shows a blank instead of a fake zero.
    pub fn push_sample(&mut self, gpu: Option<GpuSample>) {
        let cpu = self.sample_cpu().unwrap_or(0.0);
        let mem = self.sample_mem().unwrap_or(0.0);
        let disks = self.disk.snapshot();
        log::debug!("cpu used {cpu:.1}% (mem {mem:.1}%)");
        // Frequency and temperature are live reads; when a read fails the
        // previous value (if any) is carried forward so a momentary `sysfs`
        // hiccup doesn't blank the series.
        let prev = self.latest();
        let freq = match read_cpu0_freq_mhz() {
            Some(f) => Some(f),
            None => prev.as_ref().and_then(|s| s.freq),
        };
        let temp = match read_cpu_temp_c() {
            Some(t) => Some(t),
            None => prev.as_ref().and_then(|s| s.temp),
        };
        // GPU: same carry-over rule as freq/temp. On a host with no GPU the
        // caller keeps passing `None` (and never spawns rocm-smi), so the
        // fields stay `None` for the lifetime of the session.
        let (gpu_use, gpu_vram, gpu_temp) = match gpu {
            Some(g) => (Some(g.use_pct), Some(g.vram_pct), Some(g.temp_c)),
            None => (
                prev.as_ref().and_then(|s| s.gpu_use),
                prev.as_ref().and_then(|s| s.gpu_vram),
                prev.as_ref().and_then(|s| s.gpu_temp),
            ),
        };
        self.history.push_back(Sample {
            cpu,
            mem,
            freq,
            temp,
            gpu_use,
            gpu_vram,
            gpu_temp,
            disks,
        });
        while self.history.len() > self.cap {
            self.history.pop_front();
        }
    }

    /// The newest sample (if any).
    fn latest(&self) -> Option<Sample> {
        self.history.back().cloned()
    }

    fn sample_cpu(&mut self) -> Option<f64> {
        let (total, idle) = read_live_cpu_line();
        // `(0, 0)` can only mean the aggregate `cpu` line was missing or
        // unparsable — a real `/proc/stat` always reports non-zero cumulative
        // counters, so `total == 0` is unambiguously a failed read. Skip
        // recording that as a baseline, or the next interval would compute a
        // bogus spike against it. Returning `None` here maps to `0.0` in
        // `push_sample`, keeping the previous baseline intact for the retry.
        if total == 0 {
            return None;
        }
        // The *first* call has no prior baseline. Rather than reporting a
        // flat `0.0` (the old symptom of "CPU stuck at 0 on the first frame"),
        // return a since-boot lifetime mean — the same "first frame" idea as
        // `CpuTracker::lifetime_avg_percent` for a freshly-tracked process —
        // so the graph series starts with a real value at the left edge.
        // The baseline is recorded unconditionally so the next call measures
        // a proper interval delta.
        let percent = match self.stat_baseline {
            Some(baseline) => cpu_percent(&baseline, total, idle),
            None => {
                lifetime_cpu_percent(total, idle, procfs::ticks_per_second(), read_uptime_secs())
            }
        };
        self.stat_baseline = Some(StatBaseline {
            last_total: total,
            last_idle: idle,
        });
        percent
    }

    fn sample_mem(&self) -> Option<f64> {
        read_meminfo_percent().inspect(|&p| log::debug!("meminfo used {p:.1}%"))
    }
}

/// Reads and parses the live aggregate `cpu` line of `/proc/stat`.
///
/// Returns the aggregate (total, idle) tick counts, or `(0, 0)` (with a
/// warning) if the file is unreadable or malformed.
fn read_live_cpu_line() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let line = text.lines().find(|l| l.starts_with("cpu")).unwrap_or("");
    match parse_cpu_line(line) {
        Some(v) => v,
        None => {
            warn!("Can't parse aggregate cpu line from /proc/stat");
            (0, 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// `parse_cpu_line` sums the aggregate (non-idle + idle) fields of the
    /// whole-system `cpu` line and rejects per-core (`cpu0`) or malformed lines.
    #[rstest]
    #[case::aggregate(
        "cpu  100 200 300 400 500 600 700 800 900 0",
        Some((4500, 900))
    )]
    #[case::rejects_per_core("cpu0 1 2 3 4 5 6 7 8 9 0", None)]
    #[case::rejects_short("cpu 1 2 3", None)]
    fn test_parse_cpu_line(#[case] line: &str, #[case] expected: Option<(u64, u64)>) {
        assert_eq!(parse_cpu_line(line), expected);
    }

    /// `cpu_percent` from two counter samples: busy, idle, a counter reset
    /// (underflow -> None) and no elapsed time (-> Some(0)).
    #[rstest]
    #[case::busy(1000, 800, 2000, 1300, Some(50.0))]
    #[case::idle(1000, 900, 2000, 1900, Some(0.0))]
    #[case::counters_reset(10_000, 9_000, 100, 90, None)]
    #[case::no_time_elapsed(1000, 800, 1000, 800, Some(0.0))]
    fn test_cpu_percent(
        #[case] last_total: u64,
        #[case] last_idle: u64,
        #[case] total: u64,
        #[case] idle: u64,
        #[case] expected: Option<f64>,
    ) {
        let last = StatBaseline {
            last_total,
            last_idle,
        };
        assert_eq!(cpu_percent(&last, total, idle), expected);
    }

    /// `meminfo_used_percent` = 1 - MemAvailable/MemTotal; `None` when a field
    /// is missing or MemTotal is zero.
    #[rstest]
    #[case::fifty_percent(
        "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:     8000000 kB\n",
        Some(50.0)
    )]
    #[case::missing_fields("MemTotal: 100 kB\n", None)]
    #[case::empty("", None)]
    #[case::zero_total("MemTotal: 0 kB\nMemAvailable: 0 kB\n", None)]
    fn test_meminfo_used_percent(#[case] meminfo: &str, #[case] expected: Option<f64>) {
        assert_eq!(meminfo_used_percent(meminfo), expected);
    }

    /// `meminfo_total_mb` reports MemTotal in MB; `None` if absent or zero.
    #[rstest]
    #[case::sixteen_gib(
        "MemTotal:       16777216 kB\nMemFree:         1000000 kB\n",
        Some(16384.0)
    )]
    #[case::exact_mb("MemTotal:       16000000 kB\nMemFree:         1000000 kB\n", Some(16000000.0 / 1024.0))]
    #[case::missing("MemFree: 100 kB\n", None)]
    #[case::empty("", None)]
    #[case::zero_total("MemTotal: 0 kB\n", None)]
    fn test_meminfo_total_mb(#[case] meminfo: &str, #[case] expected: Option<f64>) {
        assert_eq!(meminfo_total_mb(meminfo), expected);
    }

    #[test]
    fn test_push_sample_caps_history() {
        let mut m = SystemMetrics::with_cap(3);
        for _ in 0..5 {
            m.push_sample(None);
        }
        assert_eq!(m.history().len(), 3);
    }

    #[test]
    fn test_push_sample_updates_history_and_last() {
        let mut m = SystemMetrics::with_cap(10);
        assert!(m.history().is_empty());
        m.push_sample(None);
        assert_eq!(m.history().len(), 1);
    }

    /// Regression test for "CPU stuck at 0": on the *first* read there is no
    /// prior baseline, so the code must (a) record one for the next interval
    /// and (b) return a usable value — historically it returned `None` and
    /// the UI mapped that to `0.0`, looking like a permanent "CPU idle". The
    /// fix returns a since-boot lifetime average (`total - idle` ticks /
    /// uptime) so the first frame is a real measurement, mirroring the CPU
    /// per-process `lifetime_avg_percent` already used for freshly-tracked
    /// PIDs.
    ///
    /// The second read measures a proper interval delta against the recorded
    /// baseline and must be `Some` on any host with a readable `/proc/stat`.
    #[test]
    fn test_second_cpu_sample_is_measured_not_stuck_zero() {
        let mut m = SystemMetrics::with_cap(10);
        // First read: no baseline yet, so the value comes from a since-boot
        // lifetime average (never a flat 0.0). It *must* still record a
        // baseline for the next interval to measure against — that is the
        // exact step the original bug skipped.
        let first = m.sample_cpu();
        assert!(
            m.stat_baseline.is_some(),
            "the first read must record a baseline for the next interval"
        );
        assert!(
            first.is_some(),
            "the first read must return Some (a since-boot lifetime \
             average or a real percent) — None maps to 0.0 and reads as \
             'CPU stuck at 0'"
        );
        // Second read: a baseline now exists, so a real measurement is
        // possible and the permanent-None / permanent-zero symptom is gone.
        let second = m.sample_cpu();
        assert!(
            second.is_some(),
            "the second read must return Some (some %), not None — the old \
             bug returned None forever after the first call"
        );
    }

    /// `lifetime_cpu_percent` is `(total - idle)` ticks / (`uptime * tps`).
    /// Equivalently: the fraction of the system's wall-clock time spent not
    /// idle. `None` when `uptime_secs` is `None` or non-positive (missing or
    /// bad `/proc/uptime`), when `tps` is `0`, or when `total < idle` (a
    /// counter regression can't be measured). The `CpuTracker::
    /// lifetime_avg_percent` analog is `(utime + stime) / elapsed_ticks *
    /// 100` for a single process; we use `(total - idle)` here for the
    /// system CPU, which is the same formula.
    ///
    /// All cases use `tps = 100` (the common Linux value) and `uptime = 10`:
    /// a wall of `10 × 100 = 1000` ticks. `non_idle` must be `0.5 · 1000`
    /// for 50%, `0.9 · 1000` for 90%, `0 · 1000` for 0%.
    #[rstest]
    #[case::fifty_percent(1000, 500, 100, Some(10.0), Some(50.0))]
    #[case::busy(1000, 100, 100, Some(10.0), Some(90.0))]
    #[case::idle(500, 500, 100, Some(10.0), Some(0.0))]
    #[case::no_uptime(1000, 500, 100, None, None)]
    #[case::zero_uptime(1000, 500, 100, Some(0.0), None)]
    #[case::negative_uptime(1000, 500, 100, Some(-1.0), None)]
    #[case::zero_tps(1000, 500, 0, Some(10.0), None)]
    #[case::counter_regression(10, 20, 100, Some(10.0), None)]
    fn test_lifetime_cpu_percent(
        #[case] total: u64,
        #[case] idle: u64,
        #[case] tps: u64,
        #[case] uptime: Option<f64>,
        #[case] expected: Option<f64>,
    ) {
        let got = lifetime_cpu_percent(total, idle, tps, uptime);
        assert_eq!(
            got, expected,
            "inputs total={total} idle={idle} tps={tps} uptime={uptime:?}"
        );
    }

    /// A pushed sample records the live core-0 frequency when the `cpufreq`
    /// interface is present. The current frequency is a *live* reading that
    /// changes between samples (turbo boost), so we assert the recorded
    /// value, when present, is positive and physically sane — not that it
    /// equals some other moment's reading. A sample with no `cpufreq` source
    /// records `None`.
    #[test]
    fn test_push_sample_carries_freq() {
        let mut m = SystemMetrics::with_cap(10);
        m.push_sample(None);
        let last = m.history().pop().unwrap();
        if let Some(mhz) = last.freq {
            assert!(mhz > 0.0, "recorded freq must be positive");
            assert!(
                mhz < 1_000_000.0,
                "recorded freq must be physically sane Hz range"
            );
        }
        // A later sample with a successful live read also records a sane value.
        m.push_sample(None);
        let last2 = m.history().pop().unwrap();
        if let Some(mhz) = last2.freq {
            assert!(mhz > 0.0, "second sample's freq must be positive");
            assert!(mhz < 1_000_000.0, "second sample's freq must be sane");
        }
    }

    /// A pushed sample records the GPU reading that was supplied to it. When
    /// `None` was *first* passed (no GPU on this host), the GPU fields are
    /// `None` and remain `None` on subsequent `None`-carrying samples — the
    /// `Option` is carried forward, not reset to zero (a spurious `0` would
    /// look like a live reading of "GPU idle").
    #[test]
    fn test_push_sample_carries_gpu() {
        let mut m = SystemMetrics::with_cap(10);

        // No GPU provided (typical on a non-AMD host).
        m.push_sample(None);
        let last = m.history().pop().unwrap();
        assert_eq!(last.gpu_use, None);
        assert_eq!(last.gpu_vram, None);
        assert_eq!(last.gpu_temp, None);

        // Passing a concrete GpuSample on the next tick records it.
        m.push_sample(Some(GpuSample {
            use_pct: 42.0,
            vram_pct: 77.0,
            temp_c: 61.0,
        }));
        let last = m.history().pop().unwrap();
        assert_eq!(last.gpu_use, Some(42.0));
        assert_eq!(last.gpu_vram, Some(77.0));
        assert_eq!(last.gpu_temp, Some(61.0));

        // A follow-up `None` (a failed spawn on a live GPU, or a GPU that
        // dropped off the bus) carries the last reading forward rather than
        // blanking to zero.
        m.push_sample(None);
        let last = m.history().pop().unwrap();
        assert_eq!(last.gpu_use, Some(42.0), "carry-over must hold 42.0");
        assert_eq!(last.gpu_vram, Some(77.0));
        assert_eq!(last.gpu_temp, Some(61.0));
    }

    /// A pushed sample records a per-disk reading. The first sample now
    /// reports a since-boot lifetime rate for each disk (counters / uptime),
    /// so the graph series starts at the left edge instead of leaving a blank
    /// slot. On a host with no physical disk (empty `disks`) or with an
    /// unreadable `/proc/uptime` (`None` rates), both are acceptable outcomes —
    /// the test asserts only the universally-valid invariants (>= 0 for rates,
    /// 0–100 for utilization) that must hold regardless of which case fired.
    #[test]
    fn test_push_sample_carries_disks() {
        let mut m = SystemMetrics::with_cap(10);
        m.push_sample(None);
        let first = m.history().pop().unwrap();
        for d in &first.disks {
            if let Some(b) = d.read_bps {
                assert!(b >= 0.0);
            }
            if let Some(w) = d.write_bps {
                assert!(w >= 0.0);
            }
            if let Some(u) = d.util_pct {
                assert!((0.0..=100.0).contains(&u));
            }
        }
        // The second sample now has a baseline, so each disk reports real
        // (>= 0) readings against the previous one.
        std::thread::sleep(std::time::Duration::from_millis(50));
        m.push_sample(None);
        let second = m.history().pop().unwrap();
        assert_eq!(second.disks.len(), first.disks.len(), "same set of disks");
        for d in &second.disks {
            if let Some(b) = d.read_bps {
                assert!(b >= 0.0);
            }
            if let Some(w) = d.write_bps {
                assert!(w >= 0.0);
            }
            if let Some(u) = d.util_pct {
                assert!((0.0..=100.0).contains(&u));
            }
        }
    }
}
