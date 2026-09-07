# GTK Cell-Recycle Refresh Bug — Investigation Log

Status: **NOT RESOLVED — investigation paused at the Phase 1 instrumented
baseline.** See the "Current state (2026-09-07)" section at the bottom for the
exact branch / commit / evidence / what's ruled out / what's next. Everything
above is the earlier record and is retained for continuity.

---

# GTK Cell-Recycle Refresh Bug — Investigation Log (earlier record)

**Not resolved.** There are **two** distinct, differently-persistent
defects, and neither is resolved:

- **Blank rows at the top** — *cosmetic & transient*. Takes *several*
  refreshes to clear and is **reliably cleared by scrolling down and back
  up**. Points at a render / row-manager staleness, not a binding leak.
- **Wrong process name on a row (cell desync)** — *persistent data
  corruption*. **Never fixes itself** for a broken row; later refreshes do
  not repopulate it. Points at a live binding holding the wrong `ProcessRow`
  source, or a `ProcessRow` object reused for a different PID.

Two fixes have been attempted, both unit-tested or reasoned about, but manual
testing by the user shows the desync still occurs in the running app:

1. **`set_incremental(false)` on the SortListModel** (commit `f89046d`,
   still in the tree) — a *full* re-sort every refresh. Did **not** resolve
   the desync (and did not reliably fix the blank rows either).
2. **Discard stale cell bindings on recycle** (commits `81cc6ac` + `753fd84`) —
   the binding-lifecycle fix described below. Did **not** resolve the
   persistent desync.

The contrast in persistence (scroll-clears vs. never-clears) is the strongest
clue: it is almost certainly *two* bugs, not one. The transient one is likely
a `GtkListItemManager` render/realise defect in GTK 4.18's recycling; the
persistent one is a live, wrong-source `glib::Binding` (or a `refresh_list`
data-level `ProcessRow`/PID mismatch). Neither of our two fixes addresses the
transient render defect, which is why the blank rows remain.

## Symptoms

Two distinct, *differently-persistent* symptoms appear in the process
`ColumnView` (sorted by CPU% descending) during automatic refresh cycles.
They should be treated as possibly-different bugs: one is purely cosmetic
and self-heals, the other corrupts data and does **not** self-heal.

### 1. Blank rows at the top of the list — transient, cosmetic

A gap of empty space appears between the column header and the first
visible data row; many rows can be affected at once. It is a layout/render
anomaly, not a data one — the rows underneath are correct.

Properties (per user):
- **Not cleared reliably by the next refresh.** It may take *several*
  refreshes to clear, and sometimes does not clear on refresh alone.
- **Scrolling down and back up clears it immediately.** This is the
  reliable workaround — scrolling off and back into the top forces a
  re-layout / re-realise of the recycled row widgets.
- Suggests the recycled `GtkListItem` widgets are allocated but not
  populated / not measured (or the row manager's `deleted_items` cache has
  gone stale), a render-level rather than binding-level defect.

Screenshots:
- `gtk-refresh-bug/Capture d'écran du 2026-09-06 22-20-23.png` — **many**
  blank lines above PID 12984 (RDD Process).
- `gtk-refresh-bug/Capture d'écran du 2026-09-06 22-07-32.png` — smaller
  gap above PID 5523.

### 2. Cell text desync (wrong process name) — **persistent, data corruption**

A row's cells disagree with one another: the PID / User / Name cells
reflect one process while the CPU% / MEM% columns reflect a different row,
or a PID belongs to "firefox"/"gnome-shell" but the Name cell shows a
"kworker/…" thread.

Properties (per user):
- **Never fixes itself.** Once a row is in this state it stays broken —
  subsequent refreshes do *not* repopulate the desynced cells for that
  row. (This is the crucial distinction from symptom 1 and rules out a
  purely transient render glitch.) The row's underlying `ProcessRow`
  object is evidently out of sync with at least one of its bound cell
  labels, and the sync is not repaired on later `set_item` calls.
- This is the more serious bug: it means the `glib::Binding` between a
  cell `Label` and the `ProcessRow` is pointing at the **wrong** source
  object, or the `ProcessRow` being shown is not the one its PID belongs
  to.

Screenshots:
- `gtk-refresh-bug/Capture d'écran du 2026-09-06 22-08-52.png` — row 3 =
  PID 61821 · name "simpletaskmgr" · CPU 6.7%, but the ordering does not
  match CPU-descending and the row sequence is internally inconsistent
  with the rest of the visible set.
- `gtk-refresh-bug/Capture d'écran du 2026-09-06 22-08-25.png` — row 2 =
  PID 5523 (gnome-shell, 61.9%) sitting below PID 12239 (kworker, 2.7%)
  and above PID 10581 (0.0%) — the sequence 2.7% → 61.9% → 0.0% is not
  sorted, and several rows are missing entirely (they are in "blank" or
  desynced state at the same time).
- `gtk-refresh-bug/Capture d'écran du 2026-09-06 16-40-09.png` — 12.7% →
  0.0% → 5.3% → 2.7% sequence.

### 3. (Corollary) Stale sort order

Because of symptoms 1 + 2, the *apparent* row sequence is frequently not
CPU-descending. This is a downstream consequence: the SortListModel's
comparator reads the *current* `ProcessRow` data, so the visual order
being wrong means (a) some top rows are blank (not yet rendered) and/or
(b) some rows are showing the wrong `ProcessRow`'s data. It is not a
separate sort bug. `set_incremental(false)` (f89046d) did *not* reliably clear
the blank rows and did not fix symptom 2 at all, which points to a
binding/reference staleness and a render defect — neither of which a
full-re-sort emission mode addresses.

---

The two symptoms have *opposite* behaviour (one clears on scroll / several
refreshes, the other never clears), which is strong evidence they are two
separate defects in the same recycle path rather than one. Both are
consistent with a stale `glib::Binding` / stale `ProcessRow` reference on a
recycled cell, but the persistent one additionally rules out pure render
state and points at a live binding that keeps writing the wrong row's data
(e.g. a binding that was never re-targeted after `bind`, or a `ProcessRow`
object that got reused for a different PID by `refresh_list`).

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
values. That did **not** reliably fix the blank rows (the persistent-blank
cases in the screenshots still appear with this line in place), and it did
**not** fix the out-of-sync cell text (kworker name on firefox's PID). The
key data point: a full re-sort still desyncs cells and still leaves blank
rows, which points away from "sort deltas reorder things" and toward (a) a
per-cell binding/reference staleness that a full re-sort still leaves in
place, and (b) a separate GTK render/realise defect that neither the
incremental switch nor the binding fix touches. See "Not enough" below and
next steps.

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

The two fixes target the binding lifecycle, but the user observes the
persistent desync and the transient blank rows are *both* still present.
Notably:

- The persistent desync **never self-heals** and scrolling does *not* clear
  it — so it is not a render/realise glitch like the blanks; it is a
  reference that is permanently wrong for that row.
- The unit tests prove only the raw glib binding semantics (no
  drop-disconnect, two-live-bindings race) using a minimal glib stand-in — they
  do not exercise GTK's real item-factory signal ordering, nor the
  `GtkColumnView` cell pipeline, nor `refresh_list`'s in-place `ProcessRow`
  reuse.

Possible remaining holes (not yet confirmed):

1. **`refresh_list` may hand one `ProcessRow` object to two PIDs.** `refresh`
   looks up rows by PID and reuses the *same object* (`rows.remove(&pid)` /
   `set_item`) — but if two different PIDs ever map to one object (e.g. a
   PID reuse by the kernel after a short-lived process exits, or a bookkeeping
   bug in `order`/`rows`), the *name* cell and *pid* cell can legitimately
   disagree by a whole refresh. That is a data-level (not binding-level)
   desync our fix would never touch, and it would *explain the persistence*
   (the wrong source object keeps being re-notified on every refresh). This
   is the leading hypothesis now that the symptom is non-transient.
2. **A stale source object kept alive elsewhere** — detail panel, tooltip,
   context menu — holding a leaked `glib::Binding` or a strong `ProcessRow`
   ref that fires `notify` into the recycled cell.
3. **`bind` → `bind` without a real `unbind`** — if GTK4 fires `setup`/`bind`
   twice on a row widget without `unbind` for a new row, our guard in
   `bind_cell_binding` should log it (`log::warn!`) — but we have not yet
   captured logs from a real repro to confirm the guard fires at all.

## Next steps / instrumentation to pin down the real path

The persistence split is the single most useful lever: **the blank rows
clear on scroll-down/up, the wrong-name rows do not.** Use that to
separate the two defects before chasing any one of them.

0. **Exploit the scroll workaround as a diagnostic.** For a *blank* row,
   scrolling off/on re-realises it → render/row-manager staleness
   (GTK 4.18 `GtkListItemManager`). For a *wrong-name* row, scroll does
   *nothing* → the binding's source is permanently wrong. Any fix that
   doesn't survive a "scroll down then up" is a render-layer fix; the
   persistent bug needs a source-reference fix.

1. **Identify the actual binding source of a broken row (primary target).**
   The persistent wrong-name symptom means some `glib::Binding`'s *source*
   `ProcessRow` is not the row its PID belongs to. Instrument to pin it down
   without guessing:
   - In `bind_cell_binding` (src/ui.rs) log `(label=id, target_prop,
     source_pid, source_id)` on every call.
   - In `ProcessRow::set_item` (src/process_row.rs) log `(row id, pid,
     changed_props)`.
   - At the moment a desync is visible, read the cell's *current* name text
     and compare it against the `name` of `row.property` for that `row id`.
     If they differ, find which `set_item` log wrote that text — that `row id`
     is the *actual* source the live binding holds, independent of which row
     the cell is showing as PID. That either proves the binding source object
     is the wrong one, or proves the *row data itself* is wrong (see #2).

2. **Audit PID → `ProcessRow` object consistency (the data-level
   alternative).** If step 1 shows the cell and its binding source agree but
   the *object's own data* is wrong, the bug is a data-level `ProcessRow`
   being written with another process's item (e.g. a PID reuse colliding the
   map in `refresh`). Add a debug invariant in `refresh` (src/refresh_list.rs):
   after each pass, assert the PID→object map is injective and that no
   `ProcessRow` ever reports a PID different from the one it was created for
   (log old→new pid on any `set_item` with a changed pid). If such a
   collision is real, the fix is to *replace* the object rather than mutate
   it in place.

3. **Confirm the stale-binding guard is even firing.** The `log::warn!` in
   `bind_cell_binding` (the "leaked-binding guard") only fires if a *prior*
   binding was still parked on the label at bind time. If it *never* fires
   during a repro, the stale write is NOT coming from a leaked same-label
   binding — it is coming from a wrong source object (step #2) or a different
   widget. That single log is the cheapest discriminator we have.

4. **Grep all `bind_property` call sites** (not only the factory): the
   selected-process detail panel, graph-pane tooltips, context-menu label.
   Any of them holding a `ProcessRow` ref that outlives a refresh can fire
   `notify` into a recycled cell.

5. **Render-layer fix for the blank rows (separate track).** Since those
   clear on scroll, they are almost certainly a GTK 4.18 `GtkListItemManager`
   realise/measure defect. Candidate mitigations: force a `queue_draw` /
   `size_allocate` re-run after the splice, avoid the single-signal splice in
   `refresh` for the top-of-list window, or (bigger) revisit `set_item`-in-
   place vs. replacing objects so the row manager always re-binds cleanly.

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

1. Run the app; leave short-lived / reordering kernel threads (kworkers) in
   the set — they drive the constant PID churn.
2. Sort the process list by CPU (or MEM) and let the app auto-refresh (or
   toggle column sort to re-trigger re-sorts).
3. Observe:
   - **Blank gap(s) at the top** (empty space above the first row). These
     are **transient but not reliably refresh-cleared**: they may take
     *several* refreshes to clear, and **scrolling down then back up clears
     them immediately**.
   - **Wrong process name on a row** (a PID belongs to one process but the
     Name cell / columns show another, e.g. "kworker/" on firefox's PID).
     This is **persistent** — once a row is in this state it does *not*
     self-heal on later refreshes and does **not** clear on scroll.
   - **Stale sort order** (CPU% not strictly decreasing, e.g. 61.9% below
     2.7%) — a downstream consequence of the above two.

The two symptoms' opposite persistence (scroll-refreshes vs. never-clears)
is the key diagnostic: treat them as **two separate bugs**.

Screenshots captured at `gtk-refresh-bug/`:

- `Capture d'écran du 2026-09-06 16-40-09.png` — stale sort order (12.7%
  → 0.0% → 5.3% → 2.7% → 1.3%).
- `Capture d'écran du 2026-09-06 22-07-32.png` — **blank rows at the top**
  (gap above PID 5523) with the otherwise well-ordered list below.
- `Capture d'écran du 2026-09-06 22-08-25.png` — wrong order + missing
  rows: PID 5523 (gnome-shell, 61.9%) sits below PID 12239 (kworker, 2.7%)
  and above PID 10581 (0.0%); several top rows not shown.
- `Capture d'écran du 2026-09-06 22-08-52.png` — persistent desync:
  13.4% → 9.3% → **6.7% (PID 61821 "simpletaskmgr") → 1.3% → 0.7% → 0.7%**;
  the ordering/identity does not match a clean CPU-descending set.
- `Capture d'écran du 2026-09-06 22-20-23.png` — **many blanks** above PID
  12984 (RDD Process), the strongest blank-gap capture.

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

---

## Current state (2026-09-07) — investigation paused at Phase 1

### Where the tree is
- **Branch:** `fix/gtk-refresh-baseline` (off `36512ad`), HEAD = `1391918`.
- **Working tree:** only this document (`GTK_REFRESH_BUG.md`) modified
  (pending commit); untracked: `gtk-refresh-bug/` (screenshots +
  `repro-gtk-refresh.txt`).
- **Phase 0 done & committed** — the two earlier fixes (and their tests)
  are all reverted:
  - `b8c80ae` revert `set_incremental(false)`
  - `0459a1e` revert `bind_cell_binding` / `CELL_BINDING_KEY`
  - `f5437ab` revert the 3 binding-lifecycle tests for the above
- **Phase 1 done & committed** — `1391918` "Instrument the refresh path":
  - `src/process_row.rs::set_item` — `warn!` if a live row's PID changes
    (data-level PID-reuse tripwire).
  - `src/refresh_list.rs::refresh` — per-tick churn digest
    (`removed=, moved=, re_texted=, added=`) at `debug!`, plus a
    `debug_assert!` that the store ends with injective PIDs (checked on the
    in-memory `order` vec, so **no FFI** on the hot path).
  - `src/ui.rs::make_cell_factory` — `BIND_PID_KEY` qdata: park the bound
    PID on `bind`, clear on `unbind`; `warn!` on a bind-without-unbind
    (recycle-race) tripwire.

### Attempted fixes — all do NOT fix the bug (kept out of the tree)
Three changes have been tried and each one is ruled out as **the** fix:

1. **`set_incremental(false)` on the `SortListModel`** — commit `f89046d`,
   later added to `main`. Full, non-incremental re-sort on every refresh.
   *Reverted* in `b8c80ae` (Phase 0). Reason: it strictly **increases** row
   churn (GTK destroys and re-adds every row widget on each re-sort), the
   opposite of what reduces the two symptoms; and the earlier user test
   shows it neither reliably clears the blank rows nor fixes the desync.
   Note: it *is* the only thing whose presence on `main` correlates with the
   bug becoming frequent — so reverting it may be a necessary part of the
   fix — but it is a mitigation-by-removal, not a root-cause fix, and it
   alone did not make the bug go away.

2. **Discard stale cell bindings on recycle** (`CELL_BINDING_KEY` qdata +
   `bind_cell_binding()` + 3 regression tests) — commits `81cc6ac` +
   `753fd84`. Forces at most one live `glib::Binding` per cell label by
   stealing and `unbind()`-ing a prior binding before each new one.
   *Reverted* in `0459a1e` (and the tests in `f5437ab`). Reason: the Phase 1
   diagnostic that was *built to confirm* this exact failure (a
   bind-without-unbind "recycle race") **never fires** in the real repro, so
   there is no leaked same-label binding to discard — hardening the binding
   lifecycle touches a path that isn't at fault here.

3. **Phase 2A — pre-sort `items` by the active column before `refresh`.**
   Implemented on top of `1391918`, built, run under Xvfb, and *reverted*
   (working tree is back at `1391918`). Reason: it made things **worse**, not
   better — per-tick base-store churn went from `moved≈0` to `moved` 250–416.
   It also triggered `Gtk-CRITICAL: Comparison method violates its general
   contract`; that CRITICAL is notable because it **does not appear in the
   user's real repro at all** (verified 0 occurrences in
   `repro-gtk-refresh.txt`) — so it is an artifact of Phase 2A's larger
   permutation, not the original symptom. Pre-sort did not reduce
   `SortListModel` churn; it added churn.

Conclusion so far: none of the three is the fix. #1 and #2 are reverted and
staying out; #3 was a regression. The tree is at the clean Phase 1 baseline
with diagnostics, ready to catch the real fault.

### The key new data point (why Phase 2A was abandoned)
The Phase 1 baseline (Phase 0 + diagnostics) is what the user's repro ran.
The churn digest there is **near-zero on steady ticks** — `moved=0`,
`removed/added` 0–4 — i.e. the base `ListStore` is **not** churning, and the
two Phase 1 tripwires (`PID changed`, `recycle race`) **never fire** in the
repro. So, under the clean baseline:
- **No data-level PID collision** (injectivity holds; `set_item` PID-change
  is silent) — the "one `ProcessRow` for two PIDs" hypothesis from the
  earlier record is **not confirmed** by the log.
- **No bind-without-unbind recycle race** on our labels.
- **No base-store churn** — `refresh` is basically a no-op on most ticks.

The earlier "root-cause hypothesis" (leaked `glib::Binding` on a recycled
label) is the one the diagnostic was built to confirm, and the log does
**not** confirm it. The fault lives in a path the current diagnostics do not
observe yet.

### Hypotheses ruled in / out so far
| Hypothesis | Status | Evidence |
|---|---|---|
| `set_incremental(false)` is the sole cause of the bug | **ruled out** | reverting it (Phase 0) makes it *less* frequent (user report), not gone — so it's a contributing factor on `main`, not the whole story |
| base-store churn from `refresh` | **ruled out** as driver | digest shows `moved/added/removed ≈ 0` on steady ticks |
| leaked `glib::Binding` writing a stale cell (earlier "root cause") | **not confirmed** | `recycle race` tripwire silent; no bind-without-unbind captured |
| one `ProcessRow` reused for two PIDs | **not confirmed** | injectivity `debug_assert` holds; `PID changed` silent |
| SortListModel re-sort over in-place `notify` value updates is the fault | **leading, unproven** | only remaining churn path; every tick fires `SorterChange::Different` over ~15 `re_texted` rows |

### What is not yet known (the gap)
The existing repro log (930 lines, 92 refresh passes) contains **zero**
occurrences of `Gtk-CRITICAL`, `recycle race`, and `PID changed`; steady-state
ticks show `moved=0`, `re_texted` 11–31 (startup ticks spike to 162). No log
line co-occurs with the visual glitch — the glitch happened outside the
logged window (or the diagnostic path that would catch it is not yet
instrumented). **Re-capture with the glitch framed** is the immediate next
step.

### Next steps (in order)
1. **Re-capture a log with the glitch in-frame** (done: the existing log has
   zero `Gtk-CRITICAL`, zero `recycle race`, zero `PID changed`; steady-state
   `moved=0`, `re_texted` 11–31 — no spike brackets a visible glitch).
   Have the user trigger the blank-row / wrong-name state, `Ctrl-C`, and
   keep the whole log so we can locate the exact `refresh pass:` line.
2. **Confirm whether `main` reproduces the bug reliably** (it carries the
   `set_incremental(false)` + binding-lifecycle code). If `main` is
   consistently broken and `fix/gtk-refresh-baseline` is not, the fix is
   already "just don't do those two things" and we can merge the branch as
   the fix.
3. **If the glitch still reproduces on the baseline**, add one more
   targeted probe: log inside `connect_bind` / `connect_unbind` the exact
   `(label id, new/old pid, prop)` so we can tie a specific recycled cell to
   a specific row, and log `GtkSortListModel`'s own `items-changed` event
   stream (`store.connect_items_changed` on the *sort model*, not the base
   store) to see whether the re-sort on `SorterChange::Different` is the
   actual mutation path. Only then choose Phase 2B (object replacement on
   identity change) or a SortListModel-specific mitigation.

### Files in play right now
- `GTK_REFRESH_BUG.md` (this file) — earlier record + this section.
- `GTK_REFRESH_FIX_PLAN.md` — Phase 1/2 plan; Phase 2A **abandoned** (see
  above), Phase 2B/C unexecuted, Phase 3 (README/docs) unexecuted.
- `gtk-refresh-bug/repro-gtk-refresh.txt` — the user's Phase-1-instrumented
  repro; steady-state ticks are clean, no bad tick yet located.
- `src/ui.rs`, `src/refresh_list.rs`, `src/process_row.rs` — Phase 1
  diagnostics present at HEAD, no fixes applied.

