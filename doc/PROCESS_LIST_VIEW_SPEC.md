# Process List View — Requirements & Design (clean-room)

This spec drives the clean-room re-implementation of the process list view.
The new module `src/process_view.rs` is written from the requirements below,
not from the existing code. The legacy view parts it replaces
(`process_row.rs`, `refresh_list.rs`, and the list/detail/signal-button
wiring inside `ui.rs`) are deleted once the new module is wired in.

The data layer (`process.rs`, `process_list.rs`, `cpu_tracker.rs`,
`io_tracker.rs`, `settings.rs`) is **unchanged** — the view renders the
`ProcessItem` snapshots it is handed.

## Essentials (the brief)

1. A list of processes with **name, CPU usage, memory usage, and disk I/O**
   (plus PID for identification).
2. **Sortable per column** — click a column header.
3. The list **refreshes every 1.5 s** (app default; user-configurable).
4. **Clicking a process shows a detail view** for that process.

## View requirements

### V — Display

| # | Requirement |
|---|-------------|
| V1 | One row per process. Columns left→right: **PID, Name, CPU %, MEM %, Disk R, Disk W**. |
| V2 | Cell text: PID in decimal; Name as the row's display name; CPU % and MEM % as `x.x%`; Disk R / Disk W as human-readable rates (`999 B/s`, `12.3 KiB/s`, …, up to `TiB/s`). |
| V3 | A value that is unknown (MEM % or disk rate unreadable / unmeasured) renders as an **empty cell** — never a partial or placeholder text. |
| V4 | The list scrolls vertically and grows to fill the window. |

### S — Sorting

| # | Requirement |
|---|-------------|
| S1 | Clicking a column header sorts the whole list by that column. |
| S2 | First click sorts **ascending**; further clicks toggle that column's direction. GTK's native column-view header shows the active column + arrow. |
| S3 | Unknown values sort to the **bottom** of the list in both directions. |
| S4 | Ties are broken by **PID**, so the visible order is deterministic and stable across refreshes. |
| S5 | The comparator is a **total order** (consistent, antisymmetric). GTK's sort panics on an inconsistent comparator — this is a hard requirement, not a preference. |
| S6 | CPU % is ordered by the measured **delta ticks** (not the rounded percent): rows that display the same value keep a deterministic order and CPU-usage ties differ only by PID. |
| S7 | Default order at startup: **CPU % descending** (heaviest first, `top`-style) — matching the old view and user request, so the list is useful without any interaction. |
| S8 | A header click **scrolls the list back to the top**, so the new first row (e.g. the highest-MEM process after clicking MEM%) is visible immediately. Refresh ticks never do this — a tick's invalidation is not a sort change, so the user's scroll position survives refreshes (R3). The scroll is **deferred to a `glib::idle`**: GTK settles the click through the sorter's `primary-sort-column`/`order` notifies first and commits the re-sort afterward; ListBase keeps its anchor item on screen by *identity*, so a scroll executed before the commit would pin the *old* first row and the viewport would land wherever that row moved to (the reported bug). An idle runs after the commit, when row 0 is already the new top row. |

### R — Refresh

| # | Requirement |
|---|-------------|
| R1 | Each refresh tick reflects the new snapshot: new processes appear, vanished ones disappear, values update. |
| R2 | **No blank flash**: the row for a given PID persists across refreshes — value-only updates must not recreate rows or their text; a cell never shows another row's text (binding hygiene, B1). |
| R3 | Scroll position is preserved across a refresh (within the new bounds). |
| R4 | The selection follows the **process** (by PID), not a position: re-sorted, replaced, or reordered refreshes keep the user's process selected; a vanished process clears the selection and hides the detail pane. |
| R5 | Refresh works for any snapshot size, including the empty snapshot, and is idempotent on identical data (a second refresh of the same data is a no-op). |
| R6 | **One refresh per tick**: each timer firing produces exactly one `State::refresh` (one data read) and one view update. A second refresh per tick shrinks the CPU-delta window to the first refresh's own duration, inflating every reported %CPU (the cadence test in `tests/process_view_gtk.rs` pins this down). |

### D — Selection & detail pane

| # | Requirement |
|---|-------------|
| D1 | Single selection; nothing is selected at startup. |
| D2 | Selecting a row shows a detail pane with **Name, Command line, State, Threads, Nice, Memory, Started, Uptime** for that process — the fields *not* already visible in the row (PID, CPU %, MEM %, Disk read, Disk write stay in the list so the pane stays compact) — formatted as in a row cell (V2) with unknown values shown as `—`. The command line is selectable (copy/paste). The start time and uptime are derived from `stat.starttime` (ticks since boot) plus the host's `/proc/uptime` and ticks/sec; the absolute Memory is the RSS in a human unit. Compactness constraint: at the window's default size the pane (rows + signal buttons + status line) must fit without inner scrolling. |
| D3 | Clearing the selection (or the process vanishing) hides the pane and the list takes the full width. |
| D4 | The selected PID is queryable (`selected_pid()`) and selection changes are observable via a callback — so the app can attach actions (e.g. sending a signal, deferred) without owning the detail pane. |
| D5 | **Signal buttons**: the detail pane offers `Terminate` (SIGTERM) and `Kill` (SIGKILL) for the selected process. The view sends no signal itself — it forwards the requested signal to the app (one callback), which performs `D4`'s send via `State::kill` and reports the outcome in the pane's status line. Originally deferred (SIGHUP/SIGKILL were out of scope); implemented for 1.0.0. |

### B — Robustness

| # | Requirement |
|---|-------------|
| B1 | **Cell-slot binding hygiene**: GTK recycles row widgets. A data binding created for a recycled cell slot must be discarded the moment the slot is recycled again, so a stale binding can never point at another process's data. (Leaking the bindings instead is what this project already suffered.) |
| B2 | **Re-entrancy**: a refresh may fire the selection callback into the app (GTK changes the selection position synchronously during a store commit). No app borrow, no `&State` RefCell, may be held across the GTK call; the callback must be safe to run while a refresh is in flight. |
| B3 | **Degradation**: unreadable fields degrade to an empty cell (V3), never to a failed refresh or panic. |
| B4 | All diagnostics via the `log` crate (`error!`/`warn!`/`debug!`), never `println!`/`dbg!`. |

## T — Testability requirements

| # | Requirement |
|---|-------------|
| T1 | Cell-text formatting and comparator semantics are unit-testable with fixture data — no display, no `/proc`. |
| T2 | Store-level refresh semantics are unit-testable headless (glib objects only): survivor **object identity** across refreshes, value-only refresh emits **no `items-changed`** on the store, and the end state is correct across **100 random add/remove/update sequences**. |
| T3 | The refresh mechanism pins the GTK behaviors it relies on (verified on GTK 4.18, see *Mechanism evidence*): a sorter kick whose re-sort leaves the order unchanged emits **no** `items-changed`; a membership change auto-resorts and commits at most once — so a kick is only redundant, never harmful. |
| T4 | Selection semantics (follow-by-PID, fall-off-when-gone, callback re-entrancy during a refresh) are unit-testable with a headless `gtk4::init()`, exercising the same notify path a real click uses. |

## Module API

```rust
pub struct ProcessView /* owns the model chain + detail pane; no per-row Rust state */;

impl ProcessView {
    /// Builds the list + detail subtree. Call once; embed with `widget()`.
    pub fn new() -> Self;

    /// The subtree to embed in the window (list + detail pane).
    pub fn widget(&self) -> &gtk4::Box;

    /// One refresh tick: render `procs` (the current snapshot).
    /// Takes a reference — no snapshot cloning at the boundary.
    pub fn update(&self, procs: &[ProcessItem]);

    /// PID of the currently selected process, if any.
    pub fn selected_pid(&self) -> Option<i32>;

    /// Fires with the newly selected PID (`None` = deselected). May be called
    /// from inside `update()` (selection position change) — B2.
    pub fn connect_selection_changed<F: Fn(Option<i32>) + 'static>(&self, f: F);

    /// Initial paint only: start showing the top of the list.
    pub fn scroll_to_top(&self);
}
```

The row is a glib `ViewRow` object (six string properties, one per column, +
raw value getters for the comparator) — the only per-row Rust state. All
bookkeeping (sort state, selection, recycling, cell parking) is owned by GTK
objects or the `cell_label` registry.

## Algorithm — `update(procs)`

1. **Update survivors in place**: for every snapshot process whose PID is
   already in the store, replace the row's data and notify its six
   properties. (No store signal — the store's order and composition are
   untouched, the row object is the same one bound into the cells.)
2. **Membership**: remove rows whose PIDs vanished (single splice over the
   contiguous block); append new rows in snapshot order. GTK auto-resorts on
   these commits.
3. **Converge**: one sorter kick. If the order did not change, GTK emits
   nothing (T3 evidence); otherwise it commits the reorder exactly once.
4. **Re-pin selection by PID** (R4): resolve the PID captured at the start of
   the refresh to its new position; move the selection there (or clear it if
   the PID is gone).
5. **Restore scroll** on idle (R3), with the same tolerance guard as before.

Order matters: value updates before membership ops means the auto-resort at
step 2 already sees fresh values; step 3 then never reorders, it only
converges.

## Mechanism

- **Model chain**: `ListStore<ViewRow> → SortListModel(ColumnViewSorter) →
  SingleSelection → ColumnView`. Sorting, selection, and row-widget recycling
  all come from GTK — no hand-rolled list state.
- **Comparator** (single source of truth, per column):
  `key(column) cmp, then PID` for Pid/Name/Cpu; `rank-Optional(key) cmp, then PID`
  for Mem/DiskRead/DiskWrite with unknown pinned to the bottom (direction-aware:
  the column only sorts descending when it is the *active primary* column —
  same rule as before, keeps the "bottom in both directions" promise honest).
  `f64` compared with `total_cmp` (S5: NaN total order).
- **Cell factory**: one `Label` per column, bound to the row's string
  property; unbind retires the binding through the existing `cell_label`
  parking registry (B1).
- **Selection**: `SingleSelection::set_autoselect(false)`; a `notify::selected`
  handler updates the detail pane and emits the callback (D2/D4).

## Mechanism evidence (measured, GTK 4.18 — the runtime this builds against)

A probe against `SortListModel` + `CustomSorter` (3 rows) showed:

| Action | `items-changed` |
|---|---|
| `sorter.changed(Different)` with unchanged values | **0 events** — GTK commits nothing when the order is unchanged |
| `sorter.changed(Different)` after a value reorder | 1 event, `pos=0, removed=3, added=3`, correct new order |
| append to the base store (auto-resort), then kick | the append auto-resorts; the kick is a redundant no-op |

Consequences: the legacy order-diff gate and splice engine are unnecessary
for correctness *and* not needed to avoid visual churn — the no-op kick
already costs nothing observable. The simple algorithm above is the whole
refresh engine.

## Out of scope

- Reading `/proc`, CPU/IO tracking, the `ProcessItem`/`TaskMgrProcess` types.
- The settings popover and usage graph panels (app-level, kept as-is).
- The `--no-resort-kick` diagnostic (only meaningful with the legacy engine).
- The old "CPU % descending at start" default (S7).

## Integration & migration

1. **Spec** (this file).
2. **New module** `src/process_view.rs` (row, comparator, cell factory,
   `ProcessView`, `update`, tests) — standalone, old code untouched.
3. **Wire-in**: `ui.rs` builds the window from `ProcessView` (timer →
   `update`, settings → `refresh`+`update`, selection callback →
   `state.selected_pid`); remove the legacy list/detail/button code, the
   `--no-resort-kick` flag, and now-dead helpers.
4. **Cleanup**: delete `process_row.rs`, `refresh_list.rs`,
   `process_list::compare_values` (the comparator moves to the view),
   `SortColumn` from `lib.rs`; point `cell_label` tests at a neutral
   property object.
5. **README** update + final pass.
