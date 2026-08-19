use crate::cpu_tracker::CpuTracker;
use crate::process::{build_task_mgr_process, TaskMgrProcess};
use anyhow::{Context, Result};
use floem::prelude::SignalGet;
use floem::prelude::{create_rw_signal, RwSignal, SignalUpdate};
use imbl::Vector;
use log::{debug, warn};
use procfs::process;
use std::cell::RefCell;
use users::{Users, UsersCache};

pub struct ProcessList {
    pub processes: RwSignal<Vector<TaskMgrProcess>>,
    cpu_tracker: RefCell<CpuTracker>,
    users_cache: UsersCache,
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
        debug!("Current UID: {}", current_uid);

        let all_processes_iter = process::all_processes().context("Can't read /proc filesystem")?;
        let all_processes: Vec<process::Process> =
            all_processes_iter.filter_map(|p| p.ok()).collect();
        debug!("Retrieved {} processes from /proc", all_processes.len());

        // Each /proc file is read once per process per refresh.
        let task_mgr_process_list: Vector<TaskMgrProcess> = all_processes
            .iter()
            .filter_map(|proc| {
                let stat = match proc.stat() {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Can't read stat for pid {}: {e:?}", proc.pid());
                        return None;
                    }
                };
                let ruid = match proc.uid() {
                    Ok(u) => u,
                    Err(e) => {
                        warn!("Can't read UID for pid {}: {e:?}", proc.pid());
                        return None;
                    }
                };
                let username = match self.users_cache.get_user_by_uid(ruid) {
                    Some(user) => user.name().to_string_lossy().into_owned(),
                    None => "unknown".to_string(),
                };
                let mut task_mgr_process = build_task_mgr_process(&stat, ruid, username);
                self.cpu_tracker
                    .borrow_mut()
                    .update_process_cpu(&mut task_mgr_process, &stat);
                Some(task_mgr_process)
            })
            .collect();

        // Drop tracking entries for PIDs that no longer exist
        let live_pids: std::collections::HashSet<i32> =
            task_mgr_process_list.iter().map(|p| p.pid).collect();
        self.cpu_tracker
            .borrow_mut()
            .evict_dead_processes(|pid| live_pids.contains(&pid));

        // Capture intermediate count before UID filtering
        let processes_before_filter = task_mgr_process_list.len();
        debug!(
            "After converting to TaskMgrProcess: {} processes",
            processes_before_filter
        );

        // Only the current user's processes are shown (the "show all" toggle is not yet implemented).
        let task_mgr_process_list_filtered: Vector<TaskMgrProcess> = task_mgr_process_list
            .into_iter()
            .filter(|p| p.ruid == current_uid)
            .collect();

        let filtered_count = task_mgr_process_list_filtered.len();
        debug!("After filtering by UID: {} processes", filtered_count);
        if filtered_count == 0 {
            warn!(
                "No processes match current UID {} out of {} processes",
                current_uid, processes_before_filter
            );
        }

        Ok(task_mgr_process_list_filtered)
    }

    /// Sorts the process list by the specified column and direction
    pub fn sort_processes(&self, column: crate::SortColumn, direction: crate::SortDirection) {
        let mut processes = self.processes.get();
        // `i32`/`String` have a total order via `cmp`; `f64` uses `total_cmp` so values
        // (including NaN) sort without panicking, unlike `partial_cmp().unwrap()`.
        match (column, direction) {
            (crate::SortColumn::Pid, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| a.pid.cmp(&b.pid));
            }
            (crate::SortColumn::Pid, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| b.pid.cmp(&a.pid));
            }
            (crate::SortColumn::Username, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| a.username.cmp(&b.username));
            }
            (crate::SortColumn::Username, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| b.username.cmp(&a.username));
            }
            (crate::SortColumn::CpuPercent, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| a.cpu_percent.total_cmp(&b.cpu_percent));
            }
            (crate::SortColumn::CpuPercent, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
            }
            (crate::SortColumn::Name, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| a.name.cmp(&b.name));
            }
            (crate::SortColumn::Name, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| b.name.cmp(&a.name));
            }
        }
        self.processes.set(processes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::TaskMgrProcess;

    /// A `f64` NaN must not panic the sort (regression for `partial_cmp().unwrap()`).
    #[test]
    fn test_sort_by_cpu_percent_with_nan_does_not_panic() {
        let list = ProcessList::new();
        let mut vec = imbl::Vector::new();
        vec.push_back(TaskMgrProcess::new(
            "a".to_string(),
            1,
            1,
            "u".to_string(),
            f64::NAN,
        ));
        vec.push_back(TaskMgrProcess::new(
            "b".to_string(),
            2,
            1,
            "u".to_string(),
            5.0,
        ));
        vec.push_back(TaskMgrProcess::new(
            "c".to_string(),
            3,
            1,
            "u".to_string(),
            -1.0,
        ));
        list.processes.set(vec);

        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Ascending,
        );
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );

        // Sorting must leave the list intact with the same set of PIDs.
        let pids: Vec<i32> = list.processes.get().iter().map(|p| p.pid).collect();
        assert_eq!(pids.len(), 3);
    }
}
