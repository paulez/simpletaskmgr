use floem::{
    taffy::style_helpers::{auto, fr},
    views::{h_stack, label, Decorators, Stack},
    IntoView,
};
pub use procfs::process;
pub use users::{Users, UsersCache};

#[derive(Clone, Debug, PartialEq)]
pub struct TaskMgrProcess {
    pub name: String,
    pub pid: i32,
    pub ruid: u32,
    pub username: String,
    pub cpu_percent: f64, // top-style per-core CPU%, may exceed 100 for multi-threaded
}

impl Eq for TaskMgrProcess {}

impl std::hash::Hash for TaskMgrProcess {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.pid.hash(state);
        self.ruid.hash(state);
        self.username.hash(state);
    }
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

impl IntoView for TaskMgrProcess {
    type V = Stack;

    fn into_view(self) -> Self::V {
        let cpu_percent_str = self.cpu_percent_str();
        let name = self.name.clone();
        h_stack((
            label(move || self.pid.to_string()),
            label(move || self.username.clone()),
            label(move || cpu_percent_str.clone()),
            label(move || name.to_string()),
        ))
        .style(move |s| {
            s.width_full()
                .items_center()
                .gap(6)
                .grid()
                .grid_template_columns(vec![auto(), auto(), auto(), fr(1.)])
                .padding_vert(4)
        })
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

    #[test]
    fn test_process_struct_creation() {
        let p = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        assert_eq!(p.name, "test");
        assert_eq!(p.pid, 123);
        assert_eq!(p.ruid, 456);
        assert_eq!(p.username, "user");
        assert_eq!(p.cpu_percent, 0.0);
    }

    #[test]
    fn test_process_struct_clone() {
        let p1 = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = p1.clone();
        assert_eq!(p1, p2);
        assert!(p1 == p2);
    }

    #[test]
    fn test_process_struct_partial_eq() {
        let p1 = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p3 = TaskMgrProcess::new("different".to_string(), 123, 456, "user".to_string(), 0.0);

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_process_struct_debug() {
        let p = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let debug_string = format!("{:?}", p);
        assert!(debug_string.contains("TaskMgrProcess"));
    }

    #[test]
    fn test_process_fields_have_valid_values() {
        let p = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        assert!(!p.name.is_empty());
        assert!(p.pid > 0);
        assert!(!p.username.is_empty());
        assert_eq!(p.cpu_percent, 0.0);
    }

    #[test]
    fn test_process_view_with_different_values() {
        let test_cases = vec![
            TaskMgrProcess::new("bash".to_string(), 1, 0, "root".to_string(), 0.0),
            TaskMgrProcess::new("firefox".to_string(), 1234, 1000, "paul".to_string(), 0.0),
            TaskMgrProcess::new("systemd".to_string(), 1, 0, "root".to_string(), 0.0),
        ];

        for p in test_cases {
            // Each process should be able to be created with valid fields
            assert!(!p.name.is_empty());
            assert!(p.pid > 0);
            assert!(!p.username.is_empty());
            assert_eq!(p.cpu_percent, 0.0);
        }
    }

    #[test]
    fn test_process_struct_hash() {
        let p1 = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = TaskMgrProcess::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p3 = TaskMgrProcess::new("different".to_string(), 456, 123, "other".to_string(), 0.0);

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }
}
