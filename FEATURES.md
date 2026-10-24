# Simpletaskmgr — Features

A real-time Linux process manager (Rust / GTK 4):

- Process list: PID, user, name, CPU%, MEM%, and per-process disk read/write;
  auto-refreshing in place.
- Click-to-sort columns; numeric columns default highest-first.
- Show all processes or only the current user's (default).
- Detail pane on selection: live metrics plus Terminate (SIGTERM) and
  Kill (SIGKILL) buttons.
- Charts of system CPU & memory, CPU frequency & temperature, per-disk I/O,
  and AMD GPU stats (rolling history, tabbed).
- Persistent settings (filter + refresh interval) in a TOML file.
