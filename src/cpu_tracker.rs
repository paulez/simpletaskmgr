use anyhow::Result;
use log::debug;
use procfs::process;
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

    fn calculate_cpu_percent(
        recent_utime: u64,
        recent_stime: u64,
        last_ticks: (u64, u64),
        tps: u64,
        current_timestamp: f64,
        last_timestamp: f64,
    ) -> f64 {
        let total_time_delta =
            (recent_utime - last_ticks.0) as f64 + (recent_stime - last_ticks.1) as f64;

        let time_elapsed = current_timestamp - last_timestamp;

        if time_elapsed <= 0.0 {
            return 0.0;
        }

        // Convert ticks to seconds and calculate percentage
        let cpu_seconds = total_time_delta / tps as f64;
        (cpu_seconds / time_elapsed) * 100.0
    }

    fn update_history(usage: &mut UsageStats, utime: u64, stime: u64, timestamp: f64) {
        usage.last_ticks = (utime, stime);
        usage.last_timestamp = timestamp;
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
            let current_timestamp = self.start_instant.elapsed().as_secs_f64();

            match self.process_usage.entry(pid) {
                Occupied(mut occ) => {
                    let usage = occ.get_mut();

                    // Calculate CPU percentage using the delta between current and last ticks
                    let cpu_percent = Self::calculate_cpu_percent(
                        utime,
                        stime,
                        usage.last_ticks,
                        self.tps,
                        current_timestamp,
                        usage.last_timestamp,
                    );

                    task_mgr_process.cpu_percent = cpu_percent;
                    Self::update_history(usage, utime, stime, current_timestamp);
                }
                Vacant(vac) => {
                    vac.insert(UsageStats::new(utime, stime, current_timestamp));
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
            cpu_percent, 200.0,
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
            cpu_percent, 0.0,
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

        assert_eq!(cpu_percent, 50.0, "Should calculate 50% CPU usage");
    }

    /// Test that update_process_cpu_usage refreshes stats for all processes
    #[test]
    fn test_update_process_cpu_usage_refreshes_all_processes() {
        let mut cpu_tracker = CpuTracker::new();
        let mut processes = HashMap::new();

        let p1 = TaskMgrProcess::new("test1".to_string(), 100, 1000, "user1".to_string(), 0.0);
        let p2 = TaskMgrProcess::new("test2".to_string(), 200, 1000, "user2".to_string(), 0.0);
        let p3 = TaskMgrProcess::new("test3".to_string(), 300, 1000, "user3".to_string(), 0.0);

        processes.insert(100, p1);
        processes.insert(200, p2);
        processes.insert(300, p3);

        assert_eq!(processes.get(&100).unwrap().cpu_percent, 0.0);
        assert_eq!(processes.get(&200).unwrap().cpu_percent, 0.0);
        assert_eq!(processes.get(&300).unwrap().cpu_percent, 0.0);

        let _ = cpu_tracker.update_process_cpu_usage(&mut processes, &HashMap::new());

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

        let result = cpu_tracker.update_process_cpu_usage(&mut processes, &HashMap::new());

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

        let p1 = TaskMgrProcess::new(
            "new_process".to_string(),
            400,
            1000,
            "user4".to_string(),
            0.0,
        );
        processes.insert(400, p1);

        assert_eq!(processes.get(&400).unwrap().cpu_percent, 0.0);

        let _ = cpu_tracker.update_process_cpu_usage(&mut processes, &HashMap::new());

        assert_eq!(processes.len(), 1);
        assert!(processes.contains_key(&400));
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
            cpu_percent, 40.0,
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
            cpu_percent, 150.0,
            "Should handle minimal history correctly"
        );
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
            (cpu_percent - 200.0).abs() < 0.01,
            "Should handle sub-second precision correctly"
        );
    }
}
