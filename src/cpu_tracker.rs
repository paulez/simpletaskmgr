use log::debug;
use procfs::prelude::Current;
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

    /// Total system uptime in seconds, as read from /proc/uptime.
    /// Returns 0 if the file can't be read (e.g. in test sandboxes).
    fn system_uptime_secs(&self) -> f64 {
        match procfs::Uptime::current() {
            Ok(u) => u.uptime,
            Err(e) => {
                debug!("Can't read /proc/uptime: {}", e);
                0.0
            }
        }
    }

    /// Calculates CPU percentage from tick deltas.
    ///
    /// Returns `None` when the current tick totals are lower than the stored
    /// baseline, which indicates the PID was reused by a new process and the
    /// baseline must be reset instead of subtracting (which would underflow).
    fn calculate_cpu_percent(
        recent_utime: u64,
        recent_stime: u64,
        last_ticks: (u64, u64),
        tps: u64,
        current_timestamp: f64,
        last_timestamp: f64,
    ) -> Option<f64> {
        let recent_total = recent_utime.checked_add(recent_stime)?;
        let last_total = last_ticks.0.checked_add(last_ticks.1)?;
        let total_ticks_delta = recent_total.checked_sub(last_total)?;

        let time_elapsed = current_timestamp - last_timestamp;

        if time_elapsed <= 0.0 {
            return Some(0.0);
        }

        // Convert ticks to seconds and calculate percentage
        let cpu_seconds = total_ticks_delta as f64 / tps as f64;
        Some((cpu_seconds / time_elapsed) * 100.0)
    }

    /// Computes a process's since-start average CPU% the way `top` does on
    /// its first frame: `lifetime tick delta / system uptime` (library/pids.c:768-779,
    /// src/top/top.c:2842-2850). Used instead of 0.0 for a newly-tracked process
    /// so the list shows a meaningful, non-zero value immediately.
    ///
    /// Returns 0.0 if the process's elapsed lifetime is non-positive
    /// (e.g. /proc/uptime unreadable or stale `starttime`).
    fn lifetime_avg_percent(
        utime: u64,
        stime: u64,
        start_in_ticks: u64,
        tps: u64,
        uptime_secs: f64,
    ) -> f64 {
        let total_ticks = match utime.checked_add(stime) {
            Some(t) => t,
            None => return 0.0,
        };
        // Process age in ticks. `stat.starttime` is already in system ticks;
        // so process_age_ticks = uptime_ticks - start_in_ticks.
        let uptime_ticks = (uptime_secs * tps as f64) as u64;
        let elapsed_ticks = match uptime_ticks.checked_sub(start_in_ticks) {
            Some(s) if s > 0 => s,
            _ => return 0.0,
        };
        (total_ticks as f64) / (elapsed_ticks as f64) * 100.0
    }

    fn update_history(usage: &mut UsageStats, utime: u64, stime: u64, timestamp: f64) {
        usage.last_ticks = (utime, stime);
        usage.last_timestamp = timestamp;
    }

    /// Updates `task_mgr_process`'s CPU% using the tick delta since the last sample.
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
                let cpu_percent = Self::calculate_cpu_percent(
                    utime,
                    stime,
                    usage.last_ticks,
                    self.tps,
                    current_timestamp,
                    usage.last_timestamp,
                );

                if let Some(cpu_percent) = cpu_percent {
                    task_mgr_process.cpu_percent = cpu_percent;
                    Self::update_history(usage, utime, stime, current_timestamp);
                } else {
                    debug!("PID {} was reused, resetting CPU baseline", pid);
                    occ.insert(UsageStats::new(utime, stime, current_timestamp));
                    task_mgr_process.cpu_percent = Self::lifetime_avg_percent(
                        utime,
                        stime,
                        stat.starttime,
                        self.tps,
                        self.system_uptime_secs(),
                    );
                }
            }
            Vacant(vac) => {
                vac.insert(UsageStats::new(utime, stime, current_timestamp));
                task_mgr_process.cpu_percent = Self::lifetime_avg_percent(
                    utime,
                    stime,
                    stat.starttime,
                    self.tps,
                    self.system_uptime_secs(),
                );
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

    /// Test CPU percent calculation for 100% usage (maximum CPU usage)
    #[test]
    fn test_cpu_percent_100_percent() {
        // Simulate a process using 100% CPU
        let recent_utime = 200;
        let recent_stime = 200;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(
            cpu_percent,
            Some(200.0),
            "Should calculate CPU usage based on ticks"
        );
    }

    /// Test CPU percent calculation for 0% usage (idle process)
    #[test]
    fn test_cpu_percent_0_percent() {
        let recent_utime = 100;
        let recent_stime = 100;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(
            cpu_percent,
            Some(0.0),
            "Should calculate 0% CPU usage for idle process"
        );
    }

    /// Test CPU percent calculation for partial usage
    #[test]
    fn test_cpu_percent_partial_usage() {
        let recent_utime = 150;
        let recent_stime = 150;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1002_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(cpu_percent, Some(50.0), "Should calculate 50% CPU usage");
    }

    /// Test CPU percent calculation with different time intervals
    #[test]
    fn test_cpu_percent_different_time_intervals() {
        let recent_utime = 300;
        let recent_stime = 300;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1010_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(
            cpu_percent,
            Some(40.0),
            "Should handle different time intervals correctly"
        );
    }

    /// Test CPU percent calculation with minimal history
    #[test]
    fn test_cpu_percent_minimal_history() {
        let recent_utime = 200;
        let recent_stime = 100;
        let last_ticks = (100, 50);
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(
            cpu_percent,
            Some(150.0),
            "Should handle minimal history correctly"
        );
    }

    /// Test that decreased ticks (PID reuse) returns None instead of underflowing
    #[test]
    fn test_cpu_percent_pid_reuse_returns_none() {
        // New process inherited the PID and has fewer ticks than the old one
        let recent_utime = 5;
        let recent_stime = 5;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(
            cpu_percent, None,
            "Should detect PID reuse when ticks decrease"
        );
    }

    /// Test CPU percent calculation when tick totals overflow u64 components sum
    #[test]
    fn test_cpu_percent_ticks_overflow_returns_none() {
        let recent_utime = u64::MAX;
        let recent_stime = 1;
        let last_ticks = (1, 1);
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = 1000_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert_eq!(cpu_percent, None, "Should handle tick overflow safely");
    }

    /// Test the since-start average used for a process's first sample.
    /// A process that used 2000 ticks over its 100s lifetime shows 20%.
    #[test]
    fn test_lifetime_avg_percent() {
        // tps=100, uptime=1000s => uptime_ticks=100_000
        // starttime=90_000 ticks => process age = 10_000 ticks = 100s
        let percent = CpuTracker::lifetime_avg_percent(800, 1200, 90_000, 100, 1000.0);
        assert_eq!(percent, 20.0, "2000 ticks over 100s should be 20%");
    }

    /// Test that a process whose age can't be established yields 0.0
    #[test]
    fn test_lifetime_avg_percent_nonpositive_age_returns_zero() {
        // starttime later than uptime: not possible in practice, guards the math
        assert_eq!(
            CpuTracker::lifetime_avg_percent(100, 100, 200_000, 100, 1000.0),
            0.0
        );
        // uptime reads as 0 (e.g. unreadable /proc/uptime)
        assert_eq!(CpuTracker::lifetime_avg_percent(100, 100, 0, 100, 0.0), 0.0);
    }

    /// Test that overflow of tick sums is handled safely
    #[test]
    fn test_lifetime_avg_percent_overflow_returns_zero() {
        assert_eq!(
            CpuTracker::lifetime_avg_percent(u64::MAX, 1, 90_000, 100, 1000.0),
            0.0
        );
    }

    /// Test that evict_dead_processes removes entries for dead PIDs
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

    /// Test CPU percent calculation with sub-second precision
    #[test]
    fn test_cpu_percent_subsecond_precision() {
        let recent_utime = 150;
        let recent_stime = 150;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1000.5_f64;
        let last_timestamp = 1000.0_f64;

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        assert!(
            (cpu_percent.unwrap() - 200.0).abs() < 0.01,
            "Should handle sub-second precision correctly"
        );
    }
}
