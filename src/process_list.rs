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

    /// Compares two rows' values by the given column, in ascending order.
    ///
    /// `f64` is compared with `total_cmp` so `NaN` values sort without
    /// panicking (unlike `partial_cmp().unwrap()`). This is the single
    /// source of truth for row ordering: every native `GtkColumnView`
    /// header sorter wraps it via `CustomSorter`.
    ///
    /// `descending` reports whether the column is currently sorted
    /// descending (most-first). The always-present columns (`Pid`,
    /// `Username`, `CpuPercent`, `Name`) ignore it; the columns that may be
    /// empty (`MemPercent`, `DiskRead`, `DiskWrite`) use it to pin a missing
    /// value to the bottom of the visible list in either direction (see
    /// [`rank_optional`]).
    pub fn compare_values(
        a: &ProcessItem,
        b: &ProcessItem,
        column: crate::SortColumn,
        descending: bool,
    ) -> std::cmp::Ordering {
        match column {
            crate::SortColumn::Pid => a.pid.cmp(&b.pid),
            crate::SortColumn::Username => a.value.username.cmp(&b.value.username),
            crate::SortColumn::CpuPercent => a.value.cpu_percent.total_cmp(&b.value.cpu_percent),
            crate::SortColumn::MemPercent => {
                rank_optional(a.value.mem_percent, b.value.mem_percent, descending)
            }
            crate::SortColumn::Name => a.value.name.cmp(&b.value.name),
            // A missing (`None`) rate is pinned to the bottom of the visible
            // list however the column points, so rows without I/O data never
            // float up ahead of rows with a real rate (see [`rank_optional`]).
            crate::SortColumn::DiskRead => {
                rank_optional(a.value.disk_read_speed, b.value.disk_read_speed, descending)
            }
            crate::SortColumn::DiskWrite => rank_optional(
                a.value.disk_write_speed,
                b.value.disk_write_speed,
                descending,
            ),
        }
    }
}

/// Orders an optional numeric value (CPU-independent: MEM% / disk r/w) against
/// another, always settling the missing (`None`) one at the *bottom* of the
/// visible list — whichever direction the column is pointed.
///
/// GTK4 applies the header's direction itself: for the descending (most-first)
/// direction it *negates* this comparator's whole result (see
/// `gtkcolumnviewsorter.c`). So a comparator that put `None` at the bottom in
/// ascending order would be flipped to the top in descending order. The two
/// `None`/`Some` cases must therefore return opposite orderings per direction:
///
/// * ascending (no negation): report `None` as *greater* ⇒ it lands last;
/// * descending (negated):    report `None` as *less* ⇒ negation lands it last.
///
/// `Some`/`Some` still orders by value (`total_cmp`, `NaN` safe) and `None`/`None`
/// is `Equal`, in both directions. This replaces the old `unwrap_or(0.0)`
/// shortcut, which tied a missing rate with a real zero and let it surface at
/// the top of a data-first (descending) sort — the "empty rows first" bug.
fn rank_optional(a: Option<f64>, b: Option<f64>, descending: bool) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (None, None) => std::cmp::Ordering::Equal,
        // `a` is the missing side: rank it last (greater when ascending,
        // least-then-negated-to-last when descending).
        (None, Some(_)) => {
            if descending {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        }
        // Mirror of the arm above for the case where `b` is the missing side.
        (Some(_), None) => {
            if descending {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
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
        let ab = ProcessList::compare_values(&a, &b, crate::SortColumn::Pid, false);
        let ba = ProcessList::compare_values(&b, &a, crate::SortColumn::Pid, false);
        assert_eq!(ab, std::cmp::Ordering::Less);
        assert_eq!(ba, std::cmp::Ordering::Greater);

        // Consistency: equal values compare equal no matter the order.
        assert_eq!(
            ProcessList::compare_values(&a, &c, crate::SortColumn::Pid, false),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            ProcessList::compare_values(&c, &a, crate::SortColumn::Pid, false),
            std::cmp::Ordering::Equal
        );

        // Transitivity across three distinct values.
        let d = item(7);
        assert!(ProcessList::compare_values(&a, &d, crate::SortColumn::Pid, false).is_lt());
        assert!(ProcessList::compare_values(&d, &b, crate::SortColumn::Pid, false).is_lt());
        assert!(ProcessList::compare_values(&a, &b, crate::SortColumn::Pid, false).is_lt());
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
        // `CpuPercent` has no missing values, so the direction flag is a no-op
        // here — the comparator still must not panic and must order by value.
        sorted.sort_by(|a, b| {
            ProcessList::compare_values(a, b, crate::SortColumn::CpuPercent, false)
        });
        // `total_cmp` orders finite values first and NaN last.
        let pids: Vec<i32> = sorted.iter().map(|i| i.pid).collect();
        assert_eq!(
            pids,
            vec![2, 3, 1],
            "NaN must not panic and must order total_cmp"
        );

        // Equal values compare equal in either direction (no-op for `CpuPercent`).
        let a = item(9, 1.5);
        let b = item(10, 1.5);
        assert_eq!(
            ProcessList::compare_values(&a, &b, crate::SortColumn::CpuPercent, false),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            ProcessList::compare_values(&a, &b, crate::SortColumn::CpuPercent, true),
            std::cmp::Ordering::Equal
        );
    }

    fn proc(pid: i32, ruid: u32) -> TaskMgrProcess {
        TaskMgrProcess::new(format!("name{pid}"), pid, ruid, "u".to_string(), 0.0)
    }

    /// Comparing by a disk speed column pins a missing (`None`) rate to the
    /// *bottom* of the list in either direction (so it never surfaces ahead of
    /// a real rate), while `Some` rates still order by magnitude.
    #[test]
    fn test_compare_disk_speeds_none_pinned_last() {
        fn item(pid: i32, read: Option<f64>) -> ProcessItem {
            let mut p = proc(pid, 1);
            p.disk_read_speed = read;
            ProcessItem::new(&p)
        }

        let blank = item(1, None); // no I/O data
        let positive = item(2, Some(50.0));
        let real_zero = item(3, Some(0.0));

        // A missing rate ranks strictly *below* every real rate (and below a
        // real zero), and does so in BOTH directions: ascending reports a blank
        // as greater than a real value (so it lands last), descending reports
        // it as lesser (and GTK negates that, still landing it last). The two
        // argument orders give opposite results — a pair is antisymmetric.
        for &descending in &[false, true] {
            // `blank` first.
            let blank_first = if descending {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
            assert_eq!(
                ProcessList::compare_values(
                    &blank,
                    &positive,
                    crate::SortColumn::DiskRead,
                    descending
                ),
                blank_first,
                "blank vs 50.0 (descending={descending})"
            );
            // `positive` first — the antisymmetric mirror of the line above.
            assert_eq!(
                ProcessList::compare_values(
                    &positive,
                    &blank,
                    crate::SortColumn::DiskRead,
                    descending
                ),
                blank_first.reverse(),
                "50.0 vs blank (descending={descending})"
            );
            assert_eq!(
                ProcessList::compare_values(
                    &blank,
                    &real_zero,
                    crate::SortColumn::DiskRead,
                    descending
                ),
                blank_first,
                "blank vs 0.0 (descending={descending})"
            );
        }

        // Real rates still order by magnitude regardless of the direction flag.
        assert_eq!(
            ProcessList::compare_values(&positive, &real_zero, crate::SortColumn::DiskRead, false),
            std::cmp::Ordering::Greater
        );
        // Blank vs blank is always equal.
        assert_eq!(
            ProcessList::compare_values(&blank, &item(9, None), crate::SortColumn::DiskRead, true),
            std::cmp::Ordering::Equal
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

    /// `compare_mem_percent_with_none_and_nan` mirrors the disk-speed
    /// comparator for the MEM% column.
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

        // A missing value is pinned to the list bottom in BOTH directions:
        // ascending reports it greater (last), descending reports it lesser
        // (GTK then negates to still land it last).
        for &descending in &[false, true] {
            let blank_side = if descending {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
            assert_eq!(
                ProcessList::compare_values(
                    &unknown,
                    &high,
                    crate::SortColumn::MemPercent,
                    descending
                ),
                blank_side,
                "unknown vs 12.25 (descending={descending})"
            );
            assert_eq!(
                ProcessList::compare_values(
                    &unknown,
                    &low,
                    crate::SortColumn::MemPercent,
                    descending
                ),
                blank_side,
                "unknown vs 0.5 (descending={descending})"
            );
        }

        // Known values still order by `total_cmp`.
        assert_eq!(
            ProcessList::compare_values(&low, &high, crate::SortColumn::MemPercent, false),
            std::cmp::Ordering::Less,
        );
        // Equal known values compare equal (never panics on `NaN`).
        let a = item(5, Some(0.5));
        assert_eq!(
            ProcessList::compare_values(&low, &a, crate::SortColumn::MemPercent, false),
            std::cmp::Ordering::Equal,
        );
        // `NaN` is a *known* value: `total_cmp` orders it last among knowns
        // without panicking, so it still ranks above a finite value.
        assert_eq!(
            ProcessList::compare_values(&low, &nan, crate::SortColumn::MemPercent, false),
            std::cmp::Ordering::Less,
        );
        assert_eq!(
            ProcessList::compare_values(&nan, &low, crate::SortColumn::MemPercent, false),
            std::cmp::Ordering::Greater,
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
