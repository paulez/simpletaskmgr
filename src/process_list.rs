use crate::cpu_tracker::CpuTracker;
use crate::process::Process;
use floem::prelude::{create_rw_signal, RwSignal, SignalUpdate};
use imbl::Vector;
use log::debug;
use procfs::process;
use std::cell::{LazyCell, RefCell};
use users::{Users, UsersCache};

thread_local! {
    pub static PROCESS_LIST: LazyCell<ProcessList> = LazyCell::new(|| {
        ProcessList::new()
    })
}

pub enum UserFilter {
    Current,
    All,
}

pub struct ProcessList {
    pub processes: RwSignal<Vector<Process>>,
    cpu_tracker: RefCell<CpuTracker>,
    users_cache: UsersCache,
}

impl Default for ProcessList {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessList {
    pub fn new() -> Self {
        let users_cache = UsersCache::new();
        let processes = create_rw_signal(Self::process_names(&users_cache, UserFilter::Current));
        let cpu_tracker = RefCell::new(CpuTracker::new());
        Self {
            processes,
            cpu_tracker,
            users_cache,
        }
    }

    pub fn update_process_list(&self) {
        self.processes.set(self.process_list());
    }

    fn process_list(&self) -> Vector<Process> {
        debug!("Refreshing process list");
        // Get process list using process_names() from lib.rs
        let processes = Self::process_names(&self.users_cache, UserFilter::Current);

        // Update CPU usage for each process
        let mut process_map: std::collections::HashMap<i32, Process> = processes
            .iter()
            .map(|p: &Process| (p.pid, p.clone()))
            .collect();
        self.cpu_tracker
            .borrow_mut()
            .update_process_cpu_usage(&mut process_map);

        // Convert back to vector
        let mut processes: Vector<Process> = process_map.values().cloned().collect();

        // Sort by CPU usage (highest first)
        processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

        processes
    }

    pub fn process_names(users_cache: &UsersCache, filter: UserFilter) -> Vector<Process> {
        let current_uid = users_cache.get_current_uid();

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

                        let username = match users_cache.get_user_by_uid(uid) {
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
}
