# GTK Cell-Recycle Refresh Bug — Investigation Log

Status: **NOT RESOLVED.** Two fixes have been attempted, both unit-tested or
reasoned about, but manual testing by the user shows the incoherent cell
values (e.g. "kworker/" shown on firefox's PID) still occur in the running
app:

1. **`set_incremental(false)` on the SortListModel** (commit `f89046d`,
   still in the tree) — a *full* re-sort every refresh. Fixed the blank /
   partially-sorted rows but **not** the text desync.
2. **Discard stale cell bindings on recycle** (commits `81cc6ac` + `753fd84`) —
   the binding-lifecycle fix described below.

The fact that #1 (a full re-sort) does not resolve it is the strongest clue:
the desync survives a full rebind of every row's cells, so the stale
reference is either (a) on a code path the cell factory doesn't own, (b) a
data-level `ProcessRow` reuse across PIDs in `refresh_list`, or (c) a leak
the app never hits the `unbind` path for.

## Symptom

Three interrelated symptoms appear in the process `ColumnView` (sorted by
CPU% descending) during automatic refresh cycles, and resolve on the next
refresh:

1. **Blank rows at the top of the list.** A gap of empty space appears
   between the column header and the first visible data row. The user
   reports "blank lines at the top of the process list" that disappear on
   the next refresh. (Screenshot: `gtk-refresh-bug/Capture d'écran du
   2026-09-06 22-07-32.png` shows the gap above PID 5523.)

2. **Stale sort order.** The row sequence does not match the CPU% column:
   a row showing 61.9% appears *below* one showing 2.7%, and a row showing
   12.6% appears *below* a 0.7% row. The SortListModel's comparator reads
   the *current* `ProcessRow` data, so the visual order must be
   pre-stale (a leftover from an earlier sort pass) rather than a live
   comparator disagreement. (Screenshot: `gtk-refresh-bug/Capture d'écran
   du 2026-09-06 22-08-25.png` shows 2.7% → 61.9% → 0.0% → 12.6% in that
   order; `gtk-refresh-bug/Capture d'écran du 2026-09-06 16-40-09.png`
   shows 12.7% → 0.0% → 5.3% → 2.7% → 1.3% …)

3. **Cell text desync across columns.** A row's PID / User / Name cells
   reflect one process while its CPU% / MEM% cells reflect another — for
   example a PID that belongs to "firefox" displays "kworker/u64:3-btrfs-
   endio-meta" as its Name cell, while the PID column still shows the
   original PID. This is the most severe variant of the desync (screenshot
   `22-08-25` also shows this: row 2 = PID 5523 · name "gnome-shell" ·
   CPU 61.9%, but the row sequence does not match a CPU-descending sort).

All three symptoms are transient: they appear immediately after a refresh
(splice + re-sort) and clear on the next one. The root-cause chain
suspected is: a recycled `GtkListItem`'s cell `Label` retains a live
`glib::Binding` to an old `ProcessRow`, and after the `SortListModel`
re-orders rows the stale binding lets the old row's `notify` overwrite the
cell text — while the `SortListModel`'s comparator (which reads
`ProcessRow` properties directly, not the cell label) sees fresh data and
has already ordered the list by the *old* pre-refresh values.

## Root-cause hypothesis (verified in isolation)

`make_cell_factory` (src/ui.rs) bound each recycled cell `Label` to a
`ProcessRow` with `row.bind_property(name → label, sync_create)` inside the
factory's `connect_bind`, and simply dropped the `glib::Binding` handle.
Probed empirically (standalone crate under /tmp/opencode/probe):

1. **glib does not auto-disconnect** a previous binding when a second
   `sync_create` binding targets the same `(object, property)` — both stay
   active.
2. **`glib::Binding` has no `Drop` implementation** — dropping the Rust
   handle does NOT disconnect the C-side binding.
3. With two bindings live, a `notify` on the stale source can still write
   to the target (last-writer-wins, source of the win not guaranteed).

GTK4's list-item-manager lifecycle for a recycled slot is
`setup → bind(row A) → … → unbind → bind(row B)`. The old code bound in
`bind` but never cleaned up in `unbind`, so the row-A binding remained
live while the label was displaying row B. `refresh_list` keeps refreshing
live `ProcessRow` objects in place (`set_item` → `notify`), so a
still-alive stale row could push its old text into the recycled label: the
"kworker/ on firefox's pid" symptom.

## Prior attempt — f89046d (also did not resolve the issue)

Before the binding-lifecycle fix, a different theory was pursued. The
hypothesis was that GTK 4.18's `GtkSortListModel` is *incremental* by
default, so on refresh it re-emits store changes to the `ColumnView` as
minimal remove/insert deltas. On a delta that lands *after* a `ProcessRow`
was re-texted in place, the reused `GtkListItem` was reportedly left
unbound (blank rows at the top) and the sort order only partially honored —
`show all` triggered it because the many short-lived / reordering kernels
fire that delta path every refresh.

The change (commit f89046d055b9ee99c26e0388feb25165e3eb6683, still present at
`src/ui.rs:579`) was a single line in `build_process_list`:

```rust
sort_model.set_incremental(false);
```

so every refresh is a full, deterministic re-sort over the just-updated
values. That reportedly fixed the *blank rows / partial sort*, but did **not**
fix the out-of-sync cell text (kworker name on firefox's PID). This is the
key data point: a full re-sort still desyncs cells, which points away from
"sort deltas reorder things" and toward a per-cell binding/reference staleness
that a full re-sort happens to still leave in place — consistent with the
binding-lifecycle theory above, and with the not-yet-found *other* sources of
the desync (see "Not enough" below and next steps).

## Fix implemented (commits 81cc6ac + 753fd84)

In `src/ui.rs`:

- `CELL_BINDING_KEY` qdata slot parks the live `glib::Binding` on the
  `Label`.
- `bind_cell_binding()` — steals and `unbind()`s any prior binding parked
  on the label before creating a new one (defensive guard + `log::warn!`
  diagnostic if one is ever found where it shouldn't be).
- `make_cell_factory` now connects `unbind` to steal and `unbind()` the
  parking binding, guaranteeing the label has exactly one live binding at
  any time.
- Three regression tests in `src/ui.rs::tests` proving, using a minimal
  glib `CellValue` stand-in for the label:
  - two live bindings race and a stale row's write wins (symptom),
  - `unbind()` of the old binding stops the stale write reaching the cell,
  - direct `unbind()` severs `notify` from the target.

All 320 lib tests + integration tests pass, clippy and fmt clean.

## Why it is (per user testing) NOT enough

The unit tests prove only the raw glib binding semantics — they do not
exercise GTK's real `GtkList` item-factory signal ordering, only
`set_item`→`notify` as the stale trigger, and only `ProcessRow` objects
kept alive by the test scope.

Possible remaining holes (not yet confirmed):

1. **Other stale paths we haven't disconnected.** The PID column displays
   `ProcessRow::property("pid")`; if any other widget path (e.g. the
   selected-process detail panel, tooltip, row activation, or another
   factory) holds its own leaked binding or a stale `Row` reference, the
   same symptom appears — the "kworker name on another row's PID" combo
   suggests the *name cell* and *pid cell* got different rows, i.e. a
   per-cell desync — exactly the mechanism fixed here, but we have not
   confirmed the app-level path.
2. **`refresh_list` splicing order vs binding updates.** `refresh_list`
   splices rows in by PID. If a `ProcessRow` gets reused across pids
   (row object kept, `set_item` with a *different* pid), the *name* cell
   could legitimately lag the *pid* cell by one refresh cycle. That is a
   data-level (not binding-level) desync our fix would never touch.
3. **`bind` → `bind` without a real `unbind`** — GTK4 can rebind a slot if
   the list *model* swaps an item in place. Our handler only fires on
   `connect_unbind`; if GTK4 fires `setup`/`bind` twice on a row widget
   without `unbind` for a new row, our stale-binding *guard* in
   `bind_cell_binding` should catch it (`log::warn!` fires) — but we have
   not yet captured logs from a real repro.

## Next steps / instrumentation to pin down the real path

1. Add `log::info!` in `bind_cell_binding` logging `(pid of row, prop,
   cell label ptr)` every time it's called; run the app with `-v` and
   trigger the repro by sorting on CPU until the desync appears. Correlate
   the log lines with the wrong row's PID.
2. Add a debug-only `assert` in `set_item` that checks the label text
   currently displayed matches that row's `name`/`pid` for the row's own
   cell, and `log::warn!` + a panic in debug builds if they disagree.
3. Grep `src/` for **all** `bind_property` call sites (not just the
   factory) — the detail panel, the CPU/MEM graph pane tooltips, the
   context-menu label — any of them could still be creating the same
   leaked binding. Disconnect/refresh each on the same lifecycle signals.
4. Confirm whether `refresh_list` can *rescue* a `ProcessRow` across a
   PID change (search `refresh_list.rs` for `find` / `set_item` call
   sites). If it does, that's a data-level desync and the real fix is to
   replace the row object, not mutate it.
5. If the above all come back clean, consider a stronger guard at
   `set_item`: if the row's current `notify("name")` handler does not
   correspond to the row's current cell (tracked via the `CELL_BINDING_KEY`
   qdata slot), treat the write as a stale write and skip it.

## Files involved

- `src/ui.rs` — cell factory fix + the 3 regression tests (all `#[cfg(test)]`).
- `src/refresh_list.rs` — splices rows into the `ListStore` by PID;
  the data-level side of the symptom is in this file, not `ui.rs`.
- `src/process_row.rs` — `ProcessRow` `glib::Object` subclass holding one
  `ProcessItem`; `set_item` diffs and `notify`s changed properties.
- `tests/integration_tests.rs` — no-GTK integration tests (not involved).
- `/tmp/opencode/probe/` — standalone minimal crate that established the
  glib binding semantics (no drop-disconnect, two-live-bindings race).

## Reproduction (user-reported)

1. Run the app; leave a kworker-ish PID running (kernel threads are
   common on any Linux box).
2. Sort the process list by CPU (or MEM) and trigger multiple refresh
   cycles (the app auto-refreshes).
3. Observe any of:
   - a **blank gap** at the top of the list (empty space above the first
     row) that clears on the next refresh,
   - a **stale sort order** where the CPU% values are not strictly
     decreasing (e.g. a 61.9% row below a 2.7% row),
   - a **cell desync** where a PID that belongs to one process displays
     the name of another (e.g. "kworker/" on firefox's PID).

Screenshots captured at `gtk-refresh-bug/`:

- `Capture d'écran du 2026-09-06 16-40-09.png` — stale sort order (12.7%
  → 0.0% → 5.3% → 2.7% → 1.3%).
- `Capture d'écran du 2026-09-06 22-07-32.png` — **blank rows at the top**
  (gap above PID 5523) *and* the otherwise well-ordered list immediately
  below (a "recovery" refresh that has re-bound the recycled widget).
- `Capture d'écran du 2026-09-06 22-08-25.png` — **cell desync**: PID 5523
  (gnome-shell) shows 61.9% CPU, but row order is 2.7% → 61.9% → 0.0% →
  12.6% → … which is not CPU-descending.
- `Capture d'écran du 2026-09-06 22-08-52.png` — a subsequent refresh
  re-sorting the same data (13.4% → 9.3% → 6.7% → 1.3% → 0.7% → 0.7%),
  confirming the desync is transient.

## Notes

- `cargo test` runs headless on Linux under Xvfb/offscreen; the GTK4
  widget tests in `src/ui.rs` use a minimal glib stand-in because the
  full `GtkList` factory lifecycle (with `connect_setup`/`bind`/`unbind`
  firing in the order the `GtkList` widget emits) requires the real
  widget tree and is not exercised by the tests that pass today.
- The `log::warn!` diagnostic in `bind_cell_binding` is the first thing
  to look for in a repro — if it fires, the stale-binding guard caught a
  leak (and should have retired it); if it *doesn't* fire, the stale
  write is coming from a different code path entirely (likely the
  detail panel, another widget, or a data-level `set_item` desync).
