# Simple Task Manager

A lightweight Linux process manager in Rust and GTK 4 — like a small,
simple `top` with a GUI: a live process list, system resource charts, and
Terminate/Kill buttons. It reads everything from `/proc`, so there are no
external dependencies at runtime.

![Screenshot](screenshots/screenshot_2.5s.png)

## Features

- **Process list** — PID, name, CPU%, MEM%, and per-process disk read/write,
  refreshed in place. Sorted by CPU% by default; click any column header to
  sort by it.
- **Stable display** — rows update in place and only reorder when the visible
  order actually changes, so the list doesn't flicker or jump on every tick.
- **Detail pane** — select a process to see its full command line, state,
  threads, nice value, RSS, and start time. Long values are ellipsized; click
  the Name or Command row to select and copy the full text.
- **Terminate / Kill** — send SIGTERM (graceful) or SIGKILL (forced) to the
  selected process, with the result reported in the pane. If the process has
  exited and its PID was reused, the signal is not sent to the new occupant.
- **Charts** — system CPU, memory, CPU frequency and temperature over the
  last few minutes, with legends.
- **Disk tab** — per-disk read/write throughput and utilization, read from
  `/proc/diskstats` (no `iostat` needed).
- **GPU tab** — AMD GPU utilization, VRAM and temperature via the `amdgpu`
  sysfs interface (`rocm-smi` as fallback). Hidden when no GPU is present.
- **Show all / current user only** — toggle the process filter.
- **Persistent settings** — filter and refresh interval are saved across
  restarts.

## Requirements

- Linux (uses `/proc`)
- Rust 1.75+
- GTK 4.18+ with development headers
  (`libgtk-4-dev`, `libglib2.0-dev`, `libpango1.0-dev`, `libcairo2-dev` on Debian/Ubuntu)
- Optional, for the GPU tab: an AMD GPU on the `amdgpu` driver, or
  `rocm-smi` on `PATH`

## Building and running

```bash
cargo build --release
cargo run --release
```

Pass `-v` for verbose (debug) logging; normal runs are quiet.

## Installing

Grab a prebuilt artifact from the
[releases](https://github.com/paulez/simpletaskmgr/releases):

- **AppImage** — self-contained, bundles the GTK 4 runtime. `chmod +x` and
  run. On systems without FUSE: `./simpletaskmgr-*.AppImage --appimage-extract-and-run`
- **Tarball** — bare binary, for systems that already have GTK 4.18+

You can also install from crates.io (compiles from source, so the build
dependencies above are required):

```bash
cargo install simpletaskmgr
```

Pre-releases need an explicit version: `cargo install simpletaskmgr --version 1.0.0-rc.1`.

## Settings

The Settings popover in the toolbar controls the process filter and refresh
interval (fast 0.5s / normal 1.5s / slow 3s); both are persisted to
`~/.config/simpletaskmgr/settings.toml`:

```toml
show_all = false
refresh = "normal"
```

## Development

Every push runs format, clippy, and tests via the `rust` workflow.
Releases are cut by tagging a version (`git tag v1.0.0 && git push --tags`);
the `release` workflow builds the AppImage and tarball and publishes to
crates.io. See [doc/CRATES_IO_PUBLISH.md](doc/CRATES_IO_PUBLISH.md) for
manual publishing steps.

## License

MIT OR Apache-2.0
