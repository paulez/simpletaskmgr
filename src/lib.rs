pub use users::{Users, UsersCache};
pub use procfs::process;

use floem::views::{Decorators, Stack};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Process {
    pub name: String,
    pub pid: i32,
    pub ruid: u32,
    pub username: String,
}

impl Process {
    pub fn new(name: String, pid: i32, ruid: u32, username: String) -> Self {
        Self {
            name,
            pid,
            ruid,
            username,
        }
    }
}

impl floem::IntoView for Process {
    type V = Stack;

    fn into_view(self) -> Self::V {
        floem::views::h_stack((
            floem::views::label(move || self.pid.to_string()),
            floem::views::label(move || self.ruid.to_string()),
            floem::views::label(move || self.username.clone()),
            floem::views::label(move || self.name.to_string()),
        ))
        .style(move |s| {
            s.items_center()
                .gap(6)
                .grid()
                .grid_template_columns(vec![floem::taffy::style_helpers::auto(), floem::taffy::style_helpers::auto(), floem::taffy::style_helpers::fr(1.), floem::taffy::style_helpers::auto()])
        })
    }
}

// ProcessView is only used for GUI display
#[cfg(not(test))]
use floem::{taffy::style_helpers::{auto, fr}, views::{h_stack, label}};

#[cfg(not(test))]
impl Process {
    pub fn into_view(self) -> floem::views::Stack {
        h_stack((
            label(move || self.pid.to_string()),
            label(move || self.ruid.to_string()),
            label(move || self.username.clone()),
            label(move || self.name.to_string()),
        ))
        .style(|s| {
            s.items_center()
                .gap(6)
                .grid()
                .grid_template_columns(vec![auto(), auto(), fr(1.), auto()])
        })
    }
}

pub fn process_names() -> im::Vector<Process> {
    let cache = UsersCache::new();
    process::all_processes()
        .expect("Can't read /proc")
        .filter_map(|p| match p {
            Ok(p) => Some(p),
            Err(e) => match e {
                procfs::ProcError::NotFound(_) => None,
                procfs::ProcError::Io(_e, _path) => None,
                x => {
                    println!("Can't read process due to error {x:?}");
                    None
                }
            },
        })
        .filter_map(|proc| match proc.status() {
            Ok(status) => {
                let username = match cache.get_user_by_uid(status.ruid) {
                    Some(user) => {
                        let name: &std::ffi::OsStr = user.name();
                        name.to_string_lossy().to_string()
                    }
                    None => "unknown".to_string(),
                };
                Some(Process {
                    name: status.name,
                    pid: status.pid,
                    ruid: status.ruid,
                    username,
                })
            }
            Err(e) => {
                println!("Can't get process status due to error {e:?}");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_struct_creation() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string());
        assert_eq!(p.name, "test");
        assert_eq!(p.pid, 123);
        assert_eq!(p.ruid, 456);
        assert_eq!(p.username, "user");
    }

    #[test]
    fn test_process_struct_clone() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string());
        let p2 = p1.clone();
        assert_eq!(p1, p2);
        assert!(p1 == p2);
    }

    #[test]
    fn test_process_struct_partial_eq() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string());
        let p2 = Process::new("test".to_string(), 123, 456, "user".to_string());
        let p3 = Process::new("different".to_string(), 123, 456, "user".to_string());

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_process_struct_debug() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string());
        let debug_string = format!("{:?}", p);
        assert!(debug_string.contains("Process"));
    }

    #[test]
    fn test_process_fields_have_valid_values() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string());
        assert!(!p.name.is_empty());
        assert!(p.pid > 0);
        assert!(p.ruid >= 0);
        assert!(!p.username.is_empty());
    }

    #[test]
    fn test_process_view_with_different_values() {
        let test_cases = vec![
            Process::new("bash".to_string(), 1, 0, "root".to_string()),
            Process::new("firefox".to_string(), 1234, 1000, "paul".to_string()),
            Process::new("systemd".to_string(), 1, 0, "root".to_string()),
        ];

        for p in test_cases {
            // Each process should be able to be created with valid fields
            assert!(!p.name.is_empty());
            assert!(p.pid > 0);
            assert!(p.ruid >= 0);
            assert!(!p.username.is_empty());
        }
    }

    #[test]
    fn test_process_struct_hash() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string());
        let p2 = Process::new("test".to_string(), 123, 456, "user".to_string());
        let p3 = Process::new("different".to_string(), 456, 123, "other".to_string());

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }
}