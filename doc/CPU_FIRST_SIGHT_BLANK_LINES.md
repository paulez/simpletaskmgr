# CPU first-sight sort key → blank-line bursts (investigation + fix)

Date: 2026-07-09. Status: **fixed** (Option A, see below).

Symptom: after the CPU% sort switched from raw f64 to the integer tick-delta
key (pid tie-break), the blank-line bug — visible as fully blank rows at the
top of the process list — started appearing on **most** refreshes
(`../screenshots/blank_lines.png`).

## What the blank lines are

Not empty data. A blank row is a `GtkListItem`/cell widget that was
**destroyed and not yet re-created/configured**: `GtkListItemManager` reacts
to `items-changed` by tearing down every row widget inside the changed range
and rebuilding it (see the anti-flicker contract in
`src/refresh_list.rs`). The width of that range is what GTK decides.

## How GTK 4.18's `SortListModel` sizes the change range

Source: `gtk/gtksortlistmodel.c`, branch `gtk-4-18`.

- `gtk_sort_list_model_items_changed_cb` (store add/remove path): after
  re-sorting, GTK computes `start` = first display index whose *store*
  position is at/after the changed region and `end` = size of the untouched
  tail, then emits a **single**
  `g_list_model_items_changed(self, start, n_items - added + removed, n_items)`.
  The emitted range therefore runs **from the new row's display position out
  to the nearest unchanged boundary**.
- `gtk_sort_sorter_changed` path (data-only refresh, our `changed(Different)`
  kick): emits over the total range the timsort merge touched
  (`gtk_sort_list_model_sort_step` + `gtk_tim_sort_step`), which for a
  top-row falling into the middle of the list is the whole list.
- `sort_func` breaks comparator ties by base position (stable), so the old
  f64 comparator's "tie" group kept its base (≈ pid-ascending) order.

Neither emission size depends on row-count churn per se — it depends on
**where in the display the incoming/moving row sits**.

## Why the tick-key change amplified it

The CPU column descends on `(cpu_ticks, pid)`. Existing rows carry a
**per-window delta** as key (a few ticks per 1.5 s window). A first-sight or
pid-reused row carries `utime + stime` — **lifetime ticks** (`top`-faithful
first frame, `src/cpu_tracker.rs`) — which is *always* larger than any
existing row's window delta. So:

| | key on first sight | where new procs land | emission range |
|---|---|---|---|
| old (f64 percent) | since-start average — tiny for the idle procs a desktop spawns | **bottom** of the descending list | ~1-row insert at the bottom, invisible |
| new (tick key) | lifetime ticks — always top | **top of the list** | range starts at 0 → **whole visible list destroyed** → blank band |

Then on the next refresh the same row's key collapses from lifetime ticks to
its real per-window delta: a whole-list re-permutation → a second
`items-changed(0, N, N)` → second blank burst (the "fall").

A desktop (GNOME + a heavy browser) spawns short-lived/idle processes
continuously, so every few refreshes: one top-landing + one fall, both
emitting over the visible top → **blank lines on most refreshes, at the
top of the list**. Matches the report exactly.

Note: `top`'s first-frame quirk (new task briefly ranked by its since-start
ticks) is real and is what we copied — but `top` repaints rows in place
(terminal); here the same key ordering drives GTK widget churn, so the
fidelity is not free.

## Fix options considered

- **A (chosen): first-sight & pid-reuse rows get `cpu_ticks = 0`,
  `cpu_percent = 0.0`.** Fresh rows land in the zero group (bottom, like they
  did under the old key), then rise to their true bucket within 1–2 windows
  from measured deltas. Removes *both* blank bursts (landing + fall). Cost:
  one frame of 0.0% for a fresh process; the since-start average display is
  dropped (`top` first-frame quirk no longer reproduced).
- **A′: key 0, but still display the since-start average.** Keeps the
  top-faithful number, but shows e.g. "8%" sitting at the bottom of a
  CPU-descending list for one frame — visibly non-monotonic; looks like a
  bug. Rejected.
- **B: keep top-faithful keys, delay new rows entering the store until they
  have one measured delta.** Still one big emission when the row enters;
  adds tracker "pending" state and complexity. Rejected.

## Changes

- `src/cpu_tracker.rs`: vacant and pid-reuse paths set `cpu_ticks = 0`,
  `cpu_percent = 0.0`; `lifetime_avg_percent` removed (now dead).
- `src/process.rs`, `src/metrics.rs`: doc lines updated.
- `README.md`: calm-CPU-ordering bullet no longer claims first-frame entries
  are tied to lifetime ticks.
- Tests in `src/cpu_tracker.rs` updated to expect the zero key/percent on
  first sight and pid reuse.
