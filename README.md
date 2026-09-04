# Simple Task Manager

A lightweight, interactive Linux process manager built with Rust and a GTK 4
interface. It lists running processes, tracks their resource usage in real
time, and lets you inspect and signal individual processes.

## Features

- **Process list**: PID, user (RUID), name, CPU%, MEM%, and per-process disk
  read/write speed; auto-refreshes in place, flicker-free.
- **Sortable columns**: click any header to sort; numeric columns default
  highest-first.
- **Show all processes**: toggle between every process on the system and only
  the current user's (default).
- **Detail view**: click a process to see live metrics in a side pane and send
  SIGHUP or SIGKILL to it.
- **Resource graph**: always-on charts of system CPU & memory, and CPU
  frequency & temperature, over the last few minutes.
- **GPU tab**: when an AMD GPU is present and readable via `rocm-smi`,
  a tab switch above the charts shows GPU utilization, VRAM allocation,
  and edge temperature alongside the CPU readings (the tab is hidden
  entirely when no GPU is detected).
- **Persistent settings**: filter and refresh interval survive restarts
  (Settings popover).
- **Native & light**: GTK 4 UI; reads information directly from `/proc`.

## Requirements

- Rust 1.75 or later
- Linux operating system (uses `/proc`)
- GTK 4.18 or later (development headers required: `libgtk-4-dev`,
  `libglib2.0-dev`, `libpango1.0-dev`, `libcairo2-dev` on Debian/Ubuntu)
- *(optional, for the GPU tab)* `rocm-smi` on `PATH` and an AMD GPU.
  Without it the GPU tab simply doesn't appear; the rest of the app
  works unchanged.

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --release
```

Or run with cargo directly:

```bash
cargo run
```

### Logging

By default the app logs at **warning** level and above, so normal runs print
nothing to stdout. To enable verbose (`debug`) logging for troubleshooting,
pass `-v`:

```bash
cargo run --release -- -v
```

## Settings

Settings are stored in `~/.config/simpletaskmgr/settings.toml` (or
`$XDG_CONFIG_HOME/simpletaskmgr/settings.toml`). The file is plain TOML and
safe to edit by hand:

```toml
show_all = false
refresh = "normal"
```

- `show_all` — `true` shows every process on the system, `false` (default)
  shows only the current user's
- `refresh` — polling interval: `"fast"` (0.5s), `"normal"` (1.5s, default),
  or `"slow"` (3s).

Unknown or corrupt files fall back to the defaults; a missing file is created
on your first change. Change settings at runtime via the **Settings** popover
in the toolbar (with a "Reset to defaults" action).

## License

MIT OR Apache-2.0
