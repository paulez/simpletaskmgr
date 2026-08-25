use crate::cpu_tracker::CpuTracker;
use crate::io_tracker::IoTracker;
use crate::process::{build_task_mgr_process, ProcessItem, TaskMgrProcess};
use anyhow::{Context, Result};
use log::{debug, warn};
use procfs::process;
use users::{Users, UsersCache};

pub struct ProcessList {
    pub processes: Vec<ProcessItem>,
    /// When `true`, the list shows every process on the system; when `false`
    /// it is limited to the current user's processes.
    pub show_all: bool,
    cpu_tracker: CpuTracker,
    io_tracker: IoTracker,
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
        let processes = Vec::new();
        let cpu_tracker = CpuTracker::new();
        let io_tracker = IoTracker::new();
        Self {
            processes,
            show_all: false,
            cpu_tracker,
            io_tracker,
            users_cache,
        }
    }

    /// Sets the filter: `true` shows every process on the system, `false`
    /// limits the list to the current user's processes.
    pub fn set_show_all(&mut self, show_all: bool) {
        self.show_all = show_all;
    }

    pub fn init() -> Self {
        let mut new_list = Self::new();
        new_list.update_process_list();
        // Default to the same sort the UI starts with (CPU%, descending) so
        // the first paint matches the header's active indication.
        new_list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );
        new_list
    }

    /// Reads `/proc` again, updates the per-process CPU% and the current
    /// user's visible rows (`self.processes`).
    pub fn update_process_list(&mut self) {
        match self.refresh_process_list() {
            Ok(processes) => {
                self.processes = processes
                    .into_iter()
                    .map(|p| ProcessItem::new(&p))
                    .collect();
            }
            Err(e) => {
                log::error!("Failed to update process list: {}", e);
                self.processes.clear();
            }
        }
    }

    pub fn refresh_process_list(&mut self) -> Result<Vec<TaskMgrProcess>> {
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
        let task_mgr_process_list: Vec<TaskMgrProcess> = all_processes
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
                    .update_process_cpu(&mut task_mgr_process, &stat);
                // /proc/[pid]/io is only readable for self-owned processes
                // (EACCES otherwise), so an unknown rate is expected and shown
                // as a blank cell.
                match proc.io() {
                    Ok(io) => {
                        self.io_tracker.update_process_io(
                            &mut task_mgr_process,
                            io.read_bytes,
                            io.write_bytes,
                        );
                    }
                    Err(e) => {
                        debug!("Can't read io for pid {}: {e:?}", proc.pid());
                    }
                }
                Some(task_mgr_process)
            })
            .collect();

        // Drop tracking entries for PIDs that no longer exist
        let live_pids: std::collections::HashSet<i32> =
            task_mgr_process_list.iter().map(|p| p.pid).collect();
        self.cpu_tracker
            .evict_dead_processes(|pid| live_pids.contains(&pid));
        self.io_tracker
            .evict_dead_processes(|pid| live_pids.contains(&pid));

        // Capture intermediate count before UID filtering
        let processes_before_filter = task_mgr_process_list.len();
        debug!(
            "After converting to TaskMgrProcess: {} processes",
            processes_before_filter
        );

        // Show every process when the "show all" toggle is on, otherwise only
        // the current user's.
        let task_mgr_process_list_filtered: Vec<TaskMgrProcess> =
            filter_by_user(task_mgr_process_list, current_uid, self.show_all);

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

    /// Sorts the process list in place by the specified column and direction.
    ///
    /// `f64` is compared with `total_cmp` so `NaN` values sort without
    /// panicking (unlike `partial_cmp().unwrap()`).
    pub fn sort_processes(&mut self, column: crate::SortColumn, direction: crate::SortDirection) {
        let by = |a: &ProcessItem, b: &ProcessItem| -> std::cmp::Ordering {
            match column {
                crate::SortColumn::Pid => a.pid.cmp(&b.pid),
                crate::SortColumn::Username => a.value.username.cmp(&b.value.username),
                crate::SortColumn::CpuPercent => {
                    a.value.cpu_percent.total_cmp(&b.value.cpu_percent)
                }
                crate::SortColumn::Name => a.value.name.cmp(&b.value.name),
                // Unknown (`None`) rates sort as 0.0 so rows without I/O data
                // sink to the bottom of an ascending sort.
                crate::SortColumn::DiskRead => a
                    .value
                    .disk_read_speed
                    .unwrap_or(0.0)
                    .total_cmp(&b.value.disk_read_speed.unwrap_or(0.0)),
                crate::SortColumn::DiskWrite => a
                    .value
                    .disk_write_speed
                    .unwrap_or(0.0)
                    .total_cmp(&b.value.disk_write_speed.unwrap_or(0.0)),
            }
        };
        match direction {
            crate::SortDirection::Ascending => self.processes.sort_by(by),
            crate::SortDirection::Descending => self.processes.sort_by(|a, b| by(b, a)),
        }
    }
}

/// Keeps the current user's processes unless `show_all` is set, in which case
/// every process is kept. Extracted for pure, unit-testable behavior.
fn filter_by_user(
    processes: Vec<TaskMgrProcess>,
    current_uid: u32,
    show_all: bool,
) -> Vec<TaskMgrProcess> {
    if show_all {
        processes
    } else {
        processes
            .into_iter()
            .filter(|p| p.ruid == current_uid)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::TaskMgrProcess;

    /// A `f64` NaN must not panic the sort (regression for `partial_cmp().unwrap()`).
    #[test]
    fn test_sort_by_cpu_percent_with_nan_does_not_panic() {
        let mut list = ProcessList::new();
        let items = [(1, f64::NAN), (2, 5.0), (3, -1.0)]
            .into_iter()
            .map(|(pid, cpu)| {
                let p = TaskMgrProcess::new(format!("name{}", pid), pid, 1, "u".to_string(), cpu);
                ProcessItem::new(&p)
            })
            .collect();
        list.processes = items;

        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Ascending,
        );
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );

        // Sorting must leave the list intact with the same set of PIDs.
        let mut pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![1, 2, 3]);
    }

    /// Re-sorting after a refresh re-applies the current sort to the new data;
    /// repeated sorts are stable and never lose or duplicate rows.
    #[test]
    fn test_repeated_sorts_are_stable_and_reapply() {
        fn make_items(pid: i32) -> ProcessItem {
            let p =
                TaskMgrProcess::new(format!("name{}", pid), pid, 1, "u".to_string(), pid as f64);
            ProcessItem::new(&p)
        }

        let mut list = ProcessList::new();
        list.processes = (6..=10).map(make_items).collect();

        // A refresh replaced the data; sort it descending by CPU (pid == cpu).
        list.processes = (5..=14).map(make_items).collect();
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );

        // Re-sorting the same column/direction is a no-op in ordering.
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );
        let pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![14, 13, 12, 11, 10, 9, 8, 7, 6, 5]);

        // Switching to an ascending sort reorders the same set of rows.
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Ascending,
        );
        let pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
    }

    /// `update_process_list` rebuilds the visible rows and keeps them consistent
    /// (every row maps to a unique pid) regardless of process churn.
    #[test]
    fn test_update_process_list_keeps_rows_unique() {
        let mut list = ProcessList::new();
        list.update_process_list();
        let mut pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        pids.sort();
        pids.dedup();
        assert_eq!(
            pids.len(),
            list.processes.len(),
            "no duplicate pids after refresh"
        );
    }

    /// `init` seeds the list with the default sort from the UI: CPU%
    /// descending, so the first paint is already ordered (not `Pid` order).
    #[test]
    fn test_init_sorts_by_cpu_descending() {
        let item = |pid: i32, cpu: f64| {
            let p = TaskMgrProcess::new(format!("name{pid}"), pid, 1, "u".to_string(), cpu);
            ProcessItem::new(&p)
        };

        let mut list = ProcessList::new();
        list.processes = vec![item(1, 0.5), item(2, 9.25), item(3, 1.75)];
        // Re-apply exactly what `init` does after `update_process_list`.
        list.sort_processes(
            crate::SortColumn::CpuPercent,
            crate::SortDirection::Descending,
        );

        let pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![2, 3, 1]);
    }

    fn proc(pid: i32, ruid: u32) -> TaskMgrProcess {
        TaskMgrProcess::new(format!("name{pid}"), pid, ruid, "u".to_string(), 0.0)
    }

    /// Sorting by a disk speed column tolerates unknown (`None`) rates without
    /// panicking; `None` sorts as 0.0.
    #[test]
    fn test_sort_by_disk_speeds_with_none_does_not_panic() {
        fn item(pid: i32, read: Option<f64>, write: Option<f64>) -> ProcessItem {
            let mut p = proc(pid, 1);
            p.disk_read_speed = read;
            p.disk_write_speed = write;
            ProcessItem::new(&p)
        }

        let mut list = ProcessList::new();
        list.processes = vec![
            item(1, None, Some(900.0)),
            item(2, Some(50.0), None),
            item(3, Some(10000.0), Some(20.0)),
        ];

        for col in [
            crate::SortColumn::DiskRead,
            crate::SortColumn::DiskWrite,
            crate::SortColumn::DiskRead,
            crate::SortColumn::DiskWrite,
        ] {
            for dir in [
                crate::SortDirection::Ascending,
                crate::SortDirection::Descending,
            ] {
                list.sort_processes(col, dir);
            }
        }

        // Rows are intact after sorting by a column mixing Some and None.
        let mut pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![1, 2, 3]);
    }

    /// With `show_all` off, only the current user's rows survive the filter.
    #[test]
    fn test_filter_by_user_keeps_current_uid_when_show_all_off() {
        let input = vec![proc(1, 1000), proc(2, 0), proc(3, 1000)];
        let out = filter_by_user(input, 1000, false);
        let pids: Vec<i32> = out.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![1, 3]);
    }

    /// With `show_all` on, every row is kept regardless of owner.
    #[test]
    fn test_filter_by_user_keeps_everything_when_show_all_on() {
        let input = vec![proc(1, 1000), proc(2, 0), proc(3, 1234)];
        let out = filter_by_user(input, 1000, true);
        let pids: Vec<i32> = out.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![1, 2, 3]);
    }

    /// The setter round-trips the flag used by the refresh path.
    #[test]
    fn test_set_show_all_toggles_flag() {
        let mut list = ProcessList::new();
        assert!(!list.show_all);
        list.set_show_all(true);
        assert!(list.show_all);
        list.set_show_all(false);
        assert!(!list.show_all);
    }
}
