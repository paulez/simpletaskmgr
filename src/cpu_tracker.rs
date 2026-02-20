use anyhow::{Context, Result};
use log::warn;
use procfs::process;
use std::collections::HashMap;

use crate::config::Config;
use crate::process::Process;

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
        processes: &mut HashMap<i32, Process>,
    ) -> Result<()> {
        let all_processes = process::all_processes().context("Failed to read /proc filesystem")?;

        let mut stat_map: HashMap<i32, (u64, u64)> = HashMap::new();

        for proc in all_processes.flatten() {
            match proc.stat() {
                Ok(stat) => {
                    stat_map.insert(proc.pid(), (stat.utime, stat.stime));
                }
                Err(e) => {
                    warn!("Failed to read process stat for PID {}: {}", proc.pid(), e);
                }
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
