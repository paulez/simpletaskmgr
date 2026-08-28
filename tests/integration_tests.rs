use log::LevelFilter;
use rstest::*;
use simpletaskmgr::{process::TaskMgrProcess, process_list::ProcessList};

#[cfg(test)]
mod tests {
    use super::*;

    // Initialize logger for all tests
    fn init_log() {
        let _ = simplelog::SimpleLogger::init(
            LevelFilter::Debug,
            simplelog::ConfigBuilder::new()
                .add_filter_allow_str("simpletaskmgr")
                .build(),
        );
    }

    #[rstest]
    fn test_process_names_returns_vector(all_processes: Vec<TaskMgrProcess>) {
        init_log();
        assert!(!all_processes.is_empty());
    }

    #[rstest]
    fn test_process_names_contains_expected_fields(all_processes: Vec<TaskMgrProcess>) {
        for process in all_processes.iter() {
            assert!(!process.name.is_empty());
            assert!(process.pid > 0);
            assert!(!process.username.is_empty());
        }
    }

    #[rstest]
    fn test_process_names_has_unique_pids(all_processes: Vec<TaskMgrProcess>) {
        let mut pids = std::collections::HashSet::new();
        for process in all_processes.iter() {
            assert!(
                pids.insert(process.pid),
                "Duplicate PID found: {}",
                process.pid
            );
        }
    }

    #[fixture]
    fn all_processes() -> Vec<TaskMgrProcess> {
        init_log();
        let mut process_list = ProcessList::new();
        process_list
            .refresh_process_list()
            .expect("Failed to get process list")
    }
}
