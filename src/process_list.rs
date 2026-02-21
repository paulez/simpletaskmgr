use crate::cpu_tracker::CpuTracker;
use crate::process::{read_process_details, TaskMgrProcess};
use anyhow::{Context, Result};
use floem::prelude::{create_rw_signal, RwSignal, SignalUpdate};
use imbl::Vector;
use log::{debug, warn};
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

        let all_processes_iter = process::all_processes().context("Can't read /proc filesystem")?;
        let all_processes: Vec<process::Process> =
            all_processes_iter.filter_map(|p| p.ok()).collect();

        let mut task_mgr_process_list: Vector<TaskMgrProcess> = all_processes
            .iter()
            .filter_map(
                |process| match read_process_details(process, &self.users_cache) {
                    Some(mut task_mgr_process) => {
                        let _ = self
                            .cpu_tracker
                            .borrow_mut()
                            .update_process_cpu_usage_for_process(&mut task_mgr_process, process);
                        Some(task_mgr_process)
                    }
                    None => {
                        warn!("Cannot convert {:?} to TaskMgrProcess", process);
                        None
                    }
                },
            )
            .filter(|p| self.should_include_process(p, current_uid, self.show_all_processes))
            .collect();

        // Sort by CPU usage (highest first)
        task_mgr_process_list.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

        Ok(task_mgr_process_list)
    }

    fn should_include_process(
        &self,
        proc: &TaskMgrProcess,
        current_uid: u32,
        show_all: bool,
    ) -> bool {
        if !show_all {
            return proc.ruid == current_uid;
        }
        true
    }
}
