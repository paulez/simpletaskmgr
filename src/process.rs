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
}

impl TaskMgrProcess {
    pub fn new(name: String, pid: i32, ruid: u32, username: String, cpu_percent: f64) -> Self {
        Self {
            name,
            pid,
            ruid,
            username,
            cpu_percent,
        }
    }

    pub fn cpu_percent_str(&self) -> String {
        format!("{:.1}%", self.cpu_percent)
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
