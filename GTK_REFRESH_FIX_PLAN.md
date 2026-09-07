# GTK Refresh Bug — Revised Fix Plan

Status: **IN PROGRESS** (supersedes the earlier two-attempt log in
`GTK_REFRESH_BUG.md`, whose "two separate bugs" framing is retained as a
diagnostic but whose root cause is re-assessed below).

## Fresh diagnosis

Both prior fixes treat symptoms; the common **driver** they never touch is
churn. On **every** refresh tick the pipeline runs two full orderings:

1. `refresh_list::refresh(store, items)` splices the base `ListStore` into
   **`/proc` enumeration order** (`process_list.rs` builds `processes` by
   iterating `/proc`), so the base order churns each tick as kworkers
   die/spawn. (`refresh_list.rs:106`)
2. Then the `SortListModel` **re-sorts that churning base** into the active
   display column.

The upshot: `GtkListItemManager` recycles a large batch of row widgets
**every tick** — and #1's `set_incremental(false)` makes that a *full*
clear+re-add each tick = maximum churn. Churn is what both prior fixes missed.

Reconciliation of the two symptoms: churn *re-creates* desync races faster
than old ones clear (so a bad row looks "persistent" across refreshes), and
the same recycle path is where the transient blanks originate. This explains
both the opposite persistence and why neither the incremental switch nor the
binding-lifecycle hardening moved the needle.

Supporting evidence already in the codebase: `cpu_tracker.rs:155` and
`io_tracker.rs:101` already special-case **PID reuse** (a pid whose owner
changed) — the app is already aware pid-reuse is a real event, but the row
modeling (`ProcessRow::set_item`) does *not* treat pid change as an identity
change: it mutates the same object in place.

## Execution phases

### Phase 0 — revert both prior fixes (clean baseline)

All three fix commits touch only `src/ui.rs`:

- `f89046d` — `set_incremental(false)` (+1 comment)
- `81cc6ac` — `bind_cell_binding` / `CELL_BINDING_KEY` recycle fix
- `753fd84` — 3 glib-binding regression tests

Revert newest→first, one granular commit each (`753fd84`, `81cc6ac`,
`f89046d`). Restores the plain `bind_property` call site and the default
(incremental) `SortListModel`. No other site references the reverted symbols.

### Phase 1 — instrument (diagnostic commit; stays in at `debug!`/`warn!`)

Three cheap, cheap-to-remove probes that together identify the active fault
without guesswork:

- `process_row.rs::set_item` — `log::warn!` **when the PID itself changes**
  on a live row (data-level PID-reuse lock-in; the "kworker on firefox's pid"
  hypothesis, root cause #4).
- `refresh_list.rs::refresh` — per-tick churn digest (removed/moved/added/
  re-texted counts) at `debug!`, plus `debug_assert` that after the pass the
  store's PIDs are **injective** (no two rows share a PID).
- `ui.rs::connect_bind` — park the previous bound PID in qdata (a cheap
  `i32`, no `Binding`-lifetime gymnastics); if the PID changed for the label,
  `log::warn!` so a recycle-race desync is visible at bind time.

The `warn!` lines are the discriminators. To capture them the user runs the
app with `-v` (already wired: `-v` → `LevelFilter::Debug`) during a normal
repro and shares the log. **This log decides the Phase-2 fix below.**

### Phase 2 — the fix, chosen by the log

Ranked (default = the churn one, since it's highest probability +
reversible):

- **A. Stop feeding the SortListModel a churning base.** In `make_rebuild`
  (ui.rs), **sort `items` by the active display column before calling
  `refresh`** (read `ColumnViewSorter::primary_sort_column` + current
  order). The base store then arrives **already in display order**, the
  `SortListModel` re-sort becomes a near-no-op, and per-tick recycle churn
  collapses to the real delta. This is pure logic (testable: sort over
  `items` × column × direction) and attacks the common root cause of both
  symptoms at once.
- **B. Fix the PID-reuse hole (only if the PID-change `warn!` in Phase 1
  fired):** when a PID's *identity* changes across a refresh, **replace** the
  `ProcessRow` object (new `ProcessRow::from_item`) via a fresh `splice`
  rather than mutating it in place. `set_item` in-place is fine for
  value-only changes; identity changes need a new object.
- **C. Blank-row render quirk (only if A doesn't fully kill it):** after the
  splice pass in `make_rebuild`, call `column_view.queue_resize()` (or
  `size_allocate`) to force GTK's row-manager to re-measure — a targeted
  workaround for a known GTK 4.18 `GtkListItemManager` stale-row defect.

**Do not** keep `set_incremental(false)` — it strictly increases churn and
is the opposite of what A does.

### Phase 3 — docs

- `GTK_REFRESH_BUG.md`: mark RESOLVED (or PARTIALLY, with the surviving
  symptom named), record which Phase-2 path was taken and the log that
  decided it.
- `README.md`: only if behaviour changes visibly (it shouldn't, modulo
  ordering — which stays CPU-desc by default).

## Verification

- Unit-test the Phase-2A pre-sort helper (pure: `items` + column + direction
  → expected pids); a `refresh` test with a **duplicate PID** must panic the
  injectivity `debug_assert` (guards against a data-level pid-collision).
- Existing lib + integration suites stay green.
- Every commit: `cargo check && cargo test && cargo clippy --all-targets
  && cargo fmt --check`.

## Decision points

1. Keep the Phase-1 `warn!`/`debug!` probes in the tree once the bug is
   fixed? (default: keep — they are cheap and are the first line of future
   diagnosis; remove only if noisy.)
2. If Phase-2A alone clears both symptoms, do **not** also do B or C; the
   principle is to touch only what the log justifies.
