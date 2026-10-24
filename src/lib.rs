//! Simple Task Manager: a Linux process manager in Rust and GTK 4.
//!
//! The app reads processes and system metrics directly from `/proc` and
//! `sysfs` (no external tools), and shows them in a live process list with
//! resource charts and a detail pane that can signal a process.
//!
//! The crate is split by concern: data acquisition (`process`, `metrics`,
//! `cpu_status`, `disk_status`, `gpu_status`, and the `*_tracker` delta
//! calculators), list building (`process_list`), persistence (`config`,
//! `settings`), and UI (`ui`, `process_view`, `usage_graph`, `cell_label`).

pub mod cell_label;
pub mod config;
pub mod cpu_status;
pub mod cpu_tracker;
pub mod disk_status;
pub mod gpu_status;
pub mod io_tracker;
pub mod metrics;
pub mod process;
pub mod process_list;
pub mod process_view;
pub mod settings;
pub mod signal;
pub mod ui;
pub mod usage_graph;

/// Represents the column by which processes can be sorted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    /// Process id
    Pid,
    /// CPU usage percentage
    CpuPercent,
    /// Memory usage percentage (RSS / MemTotal)
    MemPercent,
    /// Process name, compared case-insensitively
    Name,
    /// Disk read speed (bytes/second)
    DiskRead,
    /// Disk write speed (bytes/second)
    DiskWrite,
}

#[cfg(test)]
mod testutil;
