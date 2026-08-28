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
        // Display order is owned by GTK's `SortListModel`; the initial
        // CPU%-descending sort is applied by `ui` after the first paint.
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

        // Read once per refresh and reuse for every row's MEM%.
        let mem_total_kb = read_mem_total_kb();

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

        let mut io_failed = 0u32;
        let mut io_last_err: Option<procfs::ProcError> = None;

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
                // MEM% = VmRSS / MemTotal * 100 (top-style). The VmRSS line is
                // absent for some kernel threads, so a missing `status` just
                // leaves the cell blank instead of failing the whole refresh.
                if let (Some(total), Some(status)) = (mem_total_kb, proc.status().ok()) {
                    if let Some(rss_kb) = status.vmrss {
                        task_mgr_process.mem_percent = Some(mem_percent_of(rss_kb, total));
                    }
                }
                // /proc/[pid]/io is only readable for self-owned processes
                // (EACCES otherwise), so skip the read entirely for other
                // users' processes — the rate column stays blank. As root
                // (uid 0) the file is readable for every pid.
                if should_read_io(current_uid, ruid) {
                    match proc.io() {
                        Ok(io) => {
                            self.io_tracker.update_process_io(
                                &mut task_mgr_process,
                                io.read_bytes,
                                io.write_bytes,
                            );
                        }
                        Err(e) => {
                            io_failed += 1;
                            io_last_err = Some(e);
                        }
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

        if io_failed > 0 {
            debug!(
                "Can't read io for {} owned process(es); last error: {io_last_err:?}",
                io_failed
            );
        }

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

    /// Compares two rows' values by the given column, ascending.
    ///
    /// `f64` is compared with `total_cmp` so `NaN` values sort without
    /// panicking (unlike `partial_cmp().unwrap()`). This is the single
    /// source of truth for row ordering: every native `GtkColumnView`
    /// header sorter wraps it via `CustomSorter`.
    pub fn compare_values(
        a: &ProcessItem,
        b: &ProcessItem,
        column: crate::SortColumn,
    ) -> std::cmp::Ordering {
        match column {
            crate::SortColumn::Pid => a.pid.cmp(&b.pid),
            crate::SortColumn::Username => a.value.username.cmp(&b.value.username),
            crate::SortColumn::CpuPercent => a.value.cpu_percent.total_cmp(&b.value.cpu_percent),
            crate::SortColumn::MemPercent => a
                .value
                .mem_percent
                .unwrap_or(0.0)
                .total_cmp(&b.value.mem_percent.unwrap_or(0.0)),
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
    }
}

/// Reads `MemTotal` from `/proc/meminfo`, in KiB. Returns `None` (and logs a
/// warning) if the file is unreadable or lacks the field; every row then shows
/// a blank MEM% cell.
fn read_mem_total_kb() -> Option<u64> {
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => {
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                if parts.next() == Some("MemTotal:") {
                    if let Some(value) = parts.next() {
                        return value.parse::<u64>().ok();
                    }
                }
            }
            warn!("MemTotal: not found in /proc/meminfo");
            None
        }
        Err(e) => {
            warn!("Can't read /proc/meminfo: {e:?}");
            None
        }
    }
}

/// `rss_kb` as a percentage of `total_kb`, top-style (`VmRSS / MemTotal * 100`).
///
/// Pure: no I/O. Only called with a non-zero `total_kb` — the caller checks
/// before dividing, since the MEM% cell stays blank when `MemTotal` is 0 or
/// unknown.
fn mem_percent_of(rss_kb: u64, total_kb: u64) -> f64 {
    debug_assert!(total_kb > 0, "mem_percent_of called with zero MemTotal");
    rss_kb as f64 / total_kb as f64 * 100.0
}

/// Whether `/proc/[pid]/io` should be read for a process with owner `ruid`
/// while running as `current_uid`. The file is only readable for the
/// process's owner or root, so it is only attempted when the process is
/// owned by the current user, or when running as root (uid 0).
fn should_read_io(current_uid: u32, ruid: u32) -> bool {
    current_uid == 0 || ruid == current_uid
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

    /// Reads should be attempted for owned processes, and for everything when
    /// running as root; never for other users' processes.
    #[test]
    fn test_should_read_io() {
        assert!(should_read_io(1000, 1000), "owned process");
        assert!(!should_read_io(1000, 1001), "other user's process");
        assert!(should_read_io(0, 1000), "root can read any process");
        assert!(should_read_io(0, 0), "root reading a root process");
    }

    /// A `f64` NaN must not panic the comparator (regression for
    /// `partial_cmp().unwrap()`): it must still order, never panic, and the
    /// resulting sort must be total.
    #[test]
    fn test_compare_cpu_nan_does_not_panic_and_is_total_order() {
        fn item(pid: i32, cpu: f64) -> ProcessItem {
            let p = TaskMgrProcess::new(format!("name{pid}"), pid, 1, "u".to_string(), cpu);
            ProcessItem::new(&p)
        }

        let items = vec![item(1, f64::NAN), item(2, 5.0), item(3, -1.0)];
        let mut ascending = items.clone();
        ascending.sort_by(|a, b| ProcessList::compare_values(a, b, crate::SortColumn::CpuPercent));

        // `total_cmp` orders the finite values and pushes NaN to the end.
        let pids: Vec<i32> = ascending.iter().map(|i| i.pid).collect();
        assert_eq!(pids, vec![3, 2, 1], "NaN sorts last; no panic; total order");
    }

    /// `compare_values` is a *total, antisymmetric, consistent* ordering for
    /// any column — the property a stable sort (whether Rust's or GTK's
    /// `SortListModel`) relies on to never lose or duplicate rows.
    #[test]
    fn test_compare_values_is_total_and_antisymmetric() {
        fn item(pid: i32) -> ProcessItem {
            let p = TaskMgrProcess::new(format!("name{pid}"), pid, 1, "u".to_string(), pid as f64);
            ProcessItem::new(&p)
        }

        let a = item(5);
        let b = item(9);
        let c = item(5); // same pid as `a` but a distinct object

        // Antisymmetry: compare(a,b) == reverse of compare(b,a).
        let ab = ProcessList::compare_values(&a, &b, crate::SortColumn::Pid);
        let ba = ProcessList::compare_values(&b, &a, crate::SortColumn::Pid);
        assert_eq!(ab, std::cmp::Ordering::Less);
        assert_eq!(ba, std::cmp::Ordering::Greater);

        // Consistency: equal values compare equal no matter the order.
        assert_eq!(
            ProcessList::compare_values(&a, &c, crate::SortColumn::Pid),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            ProcessList::compare_values(&c, &a, crate::SortColumn::Pid),
            std::cmp::Ordering::Equal
        );

        // Transitivity across three distinct values.
        let d = item(7);
        assert!(ProcessList::compare_values(&a, &d, crate::SortColumn::Pid).is_lt());
        assert!(ProcessList::compare_values(&d, &b, crate::SortColumn::Pid).is_lt());
        assert!(ProcessList::compare_values(&a, &b, crate::SortColumn::Pid).is_lt());
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

    /// `init` no longer sorts (display order is owned by GTK's `SortListModel`,
    /// applied by `ui` after the first paint). All it must guarantee is that
    /// it populates the current-user rows from `/proc`, uniquely.
    #[test]
    fn test_init_populates_unique_rows() {
        let list = ProcessList::init();
        let mut pids: Vec<i32> = list.processes.iter().map(|p| p.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        assert_eq!(
            pids.len(),
            list.processes.len(),
            "init must not duplicate a pid"
        );
        assert!(
            !list.processes.is_empty(),
            "init must see at least the current process"
        );
    }

    /// `compare_values` is the exact comparator the `ColumnView` sorters run:
    /// it must be consistent with the sort order and NaN-safe.
    #[test]
    fn test_compare_values_matches_sort_order_and_is_nan_safe() {
        fn item(pid: i32, cpu: f64) -> ProcessItem {
            let p = TaskMgrProcess::new(format!("n{pid}"), pid, 1, "u".to_string(), cpu);
            ProcessItem::new(&p)
        }

        let nan = item(1, f64::NAN);
        let lo = item(2, -5.0);
        let hi = item(3, 42.0);

        let ascending = [lo.clone(), nan.clone(), hi.clone()];
        let mut sorted = ascending;
        sorted.sort_by(|a, b| ProcessList::compare_values(a, b, crate::SortColumn::CpuPercent));
        // `total_cmp` orders finite values first and NaN last.
        let pids: Vec<i32> = sorted.iter().map(|i| i.pid).collect();
        assert_eq!(
            pids,
            vec![2, 3, 1],
            "NaN must not panic and must order total_cmp"
        );

        // Equal values compare equal regardless of direction.
        let a = item(9, 1.5);
        let b = item(10, 1.5);
        assert_eq!(
            ProcessList::compare_values(&a, &b, crate::SortColumn::CpuPercent),
            std::cmp::Ordering::Equal
        );

        // `None` disk rates sort as 0.0.
        let mut none_r = item(20, 0.0);
        none_r.value.disk_read_speed = None;
        let mut some_r = item(21, 0.0);
        some_r.value.disk_read_speed = Some(10.0);
        assert_eq!(
            ProcessList::compare_values(&none_r, &some_r, crate::SortColumn::DiskRead),
            std::cmp::Ordering::Less
        );
    }

    fn proc(pid: i32, ruid: u32) -> TaskMgrProcess {
        TaskMgrProcess::new(format!("name{pid}"), pid, ruid, "u".to_string(), 0.0)
    }

    /// Comparing by a disk speed column tolerates unknown (`None`) rates
    /// without panicking; `None` sorts as 0.0.
    #[test]
    fn test_compare_disk_speeds_with_none_does_not_panic() {
        fn item(pid: i32, read: Option<f64>, write: Option<f64>) -> ProcessItem {
            let mut p = proc(pid, 1);
            p.disk_read_speed = read;
            p.disk_write_speed = write;
            ProcessItem::new(&p)
        }

        let none = item(1, None, Some(900.0));
        let some_read = item(2, Some(50.0), None);
        let some_write = item(3, None, Some(20.0));

        // `None` behaves as 0.0, so it sorts below any positive rate and
        // above nothing on either column — never a panic.
        assert_eq!(
            ProcessList::compare_values(&none, &some_read, crate::SortColumn::DiskRead),
            std::cmp::Ordering::Less,
            "None (0.0) < 50.0 on disk-read"
        );
        assert_eq!(
            ProcessList::compare_values(&none, &some_write, crate::SortColumn::DiskWrite),
            std::cmp::Ordering::Greater,
            "900.0 > None (0.0) on disk-write"
        );

        // `None` on both sides compares equal (both are 0.0) — still defined,
        // not a panic; a real rate still beats it.
        let both_none = item(4, None, None);
        assert_eq!(
            ProcessList::compare_values(&none, &both_none, crate::SortColumn::DiskRead),
            std::cmp::Ordering::Equal,
            "None vs None on disk-read is 0.0 == 0.0"
        );
        assert_eq!(
            ProcessList::compare_values(&some_read, &both_none, crate::SortColumn::DiskRead),
            std::cmp::Ordering::Greater,
            "50.0 > None (0.0) on disk-read"
        );
    }

    /// The MEM% ratio is top-style: RSS over total RAM, times 100.
    #[test]
    fn test_mem_percent_of_scales_rss_by_total() {
        assert_eq!(mem_percent_of(100_000, 20_000_000), 0.5);
        assert_eq!(mem_percent_of(5_000_000, 10_000_000), 50.0);
        assert_eq!(mem_percent_of(1, 1000), 0.1);
        // Sub-percent values keep their precision so the UI can show "0.3%".
        assert!((mem_percent_of(30_000, 10_000_000) - 0.3).abs() < 1e-12);
    }

    /// `read_mem_total_kb` parses the real file's `MemTotal` line when present;
    /// it is expected to succeed on any Linux host. On a host without it (or
    /// reading a container with restricted meminfo) the row simply shows no
    /// MEM% — both outcomes are acceptable here, but the value, when present,
    /// must be positive.
    #[test]
    fn test_read_mem_total_kb_is_positive_when_available() {
        let maybe = read_mem_total_kb();
        if let Some(kb) = maybe {
            assert!(kb > 0, "a real MemTotal can't be zero");
        }
    }

    /// Comparing two rows by MEM% is NaN-safe and `None`-tolerant, mirroring
    /// the disk-speed comparator.
    #[test]
    fn test_compare_mem_percent_with_none_and_nan() {
        fn item(pid: i32, mem: Option<f64>) -> ProcessItem {
            let mut p = TaskMgrProcess::new(format!("n{pid}"), pid, 1, "u".to_string(), 0.0);
            p.mem_percent = mem;
            ProcessItem::new(&p)
        }

        let unknown = item(1, None);
        let low = item(2, Some(0.5));
        let high = item(3, Some(12.25));
        let nan = item(4, Some(f64::NAN));

        // `None` (0.0) sorts below any known value.
        assert_eq!(
            ProcessList::compare_values(&unknown, &high, crate::SortColumn::MemPercent),
            std::cmp::Ordering::Less,
        );
        assert_eq!(
            ProcessList::compare_values(&low, &high, crate::SortColumn::MemPercent),
            std::cmp::Ordering::Less,
        );
        // Equal values compare equal, and NaN compares `Greater` under
        // `total_cmp` (never panics).
        let a = item(5, Some(0.5));
        assert_eq!(
            ProcessList::compare_values(&low, &a, crate::SortColumn::MemPercent),
            std::cmp::Ordering::Equal,
        );
        assert_eq!(
            ProcessList::compare_values(&unknown, &nan, crate::SortColumn::MemPercent),
            std::cmp::Ordering::Less,
        );
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
