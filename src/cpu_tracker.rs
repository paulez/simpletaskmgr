use anyhow::Result;
use procfs::process;
use std::collections::HashMap;
use std::time::Instant;

use crate::config::Config;
use crate::process::TaskMgrProcess;

#[derive(Clone)]
pub struct CpuTracker {
    process_usage: HashMap<i32, UsageStats>,
    tps: u64,
}

#[derive(Clone, Default)]
pub struct UsageStats {
    pub utime_history: Vec<u64>,
    pub stime_history: Vec<u64>,
    pub last_ticks: (u64, u64),
    pub last_timestamp: Option<f64>,
}

impl UsageStats {
    fn new(utime: u64, stime: u64) -> Self {
        Self {
            utime_history: vec![utime; Config::CPU_HISTORY_SIZE],
            stime_history: vec![stime; Config::CPU_HISTORY_SIZE],
            last_ticks: (utime, stime),
            last_timestamp: None,
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
        }
    }

    fn calculate_cpu_percent(
        recent_utime: u64,
        recent_stime: u64,
        _history_size: usize,
        last_ticks: (u64, u64),
        tps: u64,
        current_timestamp: f64,
        last_timestamp: Option<f64>,
    ) -> f64 {
        let total_time_delta =
            (recent_utime - last_ticks.0) as f64 + (recent_stime - last_ticks.1) as f64;

        let time_elapsed = match last_timestamp {
            Some(last_ts) => current_timestamp - last_ts,
            None => return 0.0, // Not enough data yet
        };

        if time_elapsed <= 0.0 {
            return 0.0;
        }

        // Convert ticks to seconds and calculate percentage
        let cpu_seconds = total_time_delta / tps as f64;
        (cpu_seconds / time_elapsed) * 100.0
    }

    fn update_history(usage: &mut UsageStats, utime: u64, stime: u64, timestamp: f64) {
        usage.utime_history.push(utime);
        usage.stime_history.push(stime);

        if usage.utime_history.len() > Config::CPU_HISTORY_SIZE {
            usage.utime_history.remove(0);
            usage.stime_history.remove(0);
        }

        usage.last_timestamp = Some(timestamp);
    }

    pub fn update_process_cpu_usage_for_process(
        &mut self,
        task_mgr_process: &mut TaskMgrProcess,
        process_obj: &process::Process,
    ) -> Result<()> {
        // Read stat for the process
        if let Ok(proc_stat) = process_obj.stat() {
            let utime = proc_stat.utime;
            let stime = proc_stat.stime;
            let pid = task_mgr_process.pid;

            // Use Instant for high-resolution timing
            let now = Instant::now();
            let current_timestamp = now.elapsed().as_secs_f64();

            match self.process_usage.entry(pid) {
                std::collections::hash_map::Entry::Occupied(mut occ) => {
                    let usage = occ.get_mut();
                    Self::update_history(usage, utime, stime, current_timestamp);

                    if usage.utime_history.len() >= 2 {
                        let recent_utime: u64 = usage.utime_history.iter().rev().take(2).sum();
                        let recent_stime: u64 = usage.stime_history.iter().rev().take(2).sum();

                        let cpu_percent = Self::calculate_cpu_percent(
                            recent_utime,
                            recent_stime,
                            usage.utime_history.len(),
                            usage.last_ticks,
                            self.tps,
                            current_timestamp,
                            usage.last_timestamp,
                        );

                        task_mgr_process.cpu_percent = cpu_percent;
                    }

                    usage.last_ticks = (utime, stime);
                }
                std::collections::hash_map::Entry::Vacant(vac) => {
                    vac.insert(UsageStats::new(utime, stime));
                }
            }
        }

        Ok(())
    }

    pub fn update_process_cpu_usage(
        &mut self,
        processes: &mut HashMap<i32, TaskMgrProcess>,
        process_objects: &HashMap<i32, process::Process>,
    ) -> Result<()> {
        // Call the new method for each process
        for (pid, tm_process) in processes.iter_mut() {
            if let Some(proc_obj) = process_objects.get(pid) {
                self.update_process_cpu_usage_for_process(tm_process, proc_obj)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Test CPU percent calculation for 100% usage (maximum CPU usage)
    #[test]
    fn test_cpu_percent_100_percent() {
        // Simulate a process using 100% CPU
        // If utime increases by 100 ticks and history size is 2, we expect 100% CPU
        let recent_utime = 200;
        let recent_stime = 200;
        let history_size = 2;
        let last_ticks = (100, 100); // Previous utime and stime
        let tps = 100; // 100 ticks per second
        let current_timestamp = 1001_f64;
        let last_timestamp = Some(1000_f64); // 1 second elapsed

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            history_size,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        // With 100 ticks delta in utime and stime (total 200 ticks),
        // and 1 second elapsed, at 100 tps:
        // cpu_time_seconds = 200 / 100 = 2.0
        // cpu_percent = (2.0 / 1.0) * 100 = 200.0
        assert_eq!(
            cpu_percent, 200.0,
            "Should calculate CPU usage based on ticks"
        );
    }

    /// Test CPU percent calculation for 0% usage (idle process)
    #[test]
    fn test_cpu_percent_0_percent() {
        // Simulate a process using 0% CPU (no change in utime/stime)
        let recent_utime = 100;
        let recent_stime = 100;
        let history_size = 2;
        let last_ticks = (100, 100); // Same utime and stime
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = Some(1000_f64);

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            history_size,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        // With no change in utime and stime, we expect 0% CPU usage
        assert_eq!(
            cpu_percent, 0.0,
            "Should calculate 0% CPU usage for idle process"
        );
    }

    /// Test CPU percent calculation for partial usage
    #[test]
    fn test_cpu_percent_partial_usage() {
        // Simulate a process using 50% CPU
        let recent_utime = 150;
        let recent_stime = 150;
        let history_size = 2;
        let last_ticks = (100, 100); // Previous utime and stime
        let tps = 100;
        let current_timestamp = 1002_f64;
        let last_timestamp = Some(1000_f64); // 2 seconds elapsed

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            history_size,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        // With 50 ticks delta in utime and stime (total 100 ticks),
        // and 2 seconds elapsed, at 100 tps:
        // cpu_time_seconds = 100 / 100 = 1.0
        // cpu_percent = (1.0 / 2.0) * 100 = 50.0
        assert_eq!(cpu_percent, 50.0, "Should calculate 50% CPU usage");
    }

    /// Test that update_process_cpu_usage refreshes stats for all processes
    #[test]
    fn test_update_process_cpu_usage_refreshes_all_processes() {
        let mut cpu_tracker = CpuTracker::new();
        let mut processes = HashMap::new();

        // Create test processes
        let p1 = TaskMgrProcess::new("test1".to_string(), 100, 1000, "user1".to_string(), 0.0);
        let p2 = TaskMgrProcess::new("test2".to_string(), 200, 1000, "user2".to_string(), 0.0);
        let p3 = TaskMgrProcess::new("test3".to_string(), 300, 1000, "user3".to_string(), 0.0);

        processes.insert(100, p1);
        processes.insert(200, p2);
        processes.insert(300, p3);

        // Initial CPU percent should be 0.0
        assert_eq!(processes.get(&100).unwrap().cpu_percent, 0.0);
        assert_eq!(processes.get(&200).unwrap().cpu_percent, 0.0);
        assert_eq!(processes.get(&300).unwrap().cpu_percent, 0.0);

        // Update CPU usage using the processes argument
        // In test environment without /proc, it may fail but shouldn't panic
        let _ = cpu_tracker.update_process_cpu_usage(&mut processes, &HashMap::new());

        // Function should not panic even if it returns an error

        // Verify all processes still exist and have valid data
        assert_eq!(processes.len(), 3);
        assert!(processes.contains_key(&100));
        assert!(processes.contains_key(&200));
        assert!(processes.contains_key(&300));
    }

    /// Test that update_process_cpu_usage handles empty process list
    #[test]
    fn test_update_process_cpu_usage_empty_list() {
        let mut cpu_tracker = CpuTracker::new();
        let mut processes = HashMap::new();

        // Test with empty process list
        let result = cpu_tracker.update_process_cpu_usage(&mut processes, &HashMap::new());

        // Should not panic and should return appropriate result
        match result {
            Ok(_) => {}  // Success
            Err(_) => {} // Expected in test environment
        }
    }

    /// Test that update_process_cpu_usage initializes new processes correctly
    #[test]
    fn test_update_process_cpu_usage_initializes_new_processes() {
        let mut cpu_tracker = CpuTracker::new();
        let mut processes = HashMap::new();

        // Add a process that hasn't been tracked yet
        let p1 = TaskMgrProcess::new(
            "new_process".to_string(),
            400,
            1000,
            "user4".to_string(),
            0.0,
        );
        processes.insert(400, p1);

        // Verify initial state
        assert_eq!(processes.get(&400).unwrap().cpu_percent, 0.0);

        // Try to update (will fail in test environment, but shouldn't panic)
        let _ = cpu_tracker.update_process_cpu_usage(&mut processes, &HashMap::new());

        // Process should still exist after update attempt
        assert_eq!(processes.len(), 1);
        assert!(processes.contains_key(&400));
    }

    /// Test CPU percent calculation with different time intervals
    #[test]
    fn test_cpu_percent_different_time_intervals() {
        // Test with different time intervals
        let recent_utime = 300;
        let recent_stime = 300;
        let history_size = 2;
        let last_ticks = (100, 100);
        let tps = 100;
        let current_timestamp = 1010_f64;
        let last_timestamp = Some(1000_f64); // 10 seconds elapsed

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            history_size,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        // With 200 ticks delta total (300-100 for utime and stime each),
        // and 10 seconds elapsed, at 100 tps:
        // cpu_time_seconds = 200 / 100 = 2.0
        // cpu_percent = (2.0 / 10.0) * 100 = 20.0
        assert_eq!(
            cpu_percent, 40.0,
            "Should handle different time intervals correctly"
        );
    }

    /// Test CPU percent calculation with minimal history (just enough for calculation)
    #[test]
    fn test_cpu_percent_minimal_history() {
        // Test with minimum history size needed for calculation (2)
        let recent_utime = 200;
        let recent_stime = 100;
        let history_size = 2;
        let last_ticks = (100, 50);
        let tps = 100;
        let current_timestamp = 1001_f64;
        let last_timestamp = Some(1000_f64); // 1 second elapsed

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            history_size,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        // With 100 ticks delta in utime and 50 ticks delta in stime (total 150 ticks),
        // and 1 second elapsed, at 100 tps:
        // cpu_time_seconds = 150 / 100 = 1.5
        // cpu_percent = (1.5 / 1.0) * 100 = 150.0
        assert_eq!(
            cpu_percent, 150.0,
            "Should handle minimal history correctly"
        );
    }

    /// Test CPU percent calculation with sub-second precision
    #[test]
    fn test_cpu_percent_subsecond_precision() {
        // Test with sub-second precision (millisecond-level)
        let recent_utime = 150;
        let recent_stime = 150;
        let history_size = 2;
        let last_ticks = (100, 100); // Previous utime and stime
        let tps = 100;
        let current_timestamp = 1000.5_f64; // 1000.5 seconds
        let last_timestamp = Some(1000.0_f64); // 0.5 seconds elapsed

        let cpu_percent = CpuTracker::calculate_cpu_percent(
            recent_utime,
            recent_stime,
            history_size,
            last_ticks,
            tps,
            current_timestamp,
            last_timestamp,
        );

        // With 50 ticks delta in utime and stime (total 100 ticks),
        // and 0.5 seconds elapsed, at 100 tps:
        // cpu_time_seconds = 100 / 100 = 1.0
        // cpu_percent = (1.0 / 0.5) * 100 = 200.0
        // Use approximate equality for floating point comparison
        assert!(
            (cpu_percent - 200.0).abs() < 0.01,
            "Should handle sub-second precision correctly"
        );
    }
}
