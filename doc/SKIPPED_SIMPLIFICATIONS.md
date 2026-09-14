# Skipped simplifications

Tier 1 over-engineering (low risk, clear win) was consolidated into three
commits:

- **Consolidate uptime reads** — `cpu_tracker` now calls
  `disk_status::read_uptime_secs()` instead of running a second
  `procfs::prelude::Current::uptime()` path.
- **usage_graph dead code** — removed `frac_of` + `sample_y_frac` (strict
  subsets of `frac_range(v, 0.0, max)`), the always-0/0.5/1.0
  `axis_range_ticks` `frac_range` boilerplate, the redundant
  `ghz.fract()==0.0` guard, the unreachable disk arm of
  `legend_items_for_pane`, and its `samples` parameter (no longer used by
  any branch).
- **Extract `is_temp_n` helper** — `hottest_temp_c` and
  `sensor_label_is_cpu` share one `temp<N>` pattern check.

The items below were considered and **not** applied. Each is real over-engineering
or duplication, but each also carries a real risk/reward trade the user should
revisit deliberately rather than as a drive-by edit.

## `gpu_status.rs` forward-compat number branch

`read_number` accepts a `serde_json::Value` that is either a string
(`"97"`) or a JSON number (`97`), and `test_parse_gpu_json_accepts_json_numbers`
pins that behaviour. The `else { v.as_f64() }` branch is defensive coding
for a `rocm-smi` format change that never actually happens — every real
fixture in this file uses strings, and the doc comment on the function
("All numbers are expected from `rocm-smi` as strings") is the only
evidence the other form is even plausible.

To collapse we would delete the number branch, its doc wording, and the
test. Reward: one fewer `if let` in a hot path that runs once per sample
tick. Risk: if `rocm-smi` ever does start emitting numbers, every reading
silently degrades to `None` (and `gpu_available()` flips to `false`,
hiding the tab). That is a *worse* failure than the current state.

Revisit condition: when `rocm-smi` is observed to emit JSON numbers
(verified by inspecting one real `--json` output on a host that emits
them), or when the GPU tab is dropped as a feature.

## `cpu_status.rs` label-based CPU-sensor fallback

`pick_cpu_hwmon` (driver `name` match, the primary path) and
`pick_cpu_hwmon_by_label` (scan `tempN_label` files) both run in
`read_cpu_temp_c`. The label path is the "when the driver's `name` isn't
in the list" fallback, with its own test
(`test_pick_cpu_hwmon_by_label_fallback`), its own non-CPU-sensor skip
(`pick_cpu_hwmon_by_label_skips_non_cpu`), and a public `is_cpu_label`
helper.

It *is* duplicated logic (the `tempN` scan, the "is this the CPU?"
question) — but it also covers real hardware: a `coretemp` build that
names its hwmon differently, a `k10temp` on an AMD chip, etc. Removing
it makes `read_cpu_temp_c` return `None` on any of those hosts, which
is a visible behaviour change (the GPU/Temp tab would just stop
reporting temperature).

The duplication is already partly de-duped: this commit extracted
`is_temp_n` so the *pattern* is single-sourced. The *decision logic*
(`which sensor is the CPU?`) stays separate because the two paths
answer different questions (`name == known_cpu_driver` vs.
`label == "CPU"`).

Revisit condition: when a second codebase that we maintain has a
cleaner single-source CPU-sensor discovery, or when the GPU/Temp tab is
dropped.

## `refresh_list.rs` — not "double sorting"

An earlier plan called this out as a "double sort." Reading the module
doc and the diff-based `refresh` implementation, it is actually a
deliberate anti-flicker contract: instead of tearing down and
rebuilding the `ListStore` (which causes the blank flash +
`GtkListItemManager` widget churn that was the target of an extended
investigation — the GTK-refresh bug), `refresh` diffs the target pid
sequence and:

- splices out the removed pids in one `items-changed` signal each,
- splices moved rows back-to-front with the widget-keep
  `store.splice(pos, len, additions)` trick,
- re-texts rows in place via `ProcessRow::set_item` (which emits
  property change, no store signal at all).

That is the "spice." Cutting it would regress to a rebuild, which is a
visible UI regression (list flash + scroll jump), and the module
documented the *why* — including the GTK 4.18
`GtkListItemManager.deleted_items` lifecycle detail at the top.

Do not treat "the code is long" as a simplification target here.
Revisit condition: only if a future GTK major version makes the
widget-reuse problem go away.

## `ui.rs` selection/scroll restoration

Same category as above: this code exists specifically so the
`ColumnView` does **not** lose its selection across a refresh. If the
user has a row selected, refreshes, and the row stays in the list, the
selection should stay. If we removed this code and GTK's default
autoselect fired, the UI would jump to row 0 on every refresh — that
is exactly the kind of regression the cell-binding and refresh-flicker
investigations (see `doc/REORDER_FLICKER.md`) were written around.

Revisit condition: a GTK release that makes `SingleSelection` not
autoselect on `items-changed`, or a user-visible bug that this code
itself is causing.

## Numeric-column descending sort (`ui.rs`)

`make_cell_sorter` reads the *primary* column's descending order so
`compare_values` can pin missing values to the bottom in either
direction. This is not duplication, it is a fix for the
"missing rows sort to the top" regression that happens when a column
is descending. Deleting it makes a user-visible regression.

Do not touch without a replacement strategy for
"missing values always at the bottom" that has been validated against
`ProcessList::compare_values` and its descending cases.

## `evict_dead_processes` (`cpu_tracker.rs` + `io_tracker.rs`)

Both files have a `pub fn evict_dead_processes(&mut self,
live_pids: impl Fn(i32) -> bool)` that does a `.retain()` over their
internal `HashMap<i32, _>`. They look duplicated, but:

- They operate on **different fields** (`self.process_usage` vs
  `self.baselines`) in **different structs** (`CpuTracker` vs
  `IoTracker`); a shared free function would need the map passed in and
  returned, or the two structs would need a common trait — neither of
  which is smaller than the current 3-line method.
- Keeping each method on its owner's type is the Rust-idiomatic shape.

Leaving both is the right call. No action.

## `metrics.rs` — lifetime vs. interval rates

`lifetime_cpu_percent` and `lifetime_disk_rates` are "real first-frame"
values computed from `/proc/uptime` and the counter deltas. They look
duplicated with the per-interval rates in `disk_status`/`cpu_tracker`,
but they are **different quantities**: interval rates are
`delta(counter) / delta(time)` between two samples, while lifetime rates
are `counter / uptime_secs` for a single sample. Folding one into the
other changes the first-frame value the user sees when the app opens.

User decision (this session): **keep the real first-frame value**, so the
lifetime-rate path stays.

## `disk_status` short-circuit (disk panes are "dead" through the legend path)

The commit that removed
`ChartPane::DiskThroughput | ChartPane::DiskUtil` from
`legend_items_for_pane` is correct *because* `paint_usage_chart` routes
disk panes to `paint_disk_pane` and returns before the dual-axis code
that would call `legend_items_for_pane`. That is the current behaviour;
do not "restore" the disk arm as a convenience — it would be dead code
in production and only exercised by tests.

If a future UI redesign wants to re-run the disk panes through the
shared dual-axis path, that redesign would also re-introduce the disk
arm. The commit message on
`57336d0` ("Remove dead usage_graph helpers and redundant math")
documents that invariant.
