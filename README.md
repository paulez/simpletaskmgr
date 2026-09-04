# Simple Task Manager

A lightweight, interactive system process manager built with Rust that displays all running processes on your system in a user-friendly graphical interface.

## Features

- **Real-time Process Display**: Shows PID, Real User ID (RUID), and process name for each running process
- **Disk I/O Activity**: Per-process disk read/write speed (bytes per second) measured between refreshes
- **Auto-refresh**: The process list automatically updates every 1.5 seconds, in place and flicker-free (rows are re-used and re-texted, not rebuilt)
- **Sortable Columns**: Click any header (PID, User, Name, CPU%, MEM%, Disk R, Disk W) to sort the list ascending/descending. The numeric columns (CPU%, MEM%, Disk R, Disk W) default to descending — highest value first — on their first click, so the most resource-hungry processes lead; click again to toggle direction. Text columns (PID, User, Name) default to ascending.
- **Show All Processes**: Toggle showing every process on the system, or only the current user's (the default)
- **Modern UI**: Built with GTK 4 for a native, responsive, clean interface
- **Linux Native**: Direct access to Linux `/proc` filesystem for accurate process information
- **Process Detail View**: Click any process to inspect its PID, name, UID, user, CPU usage, and disk read/write speed in the right-hand pane; the pane (and its signal buttons) appears only while a process is selected and the process list uses the full width when none is selected
- **Clean Start**: On launch no process is selected, so the list takes the full width and the detail pane is hidden until you pick one
- **Highest CPU First**: On launch the list is sorted by CPU% (highest first) and scrolled to the top, so the most active processes are visible immediately
- **Signal Management**: Send SIGHUP or SIGKILL to a selected process from its detail view, with success/failure feedback
 - **Resource Usage Graph**: An always-on chart above the process list shows system-wide CPU and memory usage (left pane, "CPU & Memory") and CPU frequency and temperature (right pane, "CPU Freq & Temp") over time, updating with each refresh. Each pane's title is a **live readout** of the newest sample with a `Cpu`/`Mem` (left) or `Freq`/`Temp` (right) label before every value (e.g. `Cpu 5.1% · Mem 18.1 GB` and `Freq 3.2 GHz · Temp 44 °C`), so the current values stay visible even on a low or flat line; a missing sensor reads as `-` rather than `0`. The two panes share one rolling history (~120 samples at the default 1.5 s refresh, ~3 minutes); the trace **grows left-to-right**, filling the full pane width over its first ~7 samples (~10 s), then eases to a steady slot that holds the full ~3-minute history before the oldest samples start scrolling off the left edge — the warm-up never instantly stretches a few points across the width, which reads as noise. Faint 25/50/75% gridlines and a baseline sit behind the bolder colored series, and each series keeps its own tick-labeled axis — the left pane reads CPU (0-100%, left axis) and RAM (MB, right axis, top read from `/proc/meminfo` `MemTotal`, e.g. `16 GB`); the right pane reads frequency (MHz, top read from `scaling_max_freq`, e.g. `4 GHz`) on the left and temperature (°C, 0-100) on the right — so a CPU trace at 60% and a RAM trace at 10 GB on a 16 GB ceiling each read as 60% and 63% of the pane height. Tick labels live in 10pt gutters outside the plot so the colored series never overlap them and the top label is never clipped by the pane edge
- **Persistent Settings**: Your choices (show-all filter, refresh interval) survive restarts. Open the **Settings** popover in the toolbar to change them.

## Requirements

- Rust 1.75 or later
- Linux operating system (uses procfs to read from `/proc`)
- GTK 4.18 or later (development headers required: `libgtk-4-dev`, `libglib2.0-dev`, `libpango1.0-dev`, `libcairo2-dev` on Debian/Ubuntu)

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

## Dependencies

The project uses the following Rust crates:

- **gtk4** 0.11 (GTK 4 bindings) - GUI framework, target feature "v4_18"
- **cairo-rs** 0.22 - Drawing library for the CPU/memory graph
- **procfs** 0.18.0 - Linux procfs filesystem bindings
- **glib** 0.22 - GLib objects & timers (via gtk4)
- **serde** 1 (with `derive`) - (De)serialization of user settings
- **toml** 0.8 - TOML encoding of the settings file
- **dirs** 5 - XDG config directory for settings storage

## How It Works

The application reads process information directly from the Linux `/proc`
filesystem using the `procfs` crate and displays it in a GTK 4 `ColumnView`
backed by a `gio::ListStore` and a `SingleSelection` model. Each row is a
`glib::Object` subclass (`ProcessRow`, see `src/process_row.rs`) that owns the
current `ProcessItem` snapshot for one PID; each column's cell is a plain
`Label` property-bound to one of the row's read-only string properties
(`pid`, `username`, `name`, `cpu`, `disk-read`, `disk-write`).

A `glib::timeout_add_local` timer re-reads `/proc` on each refresh and updates
the store **in place** (`src/refresh_list.rs`): it diffs the old and new PID
sequence and only touches the rows that actually changed. Each process keeps
the same `ProcessRow` object for its whole life — a changed value is re-written
on that same object, which only re-emits the changed properties, so the
bound cell labels update via `g_object_notify` with no store signal at all. A
moved row is repositioned as the *same* object via a single `GListStore`
splice mutation (one `items-changed` signal that both removes and re-adds the
rows in the affected window). That single-signal form is what makes GTK 4.18's
`GtkListItemManager` pair the removal with the re-addition and reparent the
*existing* row widgets instead of destroying and rebuilding them (a blank
flash). This also preserves the selection (GTK tracks it by row identity) and
the scroll position across refreshes.

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
aggregate `/proc/stat` `cpu` line; memory is `(MemTotal − MemAvailable) /
MemTotal` from `/proc/meminfo`, and `MemTotal` is also read up front as the
top of the RAM axis (in MB) so the RAM trace reads as full-height against the
installed memory rather than as a share of a fixed 100%.

CPU frequency and temperature are sampled on the same schedule and added to
each sample. Frequency is read from
`/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq` (kilohertz, converted
to MHz) — core 0 is the representative "current CPU frequency" source.
Temperature is read from the CPU's `hwmon` sensor (discovered by name: `cpu`,
`coretemp`, `k10temp`, `zenpower`, non-CPU sensors like `amdgpu`/`nvme` are
excluded), taking the hottest `tempN_input` reading in millidegrees. Each of
these degrades gracefully to `None` when the source is absent (containers, VMs,
or drivers not exposed); a momentary read failure carries the previous value
forward so the graph doesn't dip to zero spuriously. The resource graph is
split into two side-by-side panes that share one rolling history: the left
pane draws CPU (green) on a 0-100% left axis and RAM (blue) in MB on a right
axis whose top is the machine's total from `/proc/meminfo` — the RAM value is
converted from the sampled percent against `MemTotal` at paint time — so a
busy system and a full memory ceiling each read against their own realistic
scale rather than being squeezed onto one shared axis. The right pane draws
frequency (purple) on a dedicated left axis anchored at `scaling_max_freq`
(defaulting to 4 GHz if that interface isn't available) and temperature (red)
on its own 0-100 °C right axis. Each pane is wrapped in a vertical box with a short centered `Label`
on top ("CPU & Memory" / "CPU Freq & Temp") and the cairo `DrawingArea`
below it; the label uses the GTK theme's font and color so it stays readable
in both light and dark themes. The cairo layer also reserves a small top
band (`TOP_PAD`) so the topmost tick label is never clipped by the pane
edge, and the tick labels are drawn in gutters outside the plot so the
colored series never overlap them. Each pane is right-anchored and
fixed-slot: the newest sample sits at the right edge, each older sample one
slot to the left, and the left side stays blank until the history fills the
full width — then new samples scroll the trace left, exactly like a
scrolling system monitor.

Each process entry shows:
- **PID**: Process identifier
- **User**: Real user ID / username (the user who owns the process)
- **Name**: Process name
- **CPU%**: Per-core CPU usage for the last sample
- **MEM%**: Share of system memory used by the process (`VmRSS` from `/proc/[pid]/status` divided by `MemTotal` from `/proc/meminfo`, like `top`'s RES/MEM%); blank when the process's `VmRSS` or the system's `MemTotal` couldn't be read
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