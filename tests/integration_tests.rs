use imbl::Vector;
use rstest::*;
use simpletaskmgr::{process::TaskMgrProcess, process_list::ProcessList};
use users::UsersCache;

#[rstest]
fn test_process_names_returns_vector(all_processes: Vector<TaskMgrProcess>) {
    assert!(!all_processes.is_empty());
}

#[rstest]
fn test_process_names_contains_expected_fields(all_processes: Vector<TaskMgrProcess>) {
    for process in all_processes.iter() {
        assert!(!process.name.is_empty());
        assert!(process.pid > 0);
        assert!(process.ruid >= 0);
        assert!(!process.username.is_empty());
    }
}

#[rstest]
fn test_process_names_has_unique_pids(all_processes: Vector<TaskMgrProcess>) {
    let mut pids = std::collections::HashSet::new();
    for process in all_processes.iter() {
        assert!(
            pids.insert(process.pid),
            "Duplicate PID found: {}",
            process.pid
        );
    }
}

#[rstest]
fn test_process_names_struct_fields_accessible(all_processes: Vector<TaskMgrProcess>) {
    if let Some(process) = all_processes.get(0) {
        let _name: String = process.name.clone();
        let _pid: i32 = process.pid;
        let _ruid: u32 = process.ruid;
        let _username: String = process.username.clone();
    }
}

#[rstest]
fn test_process_names_handles_missing_users(all_processes: Vector<TaskMgrProcess>) {
    // This test verifies that processes with non-existent users still work
    for process in all_processes.iter() {
        // Even if username is "unknown", it's still a valid result
        if process.username == "unknown" {
            assert!(!process.name.is_empty());
        }
    }
}

#[fixture]
fn all_processes() -> Vector<TaskMgrProcess> {
    let users_cache = UsersCache::new();
    ProcessList::process_names(&users_cache, true).expect("Failed to get process list")
}
