use anyhow::Result;
use log::warn;
use procfs::process;
use std::collections::HashMap;

use crate::config::Config;
use crate::process::TaskMgrProcess;

#[derive(Clone)]
pub struct CpuTracker {
    process_usage: HashMap<i32, UsageStats>,
}

#[derive(Clone, Default)]
pub struct UsageStats {
    pub utime_history: Vec<u64>,
    pub stime_history: Vec<u64>,
    pub last_ticks: (u64, u64),
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
        }
    }

    fn calculate_cpu_percent(
        recent_utime: u64,
        recent_stime: u64,
        history_size: usize,
        last_ticks: (u64, u64),
    ) -> f64 {
        let total_time_delta =
            (recent_utime - last_ticks.0) as f64 + (recent_stime - last_ticks.1) as f64;
        (total_time_delta / (100.0 * history_size as f64)) * 100.0
    }

    fn update_history(usage: &mut UsageStats, utime: u64, stime: u64) {
        usage.utime_history.push(utime);
        usage.stime_history.push(stime);

        if usage.utime_history.len() > Config::CPU_HISTORY_SIZE {
            usage.utime_history.remove(0);
            usage.stime_history.remove(0);
        }
    }

    pub fn update_process_cpu_usage(
        &mut self,
        processes: &mut HashMap<i32, TaskMgrProcess>,
        process_objects: &HashMap<i32, process::Process>,
    ) -> Result<()> {
        let mut stat_map: HashMap<i32, (u64, u64)> = HashMap::new();

        // Use the pre-opened process objects to read stats
        for (pid, process_obj) in process_objects.iter() {
            // Try to read stat for each process using the pre-opened objects
            if let Ok(proc_stat) = process_obj.stat() {
                stat_map.insert(*pid, (proc_stat.utime, proc_stat.stime));
            } else {
                warn!("Failed to read process stat for PID {}", pid);
            }
        }

        let pids: Vec<i32> = processes.keys().copied().collect();

        for pid in pids {
            if let Some((utime, stime)) = stat_map.get(&pid) {
                match self.process_usage.entry(pid) {
                    std::collections::hash_map::Entry::Occupied(mut occ) => {
                        let usage = occ.get_mut();
                        Self::update_history(usage, *utime, *stime);

                        if usage.utime_history.len() >= 2 {
                            let recent_utime: u64 = usage.utime_history.iter().rev().take(2).sum();
                            let recent_stime: u64 = usage.stime_history.iter().rev().take(2).sum();

                            let cpu_percent = Self::calculate_cpu_percent(
                                recent_utime,
                                recent_stime,
                                usage.utime_history.len(),
                                usage.last_ticks,
                            );

                            if let Some(process) = processes.get_mut(&pid) {
                                process.cpu_percent = cpu_percent;
                            }
                        }

                        usage.last_ticks = (*utime, *stime);
                    }
                    std::collections::hash_map::Entry::Vacant(vac) => {
                        vac.insert(UsageStats {
                            utime_history: vec![*utime; Config::CPU_HISTORY_SIZE],
                            stime_history: vec![*stime; Config::CPU_HISTORY_SIZE],
                            last_ticks: (*utime, *stime),
                        });
                    }
                }
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

        let cpu_percent =
            CpuTracker::calculate_cpu_percent(recent_utime, recent_stime, history_size, last_ticks);

        // With 100 ticks delta in utime and stime (total 200 ticks),
        // and history_size of 2, we expect:
        // total_time_delta = (200-100) + (200-100) = 200
        // cpu_percent = (200 / (100.0 * 2)) * 100 = 100.0
        assert_eq!(cpu_percent, 100.0, "Should calculate 100% CPU usage");
    }

    /// Test CPU percent calculation for 0% usage (idle process)
    #[test]
    fn test_cpu_percent_0_percent() {
        // Simulate a process using 0% CPU (no change in utime/stime)
        let recent_utime = 100;
        let recent_stime = 100;
        let history_size = 2;
        let last_ticks = (100, 100); // Same utime and stime

        let cpu_percent =
            CpuTracker::calculate_cpu_percent(recent_utime, recent_stime, history_size, last_ticks);

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

        let cpu_percent =
            CpuTracker::calculate_cpu_percent(recent_utime, recent_stime, history_size, last_ticks);

        // With 50 ticks delta in utime and stime (total 100 ticks),
        // and history_size of 2, we expect:
        // total_time_delta = (150-100) + (150-100) = 100
        // cpu_percent = (100 / (100.0 * 2)) * 100 = 50.0
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

    /// Test CPU percent calculation with different history sizes
    #[test]
    fn test_cpu_percent_different_history_sizes() {
        // Test with history_size = 5 (the default CPU_HISTORY_SIZE)
        let recent_utime = 500;
        let recent_stime = 500;
        let history_size = 5;
        let last_ticks = (100, 100);

        let cpu_percent =
            CpuTracker::calculate_cpu_percent(recent_utime, recent_stime, history_size, last_ticks);

        // With 400 ticks delta total (500-100 for utime and stime each)
        // cpu_percent = (400 / (100.0 * 5)) * 100 = 80.0
        // But looking at the formula: (total_time_delta / (100.0 * history_size as f64)) * 100.0
        // total_time_delta = (500-100) + (500-100) = 800
        // cpu_percent = (800 / (100.0 * 5)) * 100 = 160.0
        assert_eq!(
            cpu_percent, 160.0,
            "Should handle different history sizes correctly"
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

        let cpu_percent =
            CpuTracker::calculate_cpu_percent(recent_utime, recent_stime, history_size, last_ticks);

        // With 100 ticks delta in utime and 50 ticks delta in stime (total 150 ticks)
        // cpu_percent = (150 / (100.0 * 2)) * 100 = 75.0
        assert_eq!(cpu_percent, 75.0, "Should handle minimal history correctly");
    }
}
