pub mod config;
pub mod cpu_tracker;
pub mod process;
pub mod process_list;
pub mod signal;
pub mod ui;

/// Represents the column by which processes can be sorted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Pid,
    Username,
    CpuPercent,
    Name,
}

/// Represents the sorting direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}
