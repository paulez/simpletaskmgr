# Simple Task Manager

A lightweight, interactive system process manager built with Rust that displays all running processes on your system in a user-friendly graphical interface.

## Features

- **Real-time Process Display**: Shows PID, Real User ID (RUID), and process name for each running process
- **Disk I/O Activity**: Per-process disk read/write speed (bytes per second) measured between refreshes
- **Auto-refresh**: The process list automatically updates every 1.5 seconds, in place and flicker-free (rows are re-used and re-texted, not rebuilt)
- **Sortable Columns**: Click any header (PID, User, Name, CPU%, Disk R, Disk W) to sort the list ascending/descending
- **Show All Processes**: Toggle showing every process on the system, or only the current user's (the default)
- **Modern UI**: Built with GTK 4 for a native, responsive, clean interface
- **Linux Native**: Direct access to Linux `/proc` filesystem for accurate process information
- **Process Detail View**: Click any process to inspect its PID, name, UID, user, CPU usage, and disk read/write speed
- **Signal Management**: Send SIGHUP or SIGKILL to a selected process from its detail view, with success/failure feedback
- **Resource Usage Graph**: An always-on chart above the process list shows system-wide CPU and memory usage over time (a rolling window of ~3 minutes), updating with each refresh
- **Persistent Settings**: Your choices (show-all filter, refresh interval) survive restarts. Open the **Settings** popover in the toolbar to change them.

## Requirements

- Rust 1.75 or later
- Linux operating system (uses procfs to read from `/proc`)
- GTK 4.10 or later (development headers required: `libgtk-4-dev`, `libglib2.0-dev`, `libpango1.0-dev`, `libcairo2-dev` on Debian/Ubuntu)

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

## Dependencies

The project uses the following Rust crates:

- **gtk4** 0.11 (GTK 4 bindings) - GUI framework, target feature "v4_10"
- **cairo-rs** 0.22 - Drawing library for the CPU/memory graph
- **procfs** 0.18.0 - Linux procfs filesystem bindings
- **glib** 0.22 - GLib objects & timers (via gtk4)
- **serde** 1 (with `derive`) - (De)serialization of user settings
- **toml** 0.8 - TOML encoding of the settings file
- **dirs** 5 - XDG config directory for settings storage

## How It Works

The application reads process information directly from the Linux `/proc`
filesystem using the `procfs` crate and displays it in a GTK 4 `ListView`
backed by a `gio::ListStore` and a `SingleSelection` model. Each row is a
`glib::Object` subclass (`ProcessRow`, see `src/process_row.rs`) that owns the
current `ProcessItem` snapshot for one PID.

A `glib::timeout_add_local` timer re-reads `/proc` on each refresh and updates
the store **in place** (`src/refresh_list.rs`): it diffs the old and new PID
sequence and only touches the rows that actually changed. Each process keeps
the same `ProcessRow` object for its whole life — a changed value is re-written
on that same object (and its on-screen labels re-texted in place), with **no**
store signal at all. A moved row is repositioned as the *same* object via a
single `GListStore` splice mutation (one `items-changed` signal that both
removes and re-adds the rows in the affected window). That single-signal form
is what makes GTK 4.18's `GtkListItemManager` pair the removal with the
re-addition and reparent the *existing* row widgets instead of destroying and
rebuilding them — the two-signal `remove`+`insert` form would tear each row
widget down (a blank flash). This also preserves the selection (GTK tracks it
by row identity) and the scroll position across refreshes.

CPU usage is reported in `top`-style per-core percentages: 100% means one core fully
saturated, and multi-threaded processes can show more than 100%. A process's first
sample shows its average CPU usage since it started, then switches to the
per-interval rate.

Disk read/write activity is reported per-process as a transfer speed (e.g.
`1.5 MiB/s`), computed from the delta of the `read_bytes` and `write_bytes`
counters in `/proc/[pid]/io` between refreshes. A process's first sample shows a
blank cell, since a speed needs two samples. The kernel only allows this file to be
read for a process you own (or as root), so the app only reads it for your own
processes (or for every process when run as root); other users' processes show
blank disk columns — this is a Linux restriction, not an error.

System-wide CPU and memory usage are sampled once per refresh and kept in a
rolling history (the last ~120 samples). CPU% is computed from the delta in the
aggregate `/proc/stat` `cpu` line; memory% is `(MemTotal − MemAvailable) /
MemTotal` from `/proc/meminfo`. The resource graph renders both as area charts on
a shared 0-100% axis, drawn with cairo into a `DrawingArea`.

Each process entry shows:
- **PID**: Process identifier
- **User**: Real user ID / username (the user who owns the process)
- **Name**: Process name
- **CPU%**: Per-core CPU usage for the last sample
- **Disk R**: Disk read speed since the last refresh (blank when not yet measured or not accessible)
- **Disk W**: Disk write speed since the last refresh (blank when not yet measured or not accessible)

## Settings

Settings are stored in `~/.config/simpletaskmgr/settings.toml` (or
`$XDG_CONFIG_HOME/simpletaskmgr/settings.toml`). The file is plain TOML and safe
to edit by hand:

```toml
show_all = false
refresh = "normal"
```

- `show_all` — `true` shows every process on the system, `false` (default) shows
  only the current user's
- `refresh` — polling interval: `"fast"` (0.5s), `"normal"` (1.5s, default), or
  `"slow"` (3s). Applied immediately.

Unknown or corrupt files fall back to the defaults; a missing file is created
on your first change. Change settings at runtime via the **Settings** popover
in the toolbar (with a "Reset to defaults" action); both settings apply
immediately from the point of the change.

## License

This project is open source and available under the same license as the Rust toolchain.