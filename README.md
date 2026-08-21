# Simple Task Manager

A lightweight, interactive system process manager built with Rust that displays all running processes on your system in a user-friendly graphical interface.

## Features

- **Real-time Process Display**: Shows PID, Real User ID (RUID), and process name for each running process
- **Auto-refresh**: The process list automatically updates every 1.5 seconds
- **Sortable Columns**: Click any header (PID, User, Name, CPU%) to sort the list ascending/descending
- **Modern UI**: Built with GTK 4 for a native, responsive, clean interface
- **Linux Native**: Direct access to Linux `/proc` filesystem for accurate process information
- **Process Detail View**: Click any process to inspect its PID, name, UID, user, and CPU usage
- **Signal Management**: Send SIGHUP or SIGKILL to a selected process from its detail view, with success/failure feedback
- **Resource Usage Graph**: An always-on chart above the process list shows system-wide CPU and memory usage over time (a rolling window of ~3 minutes), updating with each refresh

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

## How It Works

The application reads process information directly from the Linux `/proc`
filesystem using the `procfs` crate and displays it in a GTK 4 `ListBox`. A
`glib::timeout_add_local` timer re-reads `/proc` every 1.5 seconds and rebuilds
the list.

CPU usage is reported in `top`-style per-core percentages: 100% means one core fully
saturated, and multi-threaded processes can show more than 100%. A process's first
sample shows its average CPU usage since it started, then switches to the
per-interval rate.

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

## License

This project is open source and available under the same license as the Rust toolchain.