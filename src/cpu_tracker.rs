use im::Vector;
use procfs::process;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::Process;

#[derive(Clone)]
struct CpuTime {
    user: f64,
    nice: f64,
    system: f64,
    idle: f64,
}

#[derive(Clone)]
pub struct CpuTracker {
    process_samples: HashMap<i32, Vec<CpuTime>>,
    last_update: Instant,
}

impl CpuTracker {
    pub fn new() -> Self {
        Self {
            process_samples: HashMap::new(),
            last_update: Instant::now(),
        }
    }

    pub fn update(&mut self, processes: &mut Vector<Process>) {
        let now = Instant::now();

        // Get current process information
        let mut current_samples: HashMap<i32, CpuTime> = HashMap::new();

        for process in processes.iter_mut() {
            if let Ok(all_processes) = process::all_processes() {
                for proc in all_processes {
                    if let Ok(p) = proc {
                        if p.pid() == process.pid {
                            if let Ok(stat) = p.stat() {
                                current_samples.insert(
                                    process.pid,
                                    CpuTime {
                                        user: stat.utime as f64,
                                        nice: stat.stime as f64,
                                        system: stat.stime as f64,
                                        idle: 0.0,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Update samples for each process
        for process in processes.iter_mut() {
            if let Some(current_times) = current_samples.get(&process.pid) {
                // Get existing samples
                let samples = self.process_samples.entry(process.pid).or_insert_with(Vec::new);

                // Add new sample (clone to avoid move)
                samples.push(CpuTime {
                    user: current_times.user,
                    nice: current_times.nice,
                    system: current_times.system,
                    idle: current_times.idle,
                });

                // Keep only last 5 samples (5 seconds at 1 second intervals)
                if samples.len() > 5 {
                    samples.remove(0);
                }

                // Compute running average if we have enough samples
                if samples.len() >= 2 {
                    // Calculate delta from oldest to newest
                    let mut total_delta = 0.0;
                    let mut total_time = 0.0;

                    for sample in samples {
                        total_delta += sample.user + sample.nice + sample.system;
                        total_time += sample.user + sample.nice + sample.system + sample.idle;
                    }

                    if total_time > 0.0 {
                        let cpu_percent = (total_delta / total_time) * 100.0;
                        process.cpu_percent = cpu_percent;
                    }
                } else {
                    // Not enough samples yet, keep previous value
                }
            }
        }

        self.last_update = now;
    }

    pub fn needs_update(&self) -> bool {
        self.last_update.elapsed() >= Duration::from_secs(1)
    }
}