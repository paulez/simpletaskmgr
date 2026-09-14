/// A plain, immutable snapshot of a process row's data as read from `/proc`.
///
/// `PartialEq` compares the whole struct field-by-field. `cpu_percent` is an
/// `f64`, so `Eq`/`Hash` are intentionally not implemented (an `f64` that can
/// be `NaN` has a `PartialEq` that is not reflexive, and no caller of this repo
/// needs it as a hash key anyway — the list keys rows on the stable `i32`
/// `pid`, not on the value).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskMgrProcess {
    /// The process's program name: `argv[0]`'s basename when the cmdline is
    /// readable (the full, un-truncated name), else the kernel's 15-char
    /// `comm` (e.g. kernel threads, zombies). See [`resolve_process_name`].
    pub name: String,
    pub pid: i32,
    pub ruid: u32,
    pub username: String,
    pub cpu_percent: f64, // top-style per-core CPU%, may exceed 100 for multi-threaded
    /// CPU *sort* key: integer CPU ticks of the latest sample, not the f64 above.
    ///
    /// The CPU% column orders rows by this `u64` (tie-broken by `pid`) instead
    /// of the full-precision `cpu_percent`: quantized to ticks of the sampling
    /// window (10 ms at 100 Hz), so two rows that display the same `0.1%`
    /// rounded value are always tied, and the `pid` tie-break keeps their
    /// relative order stable across refreshes — the same combination of a
    /// coarse integer key (`PIDS_TICS_ALL_DELTA`) and stable ordering that
    /// `top` uses. A process's first sample (and after a PID-reuse baseline
    /// reset) sets this to `0`, so a fresh row starts in the zero group and
    /// rises to its true bucket once real deltas are measured (see
    /// `doc/CPU_FIRST_SIGHT_BLANK_LINES.md`).
    pub cpu_ticks: u64,
    /// Share of system memory in percent (`VmRSS / MemTotal * 100`), or
    /// `None` when the RSS can't be read (e.g. another user's kernel thread).
    pub mem_percent: Option<f64>,
    /// Disk read speed in bytes/second, or `None` when no rate is known yet
    /// (first sample of a newly tracked process, or no permission to read
    /// `/proc/[pid]/io`).
    pub disk_read_speed: Option<f64>,
    /// Disk write speed in bytes/second, or `None` when no rate is known yet.
    pub disk_write_speed: Option<f64>,
    /// The full command line (`argv` joined by single spaces), or `None` when
    /// the process has no readable cmdline — kernel threads and zombies have
    /// an empty one. Shown as its own row in the detail pane.
    pub cmdline: Option<String>,
    /// The process state (`/proc/<pid>/stat` field 3): one of `R` (running),
    /// `S` (sleeping), `D` (disk sleep), `Z` (zombie), `T` (stopped),
    /// `t` (traced), `I` (idle kernel thread), `P` (parked kernel thread).
    /// Displayed through [`state_name`].
    pub state: char,
    /// The number of threads in the process (`/proc/<pid>/stat` field 20).
    /// `0` when the row was not built from a real `stat` (test fixtures).
    pub threads: u64,
    /// The process's scheduling priority, signed (lower runs sooner); the
    /// conventional range is `-20..19`.
    pub nice: i64,
    /// The parent process's PID (`/proc/<pid>/stat` field 4). `0` when the
    /// row was not built from a real `stat` (test fixtures).
    pub ppid: i32,
    /// The resident set size in KiB (`VmRSS`), or `None` when it could not
    /// be read (e.g. another user's kernel thread). Displayed alongside the
    /// MEM% as the absolute memory use.
    pub rss_kb: Option<u64>,
    /// The process start time as Unix seconds, or `None` when it could not
    /// be derived (no `/proc/uptime`, or a counter regression). Shown as a
    /// wall-clock timestamp plus the process's age ("uptime").
    pub start_epoch: Option<i64>,
}

impl TaskMgrProcess {
    pub fn new(name: String, pid: i32, ruid: u32, username: String, cpu_percent: f64) -> Self {
        Self {
            name,
            pid,
            ruid,
            username,
            cpu_percent,
            cpu_ticks: 0,
            mem_percent: None,
            disk_read_speed: None,
            disk_write_speed: None,
            cmdline: None,
            state: 'S',
            threads: 0,
            nice: 0,
            ppid: 0,
            rss_kb: None,
            start_epoch: None,
        }
    }

    pub fn cpu_percent_str(&self) -> String {
        format!("{:.1}%", self.cpu_percent)
    }

    /// The memory percent formatted like `"1.5%"`, or `""` when unknown.
    pub fn mem_percent_str(&self) -> String {
        self.mem_percent
            .map(|p| format!("{p:.1}%"))
            .unwrap_or_default()
    }

    /// The disk read speed formatted like `"12.3 KiB/s"`, or `""` when unknown.
    pub fn disk_read_str(&self) -> String {
        format_disk_speed(self.disk_read_speed)
    }

    /// The disk write speed formatted like `"456 B/s"`, or `""` when unknown.
    pub fn disk_write_str(&self) -> String {
        format_disk_speed(self.disk_write_speed)
    }

    /// The full command line, or `""` when none was captured (kernel thread,
    /// zombie) — the UI then shows its `"—"` placeholder.
    pub fn cmdline_str(&self) -> String {
        self.cmdline.clone().unwrap_or_default()
    }

    /// The process state formatted for humans, e.g. `"Sleeping"`.
    pub fn state_str(&self) -> String {
        state_name(self.state)
    }

    /// The resident set size formatted like `"312 MB"`, or `""` when unknown.
    pub fn rss_str(&self) -> String {
        format_size_kb(self.rss_kb)
    }

    /// The process's age relative to `now_epoch` (Unix seconds) formatted
    /// like `"2h 3m"`, or `""` when the start time is unknown.
    pub fn elapsed_str(&self, now_epoch: i64) -> String {
        self.start_epoch
            .map(|start| elapsed_secs_str(now_epoch.saturating_sub(start)))
            .unwrap_or_default()
    }

    /// The start time label (local wall-clock) — the time of day for a
    /// process started today, or the date plus a shorter time for one from
    /// another day — or `""` when the birth time is unknown so the UI shows
    /// its `"—"` placeholder. Delegates to the free [`start_time_str`] with
    /// this row's stored epoch.
    pub fn started_str(&self) -> String {
        start_time_str(self.start_epoch)
    }
}

/// Formats a bytes/second rate with a unit suffix, e.g. `"12.3 KiB/s"`.
///
/// `None` (no known rate) formats to the empty string so the UI can show a
/// blank cell instead of a misleading zero.
pub fn format_disk_speed(bytes_per_sec: Option<f64>) -> String {
    let Some(v) = bytes_per_sec else {
        return String::new();
    };
    const UNITS: &[&str] = &["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"];
    let mut value = v;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{value:.0} {}", UNITS[idx])
    } else {
        format!("{value:.1} {}", UNITS[idx])
    }
}

/// Maps a `/proc/<pid>/stat` state letter to a human name.
///
/// Mirrors the kernel's letter codes (`R`/`S`/`D`/`Z`/`T`/`t`/`I`/`P`); an
/// unrecognized letter is passed through rather than dropped so a future
/// kernel state doesn't silently read as a different one below it.
pub fn state_name(c: char) -> String {
    match c {
        'R' => "Running".to_string(),
        'S' => "Sleeping".to_string(),
        'D' => "Disk sleep".to_string(),
        'Z' => "Zombie".to_string(),
        'T' => "Stopped".to_string(),
        't' => "Traced".to_string(),
        'I' => "Idle kernel".to_string(),
        'P' => "Parked kernel".to_string(),
        other => format!("Unknown ({other})"),
    }
}

/// Formats an absolute memory size given in **KiB** with a unit suffix, e.g.
/// `"312 MB"`. `None` (unreadable) formats to the empty string so the UI can
/// show a blank cell instead of a misleading zero. Sizes under 1 MiB are
/// shown in KiB; the detail pane is the only user and human memory readings
/// are almost always ≥ 1 MiB, so KiB is kept for small kernel threads.
pub fn format_size_kb(kb: Option<u64>) -> String {
    let Some(kb) = kb else {
        return String::new();
    };
    let bytes = kb.saturating_mul(1024);
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{value:.0} {}", UNITS[idx])
    } else {
        format!("{value:.1} {}", UNITS[idx])
    }
}

/// Formats a process age in whole seconds as a compact duration, e.g. `"2h
/// 3m"`, `"45s"`. Sub-second ages collapse to `"0s"`; negatives (a start time
/// in the future, which must not happen) clamp to zero rather than render a
/// nonsense negative duration.
pub fn elapsed_secs_str(secs: i64) -> String {
    let secs = secs.max(0);
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Derives the process start time as **Unix seconds** from its
/// `stat.starttime` (clock ticks *since boot*, not an epoch)
///
/// The conversion is `now - age`, where `age = uptime - starttime/tps` is the
/// process's age in seconds at the moment `/proc/uptime` was read and `tps` is
/// the host's clock ticks per second. `None` when `uptime` is `None`
/// (`/proc/uptime` unreadable), `tps` is `0`, the age is negative (a counter
/// regression), or the arithmetic would overflow.
pub fn parse_stat_start_time(
    now_epoch: i64,
    uptime_secs: Option<f64>,
    starttime_ticks: u64,
    tps: u64,
) -> Option<i64> {
    let up = uptime_secs?;
    let tps = if tps == 0 { return None } else { tps };
    let secs_since_boot = starttime_ticks as f64 / tps as f64;
    let age = up - secs_since_boot;
    if age < 0.0 {
        return None;
    }
    let age_secs = age as i64;
    now_epoch.checked_sub(age_secs)
}

/// Formats a start time (Unix seconds) for display.
///
/// Delegates to [`format_time_at`] with the host's local UTC offset, so the
/// label reads local wall-clock time (a process manager's audience reads
/// local time). Returns `""` for `None` so the UI can show its `"—"`
/// placeholder.
pub fn start_time_str(epoch_secs: Option<i64>) -> String {
    let Some(secs) = epoch_secs else {
        return String::new();
    };
    use chrono::Local;
    let now = Local::now();
    let offset = now.offset().utc_minus_local();
    format_time_at(secs, now.timestamp(), offset)
}

/// Pure formatting of a start time with an explicit UTC offset and `now`
/// reference — testable without touching the host's timezone.
///
/// Within the same local calendar day as `now` only the time is shown (the
/// common case for a freshly started process); otherwise the date is
/// included so a process from days ago isn't mistaken for one from hours
/// ago. `""` when the offset is out of range for a [`chrono::FixedOffset`]
/// or a timestamp is invalid.
pub fn format_time_at(secs: i64, now_secs: i64, offset_secs: i32) -> String {
    use chrono::{FixedOffset, TimeZone};
    let Some(tz) = FixedOffset::east_opt(offset_secs) else {
        return String::new();
    };
    let Some(dt) = tz.timestamp_opt(secs, 0).latest() else {
        return String::new();
    };
    let Some(now) = tz.timestamp_opt(now_secs, 0).latest() else {
        return String::new();
    };
    if dt.format("%Y-%m-%d").to_string() == now.format("%Y-%m-%d").to_string() {
        dt.format("%H:%M:%S").to_string()
    } else {
        dt.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// One process row as rendered by the UI.
///
/// `pid` is the row's stable identity (used to match a row across refreshes);
/// `value` is the latest `TaskMgrProcess` snapshot for it. On each refresh we
/// keep the same `ProcessItem` for a given `pid` and replace its `value`, so
/// the row's fields carry over without the row being rebuilt from scratch.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessItem {
    pub pid: i32,
    /// The row's current data; replaced on every refresh.
    pub value: TaskMgrProcess,
}

impl Default for ProcessItem {
    fn default() -> Self {
        Self {
            pid: 0,
            value: TaskMgrProcess::new(String::new(), 0, 0, String::new(), 0.0),
        }
    }
}

impl ProcessItem {
    /// Creates a new row from a `TaskMgrProcess`.
    pub fn new(p: &TaskMgrProcess) -> Self {
        Self {
            pid: p.pid,
            value: p.clone(),
        }
    }

    /// The current CPU% as a `top`-style string, e.g. `"12.3%"`.
    pub fn cpu_percent_str(&self) -> String {
        self.value.cpu_percent_str()
    }

    /// The current MEM% as a string, e.g. `"1.5%"`, or `""` when unknown.
    pub fn mem_percent_str(&self) -> String {
        self.value.mem_percent_str()
    }
}

/// Builds a `TaskMgrProcess` from an already-read `stat` and the resolved UID.
///
/// Pure: performs no I/O. The caller is responsible for reading `/proc` once and
/// resolving the username, which lets the CPU tracker reuse the same `stat`.
pub(crate) fn build_task_mgr_process(
    stat: &procfs::process::Stat,
    ruid: u32,
    username: String,
) -> crate::process::TaskMgrProcess {
    TaskMgrProcess {
        name: stat.comm.clone(),
        pid: stat.pid,
        ruid,
        username,
        cpu_percent: 0.0,
        cpu_ticks: 0,
        mem_percent: None,
        disk_read_speed: None,
        disk_write_speed: None,
        cmdline: None,
        state: stat.state,
        threads: stat.num_threads.max(0) as u64,
        nice: stat.nice,
        ppid: stat.ppid,
        rss_kb: None,
        start_epoch: None,
    }
}

/// Resolves a process's display name to a value that is not capped by the
/// kernel's 15-char `TASK_COMM_LEN`.
///
/// `/proc/[pid]/comm` is hard-limited to 15 bytes (`stat.comm`), so it
/// truncates real program names (`gnome-terminal-server` → `gnome-terminal-`).
/// The fuller name is the process's `argv[0]` from `/proc/[pid]/cmdline`. We
/// prefer its *basename* so `argv[0]` works whether or not it carries a
/// leading path, and a name with spaces (e.g. "Isolated Web Content
/// (renderer)") survives because only `/` matters when splitting.
///
/// `argv[0]` is unavailable — the cmdline is empty or unreadable — for kernel
/// threads and zombies, so we fall back to `comm`, which is always present.
pub(crate) fn resolve_process_name(comm: &str, cmdline: Option<&[String]>) -> String {
    if let Some(arg0) = cmdline.and_then(|argv| argv.first()) {
        if let Some(basename) = std::path::Path::new(arg0)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
        {
            return basename.to_string();
        }
    }
    comm.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn proc(pid: i32, cpu: f64) -> TaskMgrProcess {
        crate::testutil::test_process(pid, cpu)
    }

    /// `cpu_percent_str` rounds to one decimal with a `%` suffix.
    #[rstest]
    #[case::zero(0.0, "0.0%")]
    #[case::partial(12.34, "12.3%")]
    fn test_cpu_percent_str(#[case] percent: f64, #[case] expected: &str) {
        assert_eq!(proc(1, percent).cpu_percent_str(), expected);
    }

    /// `mem_percent` starts unknown until a refresh has measured it, and a
    /// known value formats like CPU% while `None` formats to an empty cell.
    #[test]
    fn test_mem_percent_defaults_to_unknown_and_formats() {
        let p = proc(1, 0.0);
        assert!(p.mem_percent.is_none());
        assert_eq!(p.mem_percent_str(), "");

        let mut known = proc(1, 0.0);
        known.mem_percent = Some(1.25);
        assert_eq!(known.mem_percent_str(), "1.2%");
        known.mem_percent = Some(42.96);
        assert_eq!(known.mem_percent_str(), "43.0%");
    }

    /// `ProcessItem` mirrors `mem_percent_str` from the current snapshot.
    #[test]
    fn test_process_item_mem_percent_str() {
        let mut p = proc(7, 12.34);
        p.mem_percent = Some(3.5);
        let item = ProcessItem::new(&p);
        assert_eq!(item.mem_percent_str(), "3.5%");
    }

    #[test]
    fn test_disk_speeds_default_to_unknown() {
        let p = proc(1, 0.0);
        assert!(p.disk_read_speed.is_none());
        assert!(p.disk_write_speed.is_none());
        assert_eq!(p.disk_read_str(), "");
        assert_eq!(p.disk_write_str(), "");
    }

    #[test]
    fn test_disk_str_formats_known_rates() {
        let mut p = proc(1, 0.0);
        p.disk_read_speed = Some(1024.0 * 1024.0 * 1.5);
        p.disk_write_speed = Some(950.0);
        assert_eq!(p.disk_read_str(), "1.5 MiB/s");
        assert_eq!(p.disk_write_str(), "950 B/s");
    }

    /// `format_disk_speed` scales B → KiB → MiB → GiB → TiB and rounds to one
    /// decimal; `None` renders an empty cell.
    #[rstest]
    #[case::none(None, "")]
    #[case::zero(Some(0.0), "0 B/s")]
    #[case::bytes(Some(999.0), "999 B/s")]
    #[case::kib(Some(1024.0 * 12.3), "12.3 KiB/s")]
    #[case::mib(Some(1024.0 * 1024.0 * 512.5), "512.5 MiB/s")]
    #[case::gib(Some(1024.0 * 1024.0 * 1024.0 * 9.99), "10.0 GiB/s")]
    #[case::tib(Some(1024.0 * 1024.0 * 1024.0 * 1024.0 * 3.0), "3.0 TiB/s")]
    #[case::above_tib(Some(1024.0 * 1024.0 * 1024.0 * 1024.0 * 3072.0), "3072.0 TiB/s")]
    fn test_format_disk_speed(#[case] bytes_per_sec: Option<f64>, #[case] expected: &str) {
        assert_eq!(format_disk_speed(bytes_per_sec), expected);
    }

    /// A `ProcessItem` exposes its stable `pid` and a current snapshot.
    #[test]
    fn test_process_item_new_and_value() {
        let p = proc(7, 12.34);
        let item = ProcessItem::new(&p);
        assert_eq!(item.pid, 7);
        assert_eq!(item.value, p);
        assert_eq!(item.cpu_percent_str(), "12.3%");
    }

    /// A row is a pure value: cloning it yields an independent, equal copy.
    #[test]
    fn test_process_item_clone_is_value_copy() {
        let a = proc(7, 1.0);
        let item = ProcessItem::new(&a);
        let cloned = item.clone();
        assert_eq!(cloned, item);
        assert_eq!(cloned.value.cpu_percent, 1.0);
        assert_eq!(cloned.pid, item.pid);
    }

    /// Constructing a row by moving an owned value is byte-for-byte the same
    /// row as the borrowed `new(&p)` path — this pins the `update_process_list`
    /// refactor, which moved `refresh_process_list`'s output into rows instead
    /// of cloning each one.
    #[test]
    fn test_process_item_moved_construction_matches_borrowed() {
        let p = proc(7, 1.0);
        let from_ref = ProcessItem::new(&p);
        let owned = p;
        let from_owned = ProcessItem {
            pid: owned.pid,
            value: owned,
        };
        assert_eq!(from_owned, from_ref);
        assert_eq!(from_owned.pid, 7);
        assert_eq!(from_owned.value.name, "name7");
    }

    /// `resolve_process_name` prefers the `argv[0]` basename so a name longer
    /// than the kernel's 15-char `comm` cap comes through untouched.
    #[test]
    fn test_resolve_prefers_argv0_basename_over_truncated_comm() {
        // A cmdline that is a *path* resolves to its basename.
        let cmdline = vec!["/usr/libexec/gnome-terminal-server".to_string()];
        assert_eq!(
            resolve_process_name("gnome-terminal-", Some(cmdline.as_slice())),
            "gnome-terminal-server"
        );

        // A bare argv[0] (no path) is returned as-is.
        let cmdline = vec!["dbus-broker-launch".to_string()];
        assert_eq!(
            resolve_process_name("dbus-broker-lau", Some(cmdline.as_slice())),
            "dbus-broker-launch"
        );
    }

    /// A multi-word name with spaces survives resolution — only `/` splits the
    /// basename, not whitespace.
    #[test]
    fn test_resolve_keeps_names_with_spaces() {
        let cmdline = vec!["/usr/libexec/Isolated Web Content (renderer)".to_string()];
        assert_eq!(
            resolve_process_name("Isolated Web Co", Some(cmdline.as_slice())),
            "Isolated Web Content (renderer)"
        );
    }

    /// With no readable cmdline (kernel thread, zombie) the name falls back to
    /// `comm`, the one name that is always available.
    #[test]
    fn test_resolve_falls_back_to_comm_when_cmdline_missing() {
        assert_eq!(resolve_process_name("kworker/0:1", None), "kworker/0:1");
        // An empty cmdline (some kernel threads, zombies) also falls back.
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            resolve_process_name("ksoftirqd/0", Some(empty.as_slice())),
            "ksoftirqd/0"
        );
    }

    /// A `argv[0]` that yields no basename (e.g. the root path `"/"`) falls
    /// back to `comm` rather than producing an empty name.
    #[test]
    fn test_resolve_falls_back_when_argv0_has_no_basename() {
        let cmdline = vec!["/".to_string(), "arg".to_string()];
        assert_eq!(
            resolve_process_name("some-comm-name", Some(cmdline.as_slice())),
            "some-comm-name"
        );
    }

    /// Kernel state letters map to their conventional names; an unfamiliar
    /// letter is passed through (labeled) rather than silently mis-rendered.
    #[test]
    fn test_state_name_maps_kernel_letters() {
        for (letter, name) in [
            ('R', "Running"),
            ('S', "Sleeping"),
            ('D', "Disk sleep"),
            ('Z', "Zombie"),
            ('T', "Stopped"),
            ('t', "Traced"),
            ('I', "Idle kernel"),
            ('P', "Parked kernel"),
        ] {
            assert_eq!(state_name(letter), name, "state {letter}");
        }
        // An unknown letter keeps its identity in the label.
        assert_eq!(state_name('Q'), "Unknown (Q)");
    }

    /// Absolute memory sizes render with a binary unit suffix and one decimal
    /// place (KiB and up); `None` and sub-KiB stay unambiguous.
    #[test]
    fn test_format_size_kb_units() {
        assert_eq!(format_size_kb(None), "", "None → blank cell");
        assert_eq!(format_size_kb(Some(512)), "512.0 KiB", "512 KiB");
        assert_eq!(format_size_kb(Some(3072)), "3.0 MiB", "3 MiB");
        assert_eq!(
            format_size_kb(Some(314_572)),
            "307.2 MiB",
            "307.2 MiB (314572 KiB)"
        );
        assert_eq!(
            format_size_kb(Some(1_610_612_736)),
            "1.5 TiB",
            "1536 GiB input promotes to TiB (1536 GiB >= 1024 GiB)"
        );
    }

    /// Ages render compactly: days+hours, hours+minutes, minutes+seconds,
    /// then seconds; a negative age (start in the future) clamps to `0s`.
    #[test]
    fn test_elapsed_secs_str_compacts() {
        assert_eq!(elapsed_secs_str(0), "0s");
        assert_eq!(elapsed_secs_str(45), "45s");
        assert_eq!(elapsed_secs_str(5_400), "1h 30m", "5400s = 1h 30m");
        assert_eq!(
            elapsed_secs_str(86_400 + 2 * 3600 + 90),
            "1d 2h",
            "day + hour drops minutes"
        );
        assert_eq!(elapsed_secs_str(-100), "0s", "negative clamps to 0s");
    }

    /// The start time is `now - (uptime - starttime/tps)`: the process's
    /// birth, not a tick count. `None` guards a missing `/proc/uptime`, a
    /// zero tick rate, and a counter regression (future start).
    #[test]
    fn test_parse_stat_start_time() {
        // `now` = 5000s, `uptime` = 1000s, born 10000 ticks ago at 100 Hz
        // (100s after boot) → the process started 900s before `now` → 4100.
        assert_eq!(
            parse_stat_start_time(5_000, Some(1000.0), 10_000, 100),
            Some(4_100)
        );
        // `now` = 1000s, `uptime` = 1000s, born 5000 ticks (50s) after boot
        // → age 950s → start = 1000 - 950 = 50 (just after the epoch).
        assert_eq!(
            parse_stat_start_time(1_000, Some(1_000.0), 5_000, 100),
            Some(50)
        );
        assert_eq!(
            parse_stat_start_time(5_000, None, 10_000, 100),
            None,
            "no /proc/uptime → unknown"
        );
        assert_eq!(
            parse_stat_start_time(5_000, Some(1000.0), 10_000, 0),
            None,
            "zero ticks/sec is unusable"
        );
        // Started after now: `starttime/tps > uptime` (e.g. `uptime` read
        // before boot) → a regression the caller must drop, not show as a
        // negative age.
        assert_eq!(
            parse_stat_start_time(5_000, Some(10.0), 10_000, 100),
            None,
            "counter regression → None, not a future timestamp"
        );
        // Sub-second precision floors to whole seconds: born 1.37s after
        // boot (137 ticks at 100 Hz), system up 150s → age 148s (truncated)
        // → start = 150 - 148 = 2s after the epoch.
        assert_eq!(
            parse_stat_start_time(150, Some(150.0), 137, 100),
            Some(2),
            "1.37s birth with 150s uptime → start = epoch + 2s"
        );
    }

    /// `format_time_at` shows the time alone within a day; the date plus
    /// shorter time once the day differs, given an explicit UTC offset so the
    /// result is host-timezone independent (testable deterministically).
    #[test]
    fn test_format_time_at_same_day_vs_cross_day() {
        // `secs` = 2024-01-03 08:10:00 UTC; same-day and different-day `now`.
        let secs = 1_704_269_400;
        let now_same = 1_704_294_000; // 2024-01-03 15:00 UTC (same day)
        let now_next = 1_704_380_400; // 2024-01-04 15:00 UTC (different day)
        assert_eq!(format_time_at(secs, now_same, 0), "08:10:00");
        assert_eq!(format_time_at(secs, now_next, 0), "2024-01-03 08:10");
        // A +2h offset keeps both points on local day 01-03 (10:10 / 17:10).
        assert_eq!(format_time_at(secs, now_same, 7_200), "10:10:00");
    }
}
