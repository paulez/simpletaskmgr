use log::debug;
use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::HashMap;
use std::time::Instant;

use crate::process::TaskMgrProcess;

#[derive(Clone)]
pub struct CpuTracker {
    process_usage: HashMap<i32, UsageStats>,
    tps: u64,
    start_instant: Instant,
}

#[derive(Clone, Default)]
pub struct UsageStats {
    pub last_ticks: (u64, u64),
    pub last_timestamp: f64,
}

impl UsageStats {
    fn new(utime: u64, stime: u64, last_timestamp: f64) -> Self {
        Self {
            last_ticks: (utime, stime),
            last_timestamp,
        }
    }
}

impl Default for CpuTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuTracker {
    pub fn new() -> Self {
        Self {
            process_usage: HashMap::new(),
            tps: procfs::ticks_per_second(),
            start_instant: Instant::now(),
        }
    }

    /// Computes a process's CPU% (and tick sort key) using the delta between
    /// the current and last samples' tick counters. Returns an
    /// `Option<(ticks, percent)>` so the caller can distinguish a clean
    /// `Some` result from a PID reuse (a counter that went backwards, which
    /// would underflow the subtraction).
    fn calculate_cpu_sample(
        recent_utime: u64,
        recent_stime: u64,
        last_ticks: (u64, u64),
        tps: u64,
        current_timestamp: f64,
        last_timestamp: f64,
    ) -> Option<(u64, f64)> {
        let recent_total = recent_utime.checked_add(recent_stime)?;
        let last_total = last_ticks.0.checked_add(last_ticks.1)?;
        let total_ticks_delta = recent_total.checked_sub(last_total)?;

        let time_elapsed = current_timestamp - last_timestamp;

        if time_elapsed <= 0.0 {
            return Some((total_ticks_delta, 0.0));
        }

        // Convert ticks to seconds and calculate percentage
        let cpu_seconds = total_ticks_delta as f64 / tps as f64;
        Some((total_ticks_delta, (cpu_seconds / time_elapsed) * 100.0))
    }

    fn update_history(usage: &mut UsageStats, utime: u64, stime: u64, timestamp: f64) {
        usage.last_ticks = (utime, stime);
        usage.last_timestamp = timestamp;
    }

    /// Updates `task_mgr_process`'s CPU% (display value) and CPU sort key
    /// (`cpu_ticks`, see its docs) using the tick delta since the last sample.
    ///
    /// First sight of a pid (and a pid-reuse baseline reset) set both the
    /// key and the display to zero: a freshly-appearing process has no
    /// measured window yet, so it starts in the zero group and rises to its
    /// true bucket within one or two refreshes. See
    /// `doc/CPU_FIRST_SIGHT_BLANK_LINES.md` for why we intentionally do
    /// **not** use `top`'s since-start average here (it sent fresh procs to
    /// the top of the descending list and triggered whole-list widget
    /// churn/blank rows in GTK's `SortListModel`).
    ///
    /// Expects a `stat` that was already read (and shared with the caller) so the
    /// `/proc` `stat` file is only opened once per refresh.
    pub fn update_process_cpu(
        &mut self,
        task_mgr_process: &mut TaskMgrProcess,
        stat: &procfs::process::Stat,
    ) {
        let utime = stat.utime;
        let stime = stat.stime;
        let pid = task_mgr_process.pid;

        // Use Instant for high-resolution timing
        let current_timestamp = self.start_instant.elapsed().as_secs_f64();

        match self.process_usage.entry(pid) {
            Occupied(mut occ) => {
                let usage = occ.get_mut();

                // Calculate CPU percentage using the delta between current and last ticks
                // A `None` result means the ticks decreased, so the PID was reused by a
                // new process: discard the stale baseline and treat this as a new process.
                let cpu_sample = Self::calculate_cpu_sample(
                    utime,
                    stime,
                    usage.last_ticks,
                    self.tps,
                    current_timestamp,
                    usage.last_timestamp,
                );

                if let Some((cpu_ticks, cpu_percent)) = cpu_sample {
                    task_mgr_process.cpu_percent = cpu_percent;
                    task_mgr_process.cpu_ticks = cpu_ticks;
                    Self::update_history(usage, utime, stime, current_timestamp);
                } else {
                    debug!("PID {} was reused, resetting CPU baseline", pid);
                    occ.insert(UsageStats::new(utime, stime, current_timestamp));
                    // Pid reuse: same treatment as first sight (see above) —
                    // zero key and zero display until the next measured delta.
                    task_mgr_process.cpu_ticks = 0;
                    task_mgr_process.cpu_percent = 0.0;
                }
            }
            Vacant(vac) => {
                vac.insert(UsageStats::new(utime, stime, current_timestamp));
                // First sight: no measured window yet, so start at zero (see
                // the function docs for why we do not use the lifetime
                // average here).
                task_mgr_process.cpu_ticks = 0;
                task_mgr_process.cpu_percent = 0.0;
            }
        }
    }

    /// Removes tracking entries for PIDs that no longer exist,
    /// so the map does not grow unboundedly as processes come and go.
    pub fn evict_dead_processes(&mut self, live_pids: impl Fn(i32) -> bool) {
        self.process_usage.retain(|pid, _| live_pids(*pid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Shorthand for `calculate_cpu_sample` with the tests' fixed `tps = 100`.
    fn cpu(ru: u64, rs: u64, lt: (u64, u64), cur: f64, last: f64) -> Option<(u64, f64)> {
        CpuTracker::calculate_cpu_sample(ru, rs, lt, 100, cur, last)
    }

    /// `calculate_cpu_sample` over two cores, tps = 100. Covers the normal
    /// busy/idle math, a shorter history, PID reuse (counter decrease) and a
    /// u64 overflow, plus sub-second intervals. Each `Some` case expects the
    /// raw tick delta (the sort key) alongside the percent.
    #[rstest]
    #[case::full(200, 200, 100, 100, 1001.0, 1000.0, Some((200, 200.0)))]
    #[case::idle(100, 100, 100, 100, 1001.0, 1000.0, Some((0, 0.0)))]
    #[case::partial(150, 150, 100, 100, 1002.0, 1000.0, Some((100, 50.0)))]
    #[case::longer_interval(300, 300, 100, 100, 1010.0, 1000.0, Some((400, 40.0)))]
    #[case::minimal_history(200, 100, 100, 50, 1001.0, 1000.0, Some((150, 150.0)))]
    #[case::pid_reuse(5, 5, 100, 100, 1001.0, 1000.0, None)]
    #[case::ticks_overflow(u64::MAX, 1, 1, 1, 1001.0, 1000.0, None)]
    #[case::subsecond(150, 150, 100, 100, 1000.5, 1000.0, Some((100, 200.0)))]
    fn test_cpu_sample(
        #[case] utime: u64,
        #[case] stime: u64,
        #[case] last_u: u64,
        #[case] last_s: u64,
        #[case] now: f64,
        #[case] last: f64,
        #[case] expected: Option<(u64, f64)>,
    ) {
        let got = cpu(utime, stime, (last_u, last_s), now, last);
        match (got, expected) {
            (Some((ticks, percent)), Some((want_ticks, want_percent))) => {
                assert_eq!(ticks, want_ticks, "tick delta must be the sort key");
                assert!(
                    (percent - want_percent).abs() < 0.01,
                    "got {percent}, expected {want_percent}"
                );
            }
            _ => assert_eq!(got, expected),
        }
    }

    /// `update_process_cpu` starts a freshly-seen pid at a zero key/percent
    /// (no measured window yet), then uses the per-interval delta, and gives
    /// the same zero treatment on a baseline reset (PID reuse). See
    /// `doc/CPU_FIRST_SIGHT_BLANK_LINES.md`.
    #[test]
    fn test_update_process_cpu_first_sight_and_reuse_zero() {
        let stat = procfs::process::Process::myself().unwrap().stat().unwrap();
        let mut tracker = CpuTracker::new();
        let mut proc = TaskMgrProcess::new("probe".to_string(), 7, 0, "u".to_string(), 99.0);

        // First sight: zero key, zero display — the row lands in the zero
        // group and rises once a real delta is measured.
        tracker.update_process_cpu(&mut proc, &stat);
        assert_eq!(proc.cpu_ticks, 0);
        assert_eq!(proc.cpu_percent, 0.0);

        // Second sample with an unchanged tick counter: zero delta, zero key.
        tracker.update_process_cpu(&mut proc, &stat);
        assert_eq!(proc.cpu_ticks, 0);
        assert_eq!(proc.cpu_percent, 0.0);

        // PID reuse: a stale baseline larger than the live counters means the
        // next delta would underflow, so reset and zero the key/percent again
        // (not an underflowed delta, and not the lifetime total — see doc).
        tracker
            .process_usage
            .insert(7, UsageStats::new(1_000_000, 1_000_000, 1.0));
        tracker.update_process_cpu(&mut proc, &stat);
        assert_eq!(proc.cpu_ticks, 0);
        assert_eq!(proc.cpu_percent, 0.0);
    }

    /// `evict_dead_processes` drops baselines whose pid is no longer alive.
    #[test]
    fn test_evict_dead_processes() {
        let mut tracker = CpuTracker::new();
        tracker
            .process_usage
            .insert(100, UsageStats::new(10, 10, 0.0));
        tracker
            .process_usage
            .insert(200, UsageStats::new(20, 20, 0.0));

        tracker.evict_dead_processes(|pid| pid == 100);

        assert_eq!(tracker.process_usage.len(), 1);
        assert!(tracker.process_usage.contains_key(&100));
        assert!(!tracker.process_usage.contains_key(&200));
    }
}
