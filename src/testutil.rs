//! Shared `cfg(test)` helpers so the test modules don't each redeclare the
//! same `TaskMgrProcess`/`ProcessItem` fixtures.

use crate::process::{ProcessItem, TaskMgrProcess};

/// Builds a `TaskMgrProcess` with a conventional name, uid, and username for
/// a given `pid` and `cpu_percent` (no disk rates).
pub(crate) fn test_process(pid: i32, cpu: f64) -> TaskMgrProcess {
    TaskMgrProcess::new(format!("name{pid}"), pid, 1000, "paul".to_string(), cpu)
}

/// Builds a `ProcessItem` wrapping [`test_process`] (cpu 1.0, no disk rates).
pub(crate) fn test_item(pid: i32) -> ProcessItem {
    ProcessItem::new(&test_process(pid, 1.0))
}
