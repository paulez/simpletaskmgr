use im::Vector;
use procfs::process;
use std::collections::HashMap;

use crate::Process;

#[derive(Clone)]
pub struct CpuTracker {
    process_cpu_times: HashMap<i32, u64>,
}

impl CpuTracker {
    pub fn new() -> Self {
        Self {
            process_cpu_times: HashMap::new(),
        }
    }

    pub fn update(&mut self, processes: &mut Vector<Process>) {
        for process in processes.iter_mut() {
            if let Ok(all_processes) = process::all_processes() {
                for proc in all_processes {
                    if let Ok(p) = proc {
                        if p.pid() == process.pid {
                            if let Ok(stat) = p.stat() {
                                let current_cpu_time = stat.utime + stat.stime;

                                match self.process_cpu_times.get(&process.pid) {
                                    Some(&previous_cpu_time) => {
                                        let elapsed_sec = 1.0;
                                        let cpu_delta = current_cpu_time
                                            .saturating_sub(previous_cpu_time)
                                            as f64;
                                        let cpu_percent =
                                            (cpu_delta / (100.0 * elapsed_sec)) * 100.0;
                                        process.cpu_percent = cpu_percent;
                                    }
                                    None => {
                                        process.cpu_percent = 0.0;
                                    }
                                }

                                self.process_cpu_times.insert(process.pid, current_cpu_time);
                            }
                            break;
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
