use simpletaskmgr::{process_names, UserFilter};

#[test]
fn test_process_names_returns_vector() {
    let processes = process_names(UserFilter::All);
    assert!(!processes.is_empty());
}

#[test]
fn test_process_names_contains_expected_fields() {
    let processes = process_names(UserFilter::All);
    for process in processes.iter() {
        assert!(!process.name.is_empty());
        assert!(process.pid > 0);
        assert!(process.ruid >= 0);
        assert!(!process.username.is_empty());
    }
}

#[test]
fn test_process_names_has_unique_pids() {
    let processes = process_names(UserFilter::All);
    let mut pids = std::collections::HashSet::new();
    for process in processes.iter() {
        assert!(pids.insert(process.pid), "Duplicate PID found: {}", process.pid);
    }
}

#[test]
fn test_process_names_struct_fields_accessible() {
    let processes = process_names(UserFilter::All);
    if let Some(process) = processes.get(0) {
        let _name: String = process.name.clone();
        let _pid: i32 = process.pid;
        let _ruid: u32 = process.ruid;
        let _username: String = process.username.clone();
    }
}

#[test]
fn test_process_names_handles_missing_users() {
    // This test verifies that processes with non-existent users still work
    let processes = process_names(UserFilter::All);
    for process in processes.iter() {
        // Even if username is "unknown", it's still a valid result
        if process.username == "unknown" {
            assert!(!process.name.is_empty());
        }
    }
}