use std::collections::HashMap;

use crate::Process;

pub fn build_process_map() -> HashMap<i32, Process> {
    let mut process_map: HashMap<i32, Process> = HashMap::new();

    if let Ok(all_processes) = process::all_processes() {
        for proc_result in all_processes {
            if let Ok(proc) = proc_result {
                let process = Process {
                    name: "".to_string(),
                    pid: proc.pid(),
                    ruid: 0,
                    username: "".to_string(),
                    cpu_percent: 0.0,
                };

                process_map.insert(proc.pid(), process);
            }
        }
    }

    process_map
}
            }
        }
    }

    process_map
}

pub fn build_process_map_with_details(
    process_map: &HashMap<i32, Process>,
) -> HashMap<i32, Process> {
    let mut process_map = process_map.clone();

    if let Ok(all_processes) = process::all_processes() {
        for proc_result in all_processes {
            if let Ok(proc) = proc_result {
                if let Ok(stat) = proc.stat() {
                    if let Some(uid) = process_map.get_mut(&proc.pid()) {
                        uid.ruid = stat.uid();

                        if let Some(user) = users::get_user_by_uid(stat.uid()) {
                            uid.username = user.name().to_string_lossy().to_string();
                        }
                    }
                }
            }
        }
    }

    process_map
}
