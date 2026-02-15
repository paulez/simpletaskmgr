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
