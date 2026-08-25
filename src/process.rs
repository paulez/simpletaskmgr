pub use procfs::process;
pub use users::{Users, UsersCache};

/// A plain, immutable snapshot of a process row's data as read from `/proc`.
///
/// `PartialEq` compares the whole struct field-by-field. `cpu_percent` is an
/// `f64`, so `Eq`/`Hash` are intentionally not implemented (an `f64` that can
/// be `NaN` has a `PartialEq` that is not reflexive, and no caller of this repo
/// needs it as a hash key anyway — the list keys rows on the stable `i32`
/// `pid`, not on the value).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskMgrProcess {
    pub name: String,
    pub pid: i32,
    pub ruid: u32,
    pub username: String,
    pub cpu_percent: f64, // top-style per-core CPU%, may exceed 100 for multi-threaded
    /// Disk read speed in bytes/second, or `None` when no rate is known yet
    /// (first sample of a newly tracked process, or no permission to read
    /// `/proc/[pid]/io`).
    pub disk_read_speed: Option<f64>,
    /// Disk write speed in bytes/second, or `None` when no rate is known yet.
    pub disk_write_speed: Option<f64>,
}

impl TaskMgrProcess {
    pub fn new(name: String, pid: i32, ruid: u32, username: String, cpu_percent: f64) -> Self {
        Self {
            name,
            pid,
            ruid,
            username,
            cpu_percent,
            disk_read_speed: None,
            disk_write_speed: None,
        }
    }

    pub fn cpu_percent_str(&self) -> String {
        format!("{:.1}%", self.cpu_percent)
    }

    /// The disk read speed formatted like `"12.3 KiB/s"`, or `""` when unknown.
    pub fn disk_read_str(&self) -> String {
        format_disk_speed(self.disk_read_speed)
    }

    /// The disk write speed formatted like `"456 B/s"`, or `""` when unknown.
    pub fn disk_write_str(&self) -> String {
        format_disk_speed(self.disk_write_speed)
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
        disk_read_speed: None,
        disk_write_speed: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i32, cpu: f64) -> TaskMgrProcess {
        TaskMgrProcess::new(format!("name{}", pid), pid, 1000, "paul".to_string(), cpu)
    }

    #[test]
    fn test_process_struct_creation() {
        let p = proc(123, 0.0);
        assert_eq!(p.name, "name123");
        assert_eq!(p.pid, 123);
        assert_eq!(p.ruid, 1000);
        assert_eq!(p.username, "paul");
        assert_eq!(p.cpu_percent, 0.0);
    }

    #[test]
    fn test_process_struct_clone_and_eq() {
        let a = proc(1, 5.0);
        let b = a.clone();
        assert_eq!(a, b);
        assert!(a == b);
    }

    #[test]
    fn test_process_struct_partial_eq() {
        let a = proc(1, 5.0);
        let same = proc(1, 5.0);
        let diff = TaskMgrProcess::new("other".to_string(), 1, 1000, "paul".to_string(), 5.0);
        assert_eq!(a, same);
        assert_ne!(a, diff);
    }

    #[test]
    fn test_process_struct_debug() {
        let p = proc(123, 0.0);
        assert!(format!("{:?}", p).contains("TaskMgrProcess"));
    }

    #[test]
    fn test_task_mgr_process_cpu_percent_str() {
        assert_eq!(proc(1, 0.0).cpu_percent_str(), "0.0%");
        assert_eq!(proc(1, 12.34).cpu_percent_str(), "12.3%");
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

    #[test]
    fn test_format_disk_speed_units() {
        assert_eq!(format_disk_speed(None), "");
        assert_eq!(format_disk_speed(Some(0.0)), "0 B/s");
        assert_eq!(format_disk_speed(Some(999.0)), "999 B/s");
        assert_eq!(format_disk_speed(Some(1024.0 * 12.3)), "12.3 KiB/s");
        assert_eq!(
            format_disk_speed(Some(1024.0f64.powi(2) * 512.5)),
            "512.5 MiB/s"
        );
        assert_eq!(
            format_disk_speed(Some(1024.0f64.powi(3) * 9.99)),
            "10.0 GiB/s"
        );
        assert_eq!(
            format_disk_speed(Some(1024.0f64.powi(4) * 3.0)),
            "3.0 TiB/s"
        );
        // Above the largest unit: value keeps growing, unit stays TiB/s.
        assert_eq!(
            format_disk_speed(Some(1024.0f64.powi(5) * 3.0)),
            "3072.0 TiB/s"
        );
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
}
