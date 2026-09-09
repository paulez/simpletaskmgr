use log::LevelFilter;
use rstest::*;
use simpletaskmgr::{
    gpu_status::{gpu_available, read_gpu_card},
    process::TaskMgrProcess,
    process_list::ProcessList,
};

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

    /// Live `rocm-smi` smoke test — kept out of the lib unit tests (see
    /// `doc/TEST_FD_LIMIT_BUG.md`) because a unit test that shells out to an
    /// external binary must not run in the parallel test pool. This runs in
    /// the integration binary, where only one `rocm-smi` spawn is in flight
    /// at a time, so it cannot saturate the process open-file budget.
    ///
    /// On a host with an AMD GPU the reading is present and physically sane;
    /// on a host without one (`rocm-smi` absent or no `card0`) it degrades to
    /// `None` — both acceptable. A `Some` reading must be in the 0–100% band
    /// and a positive (but not ridiculous) temperature.
    #[test]
    fn test_read_gpu_card_sane_when_present() {
        init_log();
        let available = gpu_available();
        if let Some(s) = read_gpu_card() {
            assert!(available, "a `Some` reading implies the GPU is available");
            assert!((0.0..=100.0).contains(&s.use_pct), "use_pct in 0–100");
            assert!((0.0..=100.0).contains(&s.vram_pct), "vram_pct in 0–100");
            assert!(s.temp_c > 0.0, "a present GPU temp must be positive");
            assert!(
                s.temp_c < 120.0,
                "a GPU temp above 120 °C indicates a parsing bug"
            );
        }
    }
}
