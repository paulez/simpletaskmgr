# Simpletaskmgr — Features

A real-time Linux process manager (Rust / GTK 4):

- Process list: PID, user, name, CPU%, MEM%, and per-process disk read/write;
  auto-refreshing in place.
- Click-to-sort columns; numeric columns default highest-first.
- Show all processes or only the current user's (default).
- Detail pane on selection: live metrics plus SIGHUP / SIGKILL.
- Charts of system CPU & memory, and CPU frequency & temperature (rolling history).
- Persistent settings (filter + refresh interval) in a TOML file.
