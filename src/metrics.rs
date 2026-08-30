use std::collections::VecDeque;

use log::warn;

use crate::cpu_status::{read_cpu0_freq_mhz, read_cpu_temp_c};

/// One system-wide usage sample.
///
/// `cpu` and `mem` are percentages (0–100), the same units the graph plots.
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// Parses `/proc/meminfo` text and returns memory utilization percent,
/// `(MemTotal − MemAvailable) / MemTotal * 100`.
///
/// Returns `None` if either field is missing or `MemTotal` is zero.
pub fn meminfo_used_percent(meminfo: &str) -> Option<f64> {
    fn field_kb(meminfo: &str, key: &str) -> Option<u64> {
        for line in meminfo.lines() {
            let label = format!("{key}:");
            let mut parts = line.split_whitespace();
            if parts.next() != Some(label.as_str()) {
                continue;
            }
            if let Some(value) = parts.next() {
                return value.parse::<u64>().ok();
            }
        }
        None
    }
    let total = field_kb(meminfo, "MemTotal")?;
    let available = field_kb(meminfo, "MemAvailable")?;
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
        }
    }

    /// Creates metrics with the given history cap (for tests).
    pub fn with_cap(cap: usize) -> Self {
        Self {
            history: VecDeque::new(),
            cap,
            stat_baseline: None,
        }
    }

    /// The current history, oldest first.
    pub fn history(&self) -> Vec<Sample> {
        self.history.iter().copied().collect()
    }

    /// The current system CPU% / memory% / frequency / temperature.
    ///
    /// CPU% for this interval is computed from the delta since the last call;
    /// the first call has no baseline, so its CPU value is 0.0. Memory% is
    /// read directly from `/proc/meminfo`. Frequency and temperature are read
    /// from `sysfs`; on a read failure the previous value (or `None` for the
    /// first sample) is carried forward so the graph doesn't dip to zero
    /// spuriously.
    pub fn push_sample(&mut self) {
        let cpu = self.sample_cpu().unwrap_or(0.0);
        let mem = self.sample_mem().unwrap_or(0.0);
        // Frequency and temperature are live reads; when a read fails the
        // previous value (if any) is carried forward so a momentary `sysfs`
        // hiccup doesn't blank the series.
        let prev = self.latest();
        let freq = match read_cpu0_freq_mhz() {
            Some(f) => Some(f),
            None => prev.and_then(|s| s.freq),
        };
        let temp = match read_cpu_temp_c() {
            Some(t) => Some(t),
            None => prev.and_then(|s| s.temp),
        };
        self.history.push_back(Sample {
            cpu,
            mem,
            freq,
            temp,
        });
        while self.history.len() > self.cap {
            self.history.pop_front();
        }
    }

    /// The newest sample (if any).
    fn latest(&self) -> Option<Sample> {
        self.history.back().copied()
    }

    fn sample_cpu(&mut self) -> Option<f64> {
        let (total, idle) = read_live_cpu_line();
        let baseline = self.stat_baseline?;
        let percent = cpu_percent(&baseline, total, idle);
        let new_baseline = StatBaseline {
            last_total: total,
            last_idle: idle,
        };
        self.stat_baseline = Some(new_baseline);
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

    #[test]
    fn test_parse_cpu_line_happy() {
        let s = "cpu  100 200 300 400 500 600 700 800 900 0";
        let (total, idle) = parse_cpu_line(s).unwrap();
        assert_eq!(total, 100 + 200 + 300 + 400 + 500 + 600 + 700 + 800 + 900);
        assert_eq!(idle, 400 + 500);
    }

    #[test]
    fn test_parse_cpu_line_rejects_per_core() {
        let s = "cpu0 1 2 3 4 5 6 7 8 9 0";
        assert!(parse_cpu_line(s).is_none());
    }

    #[test]
    fn test_parse_cpu_line_insufficient_fields() {
        assert!(parse_cpu_line("cpu 1 2 3").is_none());
    }

    #[test]
    fn test_cpu_percent_busy() {
        let last = StatBaseline {
            last_total: 1000,
            last_idle: 800,
        };
        // total grew by 1000, idle by 500 => half the interval was non-idle
        assert_eq!(cpu_percent(&last, 2000, 1300), Some(50.0));
    }

    #[test]
    fn test_cpu_percent_idle() {
        let last = StatBaseline {
            last_total: 1000,
            last_idle: 900,
        };
        assert_eq!(cpu_percent(&last, 2000, 1900), Some(0.0));
    }

    #[test]
    fn test_cpu_percent_counters_reset_returns_none() {
        let last = StatBaseline {
            last_total: 10_000,
            last_idle: 9_000,
        };
        // reboot: counters smaller than baseline -> underflow -> None (reset)
        assert_eq!(cpu_percent(&last, 100, 90), None);
    }

    #[test]
    fn test_cpu_percent_no_time_elapsed() {
        let last = StatBaseline {
            last_total: 1000,
            last_idle: 800,
        };
        assert_eq!(cpu_percent(&last, 1000, 800), Some(0.0));
    }

    #[test]
    fn test_meminfo_used_percent_happy() {
        let m = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:     8000000 kB\n";
        assert_eq!(meminfo_used_percent(m), Some(50.0));
    }

    #[test]
    fn test_meminfo_missing_fields() {
        assert!(meminfo_used_percent("MemTotal: 100 kB\n").is_none());
        assert!(meminfo_used_percent("").is_none());
    }

    #[test]
    fn test_meminfo_zero_total() {
        let m = "MemTotal: 0 kB\nMemAvailable: 0 kB\n";
        assert!(meminfo_used_percent(m).is_none());
    }

    #[test]
    fn test_push_sample_caps_history() {
        let mut m = SystemMetrics::with_cap(3);
        for _ in 0..5 {
            m.push_sample();
        }
        assert_eq!(m.history().len(), 3);
    }

    #[test]
    fn test_push_sample_updates_history_and_last() {
        let mut m = SystemMetrics::with_cap(10);
        assert!(m.history().is_empty());
        m.push_sample();
        assert_eq!(m.history().len(), 1);
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
        m.push_sample();
        let last = m.history().pop().unwrap();
        if let Some(mhz) = last.freq {
            assert!(mhz > 0.0, "recorded freq must be positive");
            assert!(
                mhz < 1_000_000.0,
                "recorded freq must be physically sane Hz range"
            );
        }
        // A later sample with a successful live read also records a sane value.
        m.push_sample();
        let last2 = m.history().pop().unwrap();
        if let Some(mhz) = last2.freq {
            assert!(mhz > 0.0, "second sample's freq must be positive");
            assert!(mhz < 1_000_000.0, "second sample's freq must be sane");
        }
    }
}
