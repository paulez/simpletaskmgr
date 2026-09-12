use crate::cpu_tracker::CpuTracker;
use crate::io_tracker::IoTracker;
use crate::metrics::read_mem_total_kb;
use crate::process::{build_task_mgr_process, resolve_process_name, ProcessItem, TaskMgrProcess};
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
                // `refresh_process_list` returns owned values — move them into the
                // row instead of cloning each (a `ProcessItem::new(&p)` copies
                // the row's `name`/`username` Strings per process per refresh).
                self.processes = processes
                    .into_iter()
                    .map(|p| ProcessItem {
                        pid: p.pid,
                        value: p,
                    })
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
        let mut io_last_err: Option<std::io::Error> = None;

        // Per-row order matters: the cheap `fstat`-based `uid()` runs first
        // and gates the expensive `/proc/[pid]/{stat,status,io}` reads. When
        // we're not showing every process (`show_all == false`), a
        // non-current-user row is thrown out by the end `filter_by_user`
        // anyway, so we skip `stat`, `status`, and `io` entirely for it —
        // the same trick `top` uses (`library/readproc.c:1249-1250` bails
        // before `stat2proc` on `!XinLN(sb.st_uid, uids, nuid)`).
        let want_all = self.show_all;
        let task_mgr_process_list: Vec<TaskMgrProcess> = all_processes
            .iter()
            .filter_map(|proc| {
                let ruid = match proc.uid() {
                    Ok(u) => u,
                    Err(e) => {
                        warn!("Can't read UID for pid {}: {e:?}", proc.pid());
                        return None;
                    }
                };
                if !want_all && ruid != current_uid {
                    return None;
                }
                let stat = match proc.stat() {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Can't read stat for pid {}: {e:?}", proc.pid());
                        return None;
                    }
                };
                let username = match self.users_cache.get_user_by_uid(ruid) {
                    Some(user) => user.name().to_string_lossy().into_owned(),
                    None => "unknown".to_string(),
                };
                let mut task_mgr_process = build_task_mgr_process(&stat, ruid, username);
                // `stat.comm` is kernel-capped at 15 chars, so the row's name
                // is re-derived from the process's `argv[0]` when the cmdline is
                // readable; the fallback is `comm` (kernel threads, zombies).
                task_mgr_process.name =
                    resolve_process_name(&stat.comm, proc.cmdline().ok().as_deref());
                self.cpu_tracker
                    .update_process_cpu(&mut task_mgr_process, &stat);
                // MEM% = VmRSS / MemTotal * 100 (top-style). `top` reads this
                // from `statm.resident` (7 short fields) rather than `status`
                // (60+ fields, one of which — VmRSS — is all we need);
                // `statm`'s values are in *pages*, so convert to KiB to match
                // `MemTotal`. A failed `statm` just leaves the cell blank
                // instead of failing the whole refresh.
                if let Some(total) = mem_total_kb {
                    if let Ok(statm) = proc.statm() {
                        let rss_kb = statm.resident.saturating_mul(procfs::page_size() / 1024);
                        if rss_kb > 0 {
                            task_mgr_process.mem_percent = Some(mem_percent_of(rss_kb, total));
                        }
                    }
                }
                // /proc/[pid]/io is only readable for self-owned processes
                // (EACCES otherwise), so skip the read entirely for other
                // users' processes — the rate column stays blank. As root
                // (uid 0) the file is readable for every pid. We read only the
                // two counters we use (`read_bytes`, `write_bytes`) instead of
                // `proc.io()`, which parses all seven into a `HashMap<String, _>`
                // and allocates a key `String` per field.
                if should_read_io(current_uid, ruid) {
                    match read_io_bytes(proc.pid()) {
                        Ok((read_bytes, write_bytes)) => {
                            self.io_tracker.update_process_io(
                                &mut task_mgr_process,
                                read_bytes,
                                write_bytes,
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

/// Reads only `read_bytes` and `write_bytes` from `/proc/[pid]/io`.
///
/// `procfs::Process::io()` parses all seven counters into a
/// `HashMap<String, u64>` and allocates a `String` key per field; we only ever
/// need the two *real-bytes* counters for the disk r/w rate, so parse just
/// those. Fields are `name: value` lines; this stops at the two it finds.
fn read_io_bytes(pid: i32) -> std::io::Result<(u64, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/io"))?;
    let mut read_bytes: Option<u64> = None;
    let mut write_bytes: Option<u64> = None;
    for line in text.lines() {
        // Each line is `field: value`; find the two we use and stop early.
        if let Some(value) = line.strip_prefix("read_bytes:") {
            read_bytes = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("write_bytes:") {
            write_bytes = value.trim().parse().ok();
        }
        if read_bytes.is_some() && write_bytes.is_some() {
            break;
        }
    }
    match (read_bytes, write_bytes) {
        (Some(r), Some(w)) => Ok((r, w)),
        (other, _) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("read_bytes/write_bytes missing from /proc/{pid}/io: {other:?}"),
        )),
    }
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

    fn proc(pid: i32, ruid: u32) -> TaskMgrProcess {
        TaskMgrProcess::new(format!("name{pid}"), pid, ruid, "u".to_string(), 0.0)
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
