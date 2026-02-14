use im::Vector;
use procfs::process;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::Process;

#[derive(Clone)]
struct CpuTime {
    user: f64,
    system: f64,
    idle: f64,
}

#[derive(Clone)]
pub struct CpuTracker {
    process_times: HashMap<i32, (CpuTime, Instant)>,
    last_update: Instant,
}

impl CpuTracker {
    pub fn new() -> Self {
        Self {
            process_times: HashMap::new(),
            last_update: Instant::now(),
        }
    }

    pub fn update(&mut self, processes: &mut Vector<Process>) {
        let now = Instant::now();

        for process in processes.iter_mut() {
            if let Ok(all_processes) = process::all_processes() {
                for proc in all_processes {
                    if let Ok(p) = proc {
                        if p.pid() == process.pid {
                            if let Ok(stat) = p.stat() {
                                let current_time = CpuTime {
                                    user: stat.utime as f64 * 0.01,
                                    system: stat.stime as f64 * 0.01,
                                    idle: stat.cutime as f64 * 0.01,
                                };

                                if let Some((prev_time, prev_time_of)) =
                                    self.process_times.get(&process.pid)
                                {
                                    let elapsed = now.duration_since(*prev_time_of).as_secs_f64();

                                    if elapsed > 0.0 {
                                        let total_time = current_time.user
                                            + current_time.system
                                            + current_time.idle;
                                        let total_prev =
                                            prev_time.user + prev_time.system + prev_time.idle;

                                        if total_prev > 0.0 && total_time > total_prev {
                                            let busy_time = current_time.user + current_time.system;
                                            let busy_prev = prev_time.user + prev_time.system;
                                            let busy_delta = busy_time - busy_prev;
                                            let time_delta = total_time - total_prev;

                                            if time_delta > 0.0 {
                                                process.cpu_percent =
                                                    (busy_delta / time_delta) * 100.0;
                                            }
                                        }
                                    }

                                    *self.process_times.get_mut(&process.pid).unwrap() =
                                        (current_time, now);
                                } else {
                                    self.process_times.insert(process.pid, (current_time, now));
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

        self.last_update = now;
    }

    pub fn needs_update(&self) -> bool {
        self.last_update.elapsed() >= Duration::from_secs(1)
    }
}
