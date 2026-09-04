use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::HashMap;
use std::time::Instant;

use crate::process::TaskMgrProcess;

/// Tracks per-process disk I/O so that each refresh can report the read and
/// write *speed* (bytes/second) since the last sample.
///
/// The same delta technique as [`CpuTracker`](crate::cpu_tracker::CpuTracker):
/// `/proc/[pid]/io` counters (`read_bytes`, `write_bytes`) are monotonic
/// per process, so a speed is `delta_bytes / delta_time`. A decrease of a
/// counter means the PID was reused by a new process: the stale baseline is
/// discarded and the unknown speed is reported.
#[derive(Clone)]
pub struct IoTracker {
    baselines: HashMap<i32, IoBaseline>,
    start_instant: Instant,
}

#[derive(Clone, Copy)]
struct IoBaseline {
    last_read_bytes: u64,
    last_write_bytes: u64,
    last_timestamp: f64,
}

impl IoBaseline {
    fn at(read: u64, write: u64, ts: f64) -> Self {
        Self {
            last_read_bytes: read,
            last_write_bytes: write,
            last_timestamp: ts,
        }
    }
}

impl Default for IoTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl IoTracker {
    pub fn new() -> Self {
        Self {
            baselines: HashMap::new(),
            start_instant: Instant::now(),
        }
    }

    /// Computes read/write speeds in bytes/second from byte deltas.
    ///
    /// Returns `None` when a counter decreased (PID reuse) or when the time
    /// elapsed since the last sample is non-positive, since neither can be
    /// attributed to a meaningful rate.
    fn calculate_rates(
        recent_read: u64,
        recent_write: u64,
        last: IoBaseline,
        current_timestamp: f64,
    ) -> Option<(f64, f64)> {
        let read_delta = recent_read.checked_sub(last.last_read_bytes)?;
        let write_delta = recent_write.checked_sub(last.last_write_bytes)?;

        let time_elapsed = current_timestamp - last.last_timestamp;
        if time_elapsed <= 0.0 {
            return None;
        }

        Some((
            read_delta as f64 / time_elapsed,
            write_delta as f64 / time_elapsed,
        ))
    }

    /// Updates `process`'s disk read/write speeds using the byte deltas since
    /// the last sample for its PID.
    ///
    /// A first sample (no baseline yet) yields `None` speeds: a rate needs two
    /// samples. A counter decrease (PID reuse) resets the baseline and also
    /// yields `None`.
    pub fn update_process_io(
        &mut self,
        process: &mut TaskMgrProcess,
        read_bytes: u64,
        write_bytes: u64,
    ) {
        let pid = process.pid;
        let current_timestamp = self.start_instant.elapsed().as_secs_f64();

        match self.baselines.entry(pid) {
            Occupied(mut occ) => {
                let last = *occ.get();
                match Self::calculate_rates(read_bytes, write_bytes, last, current_timestamp) {
                    Some((read_speed, write_speed)) => {
                        process.disk_read_speed = Some(read_speed);
                        process.disk_write_speed = Some(write_speed);
                    }
                    None => {
                        log::debug!("PID {} was reused, resetting I/O baseline", pid);
                    }
                }
                *occ.get_mut() = IoBaseline::at(read_bytes, write_bytes, current_timestamp);
            }
            Vacant(vac) => {
                process.disk_read_speed = None;
                process.disk_write_speed = None;
                vac.insert(IoBaseline::at(read_bytes, write_bytes, current_timestamp));
            }
        }
    }

    /// Removes tracking entries for PIDs that no longer exist, so the map
    /// does not grow unboundedly as processes come and go.
    pub fn evict_dead_processes(&mut self, live_pids: impl Fn(i32) -> bool) {
        self.baselines.retain(|pid, _| live_pids(*pid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn proc(pid: i32) -> TaskMgrProcess {
        crate::testutil::test_process(pid, 0.0)
    }

    fn baseline(read: u64, write: u64, ts: f64) -> IoBaseline {
        IoBaseline {
            last_read_bytes: read,
            last_write_bytes: write,
            last_timestamp: ts,
        }
    }

    /// A steady stream of 1024 read / 1536 write bytes per second.
    #[test]
    fn test_calculate_rates_steady_stream() {
        let rates = IoTracker::calculate_rates(2048, 3072, baseline(0, 0, 1000.0), 1002.0);
        assert_eq!(rates, Some((1024.0, 1536.0)));
    }

    /// No I/O between samples: both speeds zero, not unknown.
    #[test]
    fn test_calculate_rates_idle_process() {
        let rates = IoTracker::calculate_rates(100, 200, baseline(100, 200, 1000.0), 1001.0);
        assert_eq!(rates, Some((0.0, 0.0)));
    }

    /// A counter that decreased between samples (PID reuse) is detected, not
    /// underflowed — regardless of whether it is the read or the write side.
    #[rstest]
    #[case::read_decreased(5, 10)]
    #[case::write_decreased(999, 10)]
    fn test_calculate_rates_pid_reuse_returns_none(#[case] new_read: u64, #[case] new_write: u64) {
        let rates =
            IoTracker::calculate_rates(new_read, new_write, baseline(100, 200, 1000.0), 1001.0);
        assert_eq!(rates, None);
    }

    /// A non-positive time delta cannot yield a rate.
    #[test]
    fn test_calculate_rates_zero_time_delta_returns_none() {
        let last = baseline(0, 0, 1000.0);
        assert_eq!(IoTracker::calculate_rates(4096, 4096, last, 1000.0), None);
        assert_eq!(IoTracker::calculate_rates(4096, 4096, last, 999.0), None);
    }

    /// A newly-seen PID records a baseline and reports unknown speeds: a rate
    /// needs two samples.
    #[test]
    fn test_update_first_sample_is_unknown() {
        let mut tracker = IoTracker::new();
        let mut p = proc(100);
        tracker.update_process_io(&mut p, 4096, 8192);
        assert!(p.disk_read_speed.is_none());
        assert!(p.disk_write_speed.is_none());
        assert_eq!(tracker.baselines.len(), 1);
    }

    /// Every sample refreshes the baseline, so a later delta is measured from
    /// the most recent sample: a third sample with no further I/O reports
    /// exactly zero rates, and the observed speed ratio matches the byte-delta
    /// ratio (6000 read vs 4000 write = 1.5x) independent of the elapsed time.
    #[test]
    fn test_update_baseline_refreshes_between_samples() {
        let mut tracker = IoTracker::new();
        let mut p = proc(100);

        tracker.update_process_io(&mut p, 5000, 5000);
        std::thread::sleep(std::time::Duration::from_millis(50));
        // Second sample: delta 6000 read / 4000 write since the first.
        tracker.update_process_io(&mut p, 11000, 9000);

        assert!(
            p.disk_read_speed.is_some(),
            "second sample must report a rate"
        );
        assert!(p.disk_write_speed.is_some());
        let read_speed = p.disk_read_speed.unwrap();
        let write_speed = p.disk_write_speed.unwrap();
        assert!(read_speed > 0.0 && write_speed > 0.0);
        assert!(
            (read_speed / write_speed - 1.5).abs() < 0.01,
            "speed ratio must equal the byte-delta ratio"
        );

        std::thread::sleep(std::time::Duration::from_millis(50));
        // Third sample with no further I/O: exactly zero rates.
        tracker.update_process_io(&mut p, 11000, 9000);
        assert_eq!(p.disk_read_speed, Some(0.0));
        assert_eq!(p.disk_write_speed, Some(0.0));
    }

    /// PIDs that are no longer present are dropped from the baselines.
    #[test]
    fn test_evict_dead_processes() {
        let mut tracker = IoTracker::new();
        tracker.baselines.insert(100, baseline(0, 0, 0.0));
        tracker.baselines.insert(200, baseline(0, 0, 0.0));

        tracker.evict_dead_processes(|pid| pid == 100);

        assert_eq!(tracker.baselines.len(), 1);
        assert!(tracker.baselines.contains_key(&100));
        assert!(!tracker.baselines.contains_key(&200));
    }
}
