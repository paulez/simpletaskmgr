use crate::cpu_tracker::CpuTracker;
use crate::process::{build_task_mgr_process, ProcessItem, TaskMgrProcess};
use anyhow::{Context, Result};
use floem::prelude::{create_rw_signal, RwSignal, SignalGet, SignalUpdate};
use imbl::Vector;
use log::{debug, warn};
use procfs::process;
use std::cell::RefCell;
use users::{Users, UsersCache};

pub struct ProcessList {
    pub processes: RwSignal<Vector<ProcessItem>>,
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
            Ok(processes) => {
                // Reuse the existing row (and its `value` signal) for any pid that is
                // still present so an already-rendered row updates in place; build a
                // fresh row only for a pid that appeared this refresh. Dropped pids
                // simply fall out of the new vector (their signals are then disposed).
                //
                // Read untracked: `update_process_list` may be called from inside a
                // reactive effect, and reading the list here must not subscribe that
                // effect to the `processes` signal (which is written just below).
                let mut existing: std::collections::HashMap<i32, ProcessItem> = self
                    .processes
                    .get_untracked()
                    .into_iter()
                    .map(|item| (item.pid, item))
                    .collect();
                let new_items: Vector<ProcessItem> = processes
                    .into_iter()
                    .map(|p| match existing.remove(&p.pid) {
                        Some(item) => {
                            item.copy_from(&p);
                            item
                        }
                        None => ProcessItem::new(&p),
                    })
                    .collect();
                // Publish only when the row set actually changed; floem_reactive
                // runs subscriber effects unconditionally on every set, so
                // skipping a no-op write avoids needless effect runs.
                let current_pids: Vec<i32> = self
                    .processes
                    .get_untracked()
                    .iter()
                    .map(|i| i.pid)
                    .collect();
                let new_pids: Vec<i32> = new_items.iter().map(|i| i.pid).collect();
                if current_pids != new_pids {
                    self.processes.set(new_items);
                }
            }
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
        let mut all_processes = Vec::new();
        let mut failed = 0u32;
        let mut last_err = None;
        for result in all_processes_iter {
            match result {
                Ok(proc) => all_processes.push(proc),
                Err(e) => {
                    failed += 1;
                    last_err = Some(e);
                }
            }
        }
        if failed > 0 {
            warn!(
                "Failed to enumerate {} of the listed processes; last error: {last_err:?}",
                failed
            );
        }
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
        // Read the list untracked: this is an imperative operation and it must
        // not subscribe the caller's effect to the `processes` signal. Tracking
        // here would make the sort effect re-subscribe to the very signal it
        // writes at the end, so each `set` synchronously re-runs the effect,
        // which sorts and sets again, recursing until the stack overflows
        // (floem_reactive runs subscriber effects eagerly on every set, with no
        // equality check).
        let mut processes = self.processes.get_untracked();
        // Each row's fields live in its `value` signal; read the snapshot
        // untracked for the comparison (we only reorder, never mutate values).
        // `f64` uses `total_cmp` so NaN values sort without panicking, unlike
        // `partial_cmp().unwrap()`.
        match (column, direction) {
            (crate::SortColumn::Pid, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| a.pid.cmp(&b.pid));
            }
            (crate::SortColumn::Pid, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| b.pid.cmp(&a.pid));
            }
            (crate::SortColumn::Username, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| {
                    a.value_untracked()
                        .username
                        .cmp(&b.value_untracked().username)
                });
            }
            (crate::SortColumn::Username, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| {
                    b.value_untracked()
                        .username
                        .cmp(&a.value_untracked().username)
                });
            }
            (crate::SortColumn::CpuPercent, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| {
                    a.value_untracked()
                        .cpu_percent
                        .total_cmp(&b.value_untracked().cpu_percent)
                });
            }
            (crate::SortColumn::CpuPercent, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| {
                    b.value_untracked()
                        .cpu_percent
                        .total_cmp(&a.value_untracked().cpu_percent)
                });
            }
            (crate::SortColumn::Name, crate::SortDirection::Ascending) => {
                processes.sort_by(|a, b| a.value_untracked().name.cmp(&b.value_untracked().name));
            }
            (crate::SortColumn::Name, crate::SortDirection::Descending) => {
                processes.sort_by(|a, b| b.value_untracked().name.cmp(&a.value_untracked().name));
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
        let items = [(1, f64::NAN), (2, 5.0), (3, -1.0)]
            .into_iter()
            .map(|(pid, cpu)| {
                let p = TaskMgrProcess::new(format!("name{}", pid), pid, 1, "u".to_string(), cpu);
                ProcessItem::new(&p)
            })
            .collect();
        list.processes.set(items);

        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Ascending,
        );
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );

        // Sorting must leave the list intact with the same set of PIDs.
        let mut pids: Vec<i32> = list.processes.get().iter().map(|p| p.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![1, 2, 3]);
    }

    /// Re-sorting while subscribed to the list from a reactive effect must not
    /// re-subscribe that effect to the list it writes. floem_reactive runs
    /// subscriber effects eagerly (no equality check) on every set, so if
    /// `sort_processes` tracked the list it read, each `processes.set` would
    /// synchronously re-run the effect, which sorted and set again, recursing
    /// until the stack overflowed (the startup crash).
    #[test]
    fn test_sort_processes_in_effect_does_not_recurse() {
        use floem::prelude::{SignalGet, SignalTrack, SignalUpdate};
        use floem::reactive::create_effect;

        fn make_items(pid: i32) -> ProcessItem {
            let p =
                TaskMgrProcess::new(format!("name{}", pid), pid, 1, "u".to_string(), pid as f64);
            ProcessItem::new(&p)
        }

        let items: Vector<ProcessItem> = (6..=10).map(make_items).collect();

        let list = std::rc::Rc::new(ProcessList::new());
        list.processes.set(items.clone());

        let sort_column = create_rw_signal(crate::SortColumn::CpuPercent);
        let sort_direction = create_rw_signal(crate::SortDirection::Descending);

        let effect_list = std::rc::Rc::clone(&list);
        create_effect(move |_| {
            sort_column.track();
            sort_direction.track();
            effect_list.sort_processes(sort_column.get(), sort_direction.get());
        });

        // A refresh of the process list must not re-trigger the sort effect
        // (before the fix this recursed until the stack overflowed).
        list.processes.set(items.clone());

        // The sort effect still reacts to sort changes and applies them.
        sort_column.set(crate::SortColumn::Pid);
        sort_column.set(crate::SortColumn::CpuPercent);

        // Then the sort effect still reacts to a sort change and applies it.
        sort_column.set(crate::SortColumn::Pid);
        sort_column.set(crate::SortColumn::CpuPercent);

        let pids: Vec<i32> = list.processes.get().iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![10, 9, 8, 7, 6]);
    }
}
