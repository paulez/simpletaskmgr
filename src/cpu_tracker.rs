use procfs::process;
use std::collections::HashMap;

use crate::Process;

#[derive(Clone)]
pub struct CpuTracker {
    process_usage: HashMap<i32, UsageStats>,
}

#[derive(Clone)]
pub struct UsageStats {
    pub utime_history: Vec<u64>,
    pub stime_history: Vec<u64>,
    pub last_ticks: (u64, u64),
}

impl Default for UsageStats {
    fn default() -> Self {
        Self {
            utime_history: Vec::new(),
            stime_history: Vec::new(),
            last_ticks: (0, 0),
        }
    }
}

impl CpuTracker {
    pub fn new() -> Self {
        Self {
            process_usage: HashMap::new(),
        }
    }

    pub fn update_process_cpu_usage(&mut self, processes: &mut HashMap<i32, Process>) {
        if let Ok(all_processes) = process::all_processes() {
            let mut stat_map: HashMap<i32, (u64, u64)> = HashMap::new();

            for proc_result in all_processes {
                if let Ok(proc) = proc_result {
                    if let Ok(stat) = proc.stat() {
                        stat_map.insert(proc.pid(), (stat.utime, stat.stime));
                    }
                }
            }

            let pids: Vec<i32> = processes.keys().copied().collect();

            for pid in pids {
                if let Some((utime, stime)) = stat_map.get(&pid) {
                    match self.process_usage.entry(pid) {
                        std::collections::hash_map::Entry::Occupied(mut occ) => {
                            let usage = occ.get_mut();
                            usage.utime_history.push(*utime);
                            usage.stime_history.push(*stime);

                            if usage.utime_history.len() > 5 {
                                usage.utime_history.remove(0);
                                usage.stime_history.remove(0);
                            }

                            if usage.utime_history.len() >= 2 {
                                let recent_utime: u64 =
                                    usage.utime_history.iter().rev().take(2).sum();
                                let recent_stime: u64 =
                                    usage.stime_history.iter().rev().take(2).sum();
                                let history_size = usage.utime_history.len() as f64;

                                let cpu_time_delta = (recent_utime - usage.last_ticks.0) as f64
                                    + (recent_stime - usage.last_ticks.1) as f64;
                                let cpu_percent = (cpu_time_delta / (100.0 * history_size)) * 100.0;

                                if let Some(process) = processes.get_mut(&pid) {
                                    process.cpu_percent = cpu_percent;
                                }
                            }

                            usage.last_ticks = (*utime, *stime);
                        }
                        std::collections::hash_map::Entry::Vacant(vac) => {
                            vac.insert(UsageStats {
                                utime_history: vec![*utime],
                                stime_history: vec![*stime],
                                last_ticks: (*utime, *stime),
                            });
                        }
                    }
                }
            }
        }
    }

    pub fn needs_update(&self) -> bool {
        false
    }
}
