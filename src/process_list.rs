use crate::cpu_tracker::CpuTracker;
use crate::process::{read_process_details, TaskMgrProcess};
use anyhow::{Context, Result};
use floem::prelude::{create_rw_signal, RwSignal, SignalUpdate};
use imbl::Vector;
use log::debug;
use procfs::process;
use std::cell::{LazyCell, RefCell};
use users::{Users, UsersCache};

thread_local! {
    pub static PROCESS_LIST: LazyCell<ProcessList> = LazyCell::new(|| {
        ProcessList::new()
    })
}

pub struct ProcessList {
    pub processes: RwSignal<Vector<TaskMgrProcess>>,
    cpu_tracker: RefCell<CpuTracker>,
    users_cache: UsersCache,
    show_all_processes: bool,
}

impl Default for ProcessList {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessList {
    pub fn new() -> Self {
        let users_cache = UsersCache::new();
        let processes = create_rw_signal(Vector::new());
        let cpu_tracker = RefCell::new(CpuTracker::new());
        Self {
            processes,
            cpu_tracker,
            users_cache,
            show_all_processes: false,
        }
    }

    pub fn init() -> Self {
        let new_list = Self::new();
        new_list.update_process_list();
        new_list
    }

    pub fn update_process_list(&self) {
        match self.refresh_process_list() {
            Ok(processes) => self.processes.set(processes),
            Err(e) => {
                log::error!("Failed to update process list: {}", e);
                self.processes.set(Vector::new());
            }
        }
    }

    pub fn refresh_process_list(&self) -> Result<Vector<TaskMgrProcess>> {
        debug!("Refreshing process list");

        let current_uid = self.users_cache.get_current_uid();

        let all_processes = process::all_processes().context("Can't read /proc filesystem")?;

        // Collect both TaskMgrProcess objects and their Process objects in a single pass
        let mut processes: Vector<TaskMgrProcess> = Vector::new();
        let mut process_objects_map: std::collections::HashMap<i32, process::Process> =
            std::collections::HashMap::new();

        for p in all_processes {
            let proc = match p {
                Ok(p) => p,
                Err(e) => match e {
                    procfs::ProcError::NotFound(_) => continue,
                    procfs::ProcError::Io(_e, _path) => continue,
                    x => {
                        log::warn!("Can't read process due to error: {}", x);
                        continue;
                    }
                },
            };

            // Store process object for CPU tracking (even if not included in final list)
            // We need to create a new Process since we can't clone the iterator item
            // This is still better than what we had before since we're only creating
            // Process objects for PIDs that actually exist
            if let Ok(new_proc) = process::Process::new(proc.pid()) {
                process_objects_map.insert(proc.pid(), new_proc);
            }

            if !self.should_include_process(&proc, current_uid, self.show_all_processes) {
                continue;
            }

            if let Some(tm_process) = read_process_details(&proc, &self.users_cache) {
                processes.push_back(tm_process);
            }
        }

        // Update CPU usage for each process
        let mut process_map: std::collections::HashMap<i32, TaskMgrProcess> = processes
            .iter()
            .map(|p: &TaskMgrProcess| (p.pid, p.clone()))
            .collect();
        self.cpu_tracker
            .borrow_mut()
            .update_process_cpu_usage(&mut process_map, &process_objects_map)
            .context("Failed to update CPU usage")?;

        // Convert back to vector
        let mut processes: Vector<TaskMgrProcess> = process_map.values().cloned().collect();

        // Sort by CPU usage (highest first)
        processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

        Ok(processes)
    }

    fn should_include_process(
        &self,
        proc: &process::Process,
        current_uid: u32,
        show_all: bool,
    ) -> bool {
        if !show_all {
            let uid = proc.uid().expect("Can't get process UID");
            return uid == current_uid;
        }
        true
    }
}
