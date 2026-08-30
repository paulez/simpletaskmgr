pub mod config;
pub mod cpu_status;
pub mod cpu_tracker;
pub mod io_tracker;
pub mod metrics;
pub mod process;
pub mod process_list;
pub mod process_row;
pub mod refresh_list;
pub mod settings;
pub mod signal;
pub mod ui;
pub mod usage_graph;

/// Represents the column by which processes can be sorted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Pid,
    Username,
    CpuPercent,
    MemPercent,
    Name,
    DiskRead,
    DiskWrite,
}

#[cfg(test)]
mod testutil;
