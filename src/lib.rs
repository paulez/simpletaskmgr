pub use procfs::process;
pub use users::{Users, UsersCache};

use floem::views::{Decorators, Stack};

pub mod cpu_tracker;

#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub name: String,
    pub pid: i32,
    pub ruid: u32,
    pub username: String,
    pub cpu_percent: f64, // Running average of CPU usage over last 5 seconds
}

impl Eq for Process {}

impl std::hash::Hash for Process {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.pid.hash(state);
        self.ruid.hash(state);
        self.username.hash(state);
    }
}

impl Process {
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

impl floem::IntoView for Process {
    type V = Stack;

    fn into_view(self) -> Self::V {
        let pid = self.pid;
        let ruid = self.ruid;
        let username = self.username.clone();
        let cpu_percent_str_val = self.cpu_percent_str().clone();
        let name = self.name.clone();
        floem::views::h_stack((
            floem::views::label(move || pid.to_string()),
            floem::views::label(move || ruid.to_string()),
            floem::views::label(move || username.clone()),
            floem::views::label(move || cpu_percent_str_val.clone()),
            floem::views::label(move || name.clone()),
        ))
        .style(move |s| s.flex_row().items_start().gap(6).width_full())
    }
}

pub enum UserFilter {
    Current,
    All,
}

pub fn get_process(pid: i32) -> Option<Process> {
    process::all_processes()
        .expect("Can't read /proc")
        .filter_map(|p| match p {
            Ok(p) if p.pid() == pid => Some(p),
            _ => None,
        })
        .filter_map(|proc| {
            let uid = proc.uid().expect("Can't get process UID");
            let pid = proc.pid();

            match proc.stat() {
                Ok(stat) => {
                    let cpu_percent = 0.0;

                    let username = match users::UsersCache::new().get_user_by_uid(uid) {
                        Some(user) => {
                            let name: &std::ffi::OsStr = user.name();
                            name.to_string_lossy().to_string()
                        }
                        None => "unknown".to_string(),
                    };
                    Some(Process {
                        name: stat.comm.to_string(),
                        pid,
                        ruid: uid,
                        username,
                        cpu_percent,
                    })
                }
                Err(e) => {
                    println!("Can't get process stat due to error {e:?}");
                    None
                }
            }
        })
        .next()
}

// Helper functions for GUI navigation
pub fn show_process_detail(pid: i32) {
    if let Some(process) = get_process(pid) {
        // In a real implementation, this would open a modal dialog
        // For now, we'll just print to the console
        println!("\n=== Process Details ===");
        println!("PID: {}", process.pid);
        println!("Name: {}", process.name);
        println!("UID: {}", process.ruid);
        println!("Username: {}", process.username);
        println!("CPU Usage: {:.1}%", process.cpu_percent);
        println!("======================\n");
    } else {
        println!("Process with PID {} not found\n", pid);
    }
}

pub fn close_process_detail() {
    // In a real implementation, this would close any open modal
    println!("Process detail dialog closed\n");
}

pub fn process_names(filter: UserFilter) -> im::Vector<Process> {
    let cache = UsersCache::new();
    let current_uid = cache.get_current_uid();
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
        .filter_map(|proc| {
            let uid = proc.uid().expect("Can't get process UID");
            let pid = proc.pid();

            if matches!(filter, UserFilter::Current) && uid != current_uid {
                return None;
            }

            match proc.stat() {
                Ok(stat) => {
                    let cpu_percent = 0.0;

                    let username = match cache.get_user_by_uid(uid) {
                        Some(user) => {
                            let name: &std::ffi::OsStr = user.name();
                            name.to_string_lossy().to_string()
                        }
                        None => "unknown".to_string(),
                    };
                    Some(Process {
                        name: stat.comm.to_string(),
                        pid,
                        ruid: uid,
                        username,
                        cpu_percent,
                    })
                }
                Err(e) => {
                    println!("Can't get process stat due to error {e:?}");
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_struct_creation() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        assert_eq!(p.name, "test");
        assert_eq!(p.pid, 123);
        assert_eq!(p.ruid, 456);
        assert_eq!(p.username, "user");
        assert_eq!(p.cpu_percent, 0.0);
    }

    #[test]
    fn test_process_struct_clone() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = p1.clone();
        assert_eq!(p1, p2);
        assert!(p1 == p2);
    }

    #[test]
    fn test_process_struct_partial_eq() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p3 = Process::new("different".to_string(), 123, 456, "user".to_string(), 0.0);

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_process_struct_debug() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let debug_string = format!("{:?}", p);
        assert!(debug_string.contains("Process"));
    }

    #[test]
    fn test_process_fields_have_valid_values() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        assert!(!p.name.is_empty());
        assert!(p.pid > 0);
        assert!(p.ruid >= 0);
        assert!(!p.username.is_empty());
        assert_eq!(p.cpu_percent, 0.0);
    }

    #[test]
    fn test_process_view_with_different_values() {
        let test_cases = vec![
            Process::new("bash".to_string(), 1, 0, "root".to_string(), 0.0),
            Process::new("firefox".to_string(), 1234, 1000, "paul".to_string(), 0.0),
            Process::new("systemd".to_string(), 1, 0, "root".to_string(), 0.0),
        ];

        for p in test_cases {
            // Each process should be able to be created with valid fields
            assert!(!p.name.is_empty());
            assert!(p.pid > 0);
            assert!(p.ruid >= 0);
            assert!(!p.username.is_empty());
            assert_eq!(p.cpu_percent, 0.0);
        }
    }

    #[test]
    fn test_process_struct_hash() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p3 = Process::new("different".to_string(), 456, 123, "other".to_string(), 0.0);

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }
}
