# Code review — correctness & performance findings (v1.0.0-beta.3)

Full manual review of all 19 source files (~10.2k LoC) plus the two GTK
integration suites, with focus on crashes and incorrect behavior. Every
finding below was verified against the source at the time of writing; false
positives investigated and ruled out are listed in [Verified non-issues].

Severity scale: **A** = demonstrable incorrect behavior, **B** = performance /
robustness risk, **C** = minor.

## Confirmed correctness bugs

### A1 — A second launch opens a second window with a second state (`src/main.rs`, `src/ui.rs`)
`SimpleTaskMgr` is created with the default `GApplication` flags
(single-instance). A second invocation from the terminal is therefore
*forwarded to the running process*, which fires `connect_activate` again.
That handler unconditionally calls `State::new()` + `build_window`, so the
user gets:

- a second, independent refresh timer (two `/proc` walks + two `rocm-smi`
  spawns per tick),
- two windows whose settings popovers write the **same**
  `~/.config/simpletaskmgr/settings.toml`, desynced from each other.

GTK itself does not crash on this (the second `ApplicationWindow` is a valid
child of the same app), but the behavior is plainly wrong.

**Fix:** the `activate` handler presents the existing window when the
application already owns one (ui.rs gains a `pub fn activate(&gtk4::Application) -> bool`),
otherwise creates the state + window as before. `main.rs` wires that function
directly. Regression test: a second `activate` on an app that already has a
window must return `false` and leave the same window in place.

### A2 — `Reset to defaults` with "show all" on: `toggled` re-enters the state, panics, or rebuilds twice (`src/ui.rs`)
The reset handler called `sync_settings_widgets` (which was holding the shared
`state.borrow()`) before its own final `rebuild()`. Flipping the check with
`set_active` fires `toggled` *synchronously*, and the handler takes
`state.borrow_mut()`. Two failures depending on the situation:

- **panic/abort** — `RefCell` double-borrow inside a GTK signal
  (`borrow_mut` while sync's immutable borrow is live): unwinding through
  the GTK C boundary aborts the process. This is the crash path of "Reset to
  defaults" with "Show all processes" on.
- **stale second rebuild** — if the flip's handler runs at all, rebuilds the
  view; the reset handler then rebuilt again against a list that the first
  pass had already refilled (a full `view.update()` over the whole system
  list, built and thrown away).

**Fix:** `sync_settings_widgets` drops the state borrow before flipping,
reports when its flip already rebuilt, and the reset handler skips its
trailing rebuild in that case — exactly one rebuild per user action, no
re-entrant borrow. Regression test: reset with "show all" on applies the
change and costs exactly one refresh.

### A3 — A transient `/proc` read error blanks the entire process list (`src/process_list.rs`)
`update_process_list` clears `self.processes` on `Err` of
`refresh_process_list` and the list stays empty until the *next* successful
tick — a one-frame failure therefore shows an empty list, a single-row
detail pane and a bogus all-zero summary header (the row is rebuilt from
nothing even after the next tick re-adds rows, for at least one screen
paint). `refresh_process_list` itself is already resilient: per-row
failures (uid/stat/statm/io) are logged and the row is skipped; only the
`/proc` walk itself hard-fails. But a hard failure of the whole walk is
exactly what a blip (e.g. container `/proc` mount hiccup, OOM pressure)
could produce, and "blank list" is strictly worse than "last known list".

**Fix:** on `Err`, log and keep the previous snapshot; the list self-heals
on the next successful refresh. The success path is unchanged, including
`evict_dead_processes` (which still runs only after a successful refresh).

## Performance / robustness findings

### B1 — `ProcessView::update` appends new rows one commit at a time (`src/process_view.rs`)
Each new row is a separate `ListStore::append` — one `items-changed` commit
each — and the membership filter is a `Vec::contains` per candidate
(O(n²) over new rows after the common `Show all processes` toggle, when
every row is "new"). Both costs hit the GTK main thread every 1.5 s.

**Fix:** collect new `ViewRow`s and insert them with one
`ListStore::splice(n, 0, rows)` commit; membership test switches to a
`HashSet`. Behavior (content, ordering, selection) unchanged — covered by
the existing update tests plus a new `items-changed` commit-count assertion.

### B2 — The sort comparator deep-clones both rows on every comparison (`src/process_view.rs`)
`SortListModel::changed(Different)` makes GTK re-sort by invoking the
comparator, which does `ra.item()` + `rb.item()` — a full `ProcessItem`
clone (name, cmdline `Option<String>`, …) **per comparison**. With the
show-all list (thousands of rows) and long cmdlines this is the single
largest periodic allocation in the app.

**Fix:** `ViewRow` gains a borrowed accessor `item_ref()` and the comparator
compares through `compare_values(&a, &b, …)` without cloning. No behavior
change; existing ordering/`S5`/tie-break tests cover it.

### B4 — `rocm-smi` is spawned and `wait`ed on the GTK main thread with no timeout (`src/gpu_status.rs`)
`read_gpu_card` runs `Command::new("rocm-smi").output()` synchronously in
the sample worker on the GTK main loop thread. A wedged `rocm-smi` (ROCm
driver faults exist; it can block in the kernel) freezes the whole UI.

**Fix:** a generic `run_with_timeout(cmd, timeout)` helper: spawn with
piped stdout drained by a reader thread, poll `try_wait` until a 2 s
deadline, then `kill` the child and give up (`kill_on_drop`, no blocking
`wait`, so even an unkillable D-state child cannot hang the loop).
`read_gpu_card` reports `None` on timeout (GPU columns blank for that
sample — same as a read failure today). Tests: success, non-zero exit, and
a real 10 s `sleep` killed well before its runtime.

### C1 — Kill can hit a *reused* PID (TOCTOU) (`src/ui.rs`, `src/signal.rs`)
`State::kill` signals whatever process currently owns `selected_pid`, not
the one the user inspected: if that process exits and the kernel reuses the
PID before the button press, the *new* process receives the signal
(typically killed). The selected row is still on screen, so nothing in the
UI hints at this.

**Fix:** each row records the raw `stat.starttime` ticks (`TaskMgrProcess`
gains `start_time_ticks`); at send time `State::kill` re-reads
`/proc/<pid>/stat` and refuses (new `KillStatus::Failed` with a clear
"PID was reused" cause) when the live start-time differs from the row's.
A `None` row (pid already gone) still proceeds and surfaces the usual
ESRCH path. Tests spawn a `sleep` child for both the accept and the refuse
cases — the test process is never signalled.

### C2 — `CpuTracker` / `IoTracker` disagree on a non-elapsed sample (`src/cpu_tracker.rs`, `src/io_tracker.rs`)
`calculate_cpu_sample` returns `Some((delta, 0.0))` when
`time_elapsed <= 0.0`; `calculate_rates` returns `None`. Both branches are
effectively unreachable today (monotonic `Instant` clock; the only real
trigger was A2's double refresh, now removed) and the CPU branch's zero is
an intentional part of the "first-sight zero group" design
(`doc/CPU_FIRST_SIGHT_BLANK_LINES.md`), whereas `cpu_percent` is a
non-`Option` `f64` while the IO speeds are `Option`. Changing one to match
the other would be a user-visible UI/API change disproportionate to the
benefit. **Decision: keep both behaviors; document the divergence in
`cpu_tracker.rs`** so it is not mistaken for a bug.

### C5 — Redundant second `unbind()` in the cell factory teardown (`src/process_view.rs`)
`cell_label::take_and_unbind` already unbinds the `glib::Binding`; the
factory's `connect_unbind` then calls `b.unbind()` again on the same binding
(second call on an already-unbound `Binding` is a no-op, so harmless — but
misleading). **Fix: drop the redundant call.**

## Verified non-issues

Investigated during the review and confirmed correct:

- `sample_x` divide-by-zero: guarded by `fill.max(2)` (`src/usage_graph.rs:161`).
- `set_selected(u32::MAX)` on `SingleSelection`: GTK semantics — selects
  nothing (equivalent to `GTK_NO_SELECTION`); tests rely on it.
- PID-reuse underflow in both trackers: `checked_*` arithmetic returns
  `None` → baseline reset, no panic.
- `parse_stat_start_time`: `checked_sub` + non-negative age guard.
- Cell binding lifecycle (`cell_label.rs`): park/take/retire per recycle is
  sound; one bounded orphan at program teardown is documented.
- `refresh_process_list`: per-row failures are handled individually; only
  the walk itself fails (see A3 for the consequence, now fixed).
- `evict_dead_processes` runs only after a successful refresh.
- `column_sorter` uses the `cpu_ticks` quantized key + pid tie-break per
  `doc/SORT_KEY_DESIGN.md`, stable across refreshes (S5).
- `State` refresh reentrancy: `restart_timer` replaces the pending source
  each time; cadence pinned by `test_refresh_cadence_one_per_tick`.
- Settings save/load round-trips and the `~/.config` fallback path are
  covered by `settings.rs` tests.
- `Signal` → `kill(2)` mapping verified against libc numbers in
  `signal.rs` tests.

## Fix plan (priority order)

A1 → A2 → B4 → A3 → B1 → B2 → C1 → C2+C5. Each lands as its own commit with
a regression test, then `cargo test && clippy --all-targets &&
cargo fmt --check` (per `AGENTS.md`).
