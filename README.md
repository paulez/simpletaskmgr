# Simple Task Manager

A lightweight, interactive Linux process manager built with Rust and a GTK 4
interface. It lists running processes, tracks their resource usage in real
time, and lets you inspect individual processes in a live detail pane.

## Features

- **Process list**: PID, name, CPU%, MEM%, and per-process disk read/write
  speed; auto-refreshes in place, flicker-free. The name is the
  full program name (`argv[0]`), not truncated to the kernel's 15-char `comm`.
- **Sortable columns**: the list opens sorted by CPU%, heaviest first
  (`top`-style); click any other header to sort by that column (click
  again to flip the direction). Clicking a header also scrolls the list
  back to the top, so the new first row (e.g. the heaviest-MEM process
  after clicking MEM%) is visible right away. Rows with no value for a
  column (blank MEM%/Disk) always stay at the bottom, in either sort
  direction.
- **Calm CPU ordering**: the CPU% column orders rows by each process's integer
  tick delta of the sample window (ties settle by PID, in `top`'s style), so
  rows that measure equal — including rows that display the same value — keep
  their position across refreshes instead of swapping back and forth. The
  percentage shown is the live per-interval value.
- **Stable refresh**: on every refresh the values update in place and a
  re-sort is only committed when the visible order under the active column
  actually changes. If the values moved but the order is the same, the rows
  keep their place and the list commits no reflow, so it stays pixel-stable
  (even scrolled down the list) instead of flickering on every tick.
- **Show all processes**: toggle between every process on the system and only
  the current user's (default).
- **Detail view**: click a process to see its live metrics in a side pane
  (PID, name, CPU%, MEM%, and disk read/write speed).
- **Resource graph**: always-on charts of system CPU & memory, and CPU
  frequency & temperature, over the last few minutes. Every pane carries a
  colour swatch + label legend below its plot so each series is identified
  without ambiguity.
- **Disk I/O tab**: a tab switch above the charts shows each physical disk's
  read + write throughput and utilization (`%util`), sampled directly from
  `/proc/diskstats` — no `iostat`/`sysstat` dependency. Partitions and virtual
  devices are filtered out, and on a host with no physical disk the tab simply
  reads empty rather than erroring.
- **GPU tab**: when an AMD GPU is present and readable via `rocm-smi`,
  a tab switch above the charts shows GPU utilization, VRAM allocation,
  and edge temperature alongside the CPU readings (the tab is hidden
  entirely when no GPU is detected).
- **Persistent settings**: filter and refresh interval survive restarts
  (Settings popover).
- **Native & light**: GTK 4 UI; CPU, memory, and disk I/O are read directly
  from `/proc` — only the optional GPU tab needs `rocm-smi`.

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

## Releases

Releases are published automatically from a version tag. To cut a release
(e.g. `1.0.0`):

1. Bump `version` in `Cargo.toml` to a full semver value (`X.Y.Z`) and commit
   (passing the usual `cargo test` / `cargo clippy` / `cargo fmt --check` gate).
2. Tag and push:

   ```bash
   git tag v1.0.0
   git push origin main --tags
   ```
3. The `release` workflow runs on `Debian 13` (x86_64), verifies the tag
   matches the `Cargo.toml` version (and fails if they don't), and publishes
   a GitHub release containing:
   - `simpletaskmgr-<version>-x86_64-linux.tar.gz` (binary, README, LICENSE)
   - `SHA256SUMS`
   - the automatic source tarball/zip for the tag

Every push to `main` and pull request also runs the standard gate
(format, clippy, tests, build) via the `rust` workflow.

## License

MIT OR Apache-2.0
