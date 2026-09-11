# Residual "reorder" flicker — investigation and the top-style fix

The blank-row crash (`CPU_FIRST_SIGHT_BLANK_LINES.md`) landed as
"first 20 s of launch". What survived behind it was a different, smaller
flicker: on many refresh ticks the list visibly *re-ordered* (rows
shuffled), and it got noticeably worse after the user had scrolled down
and back up.

## What we measured (GTK 4.18.6 C sources + app logs)

1. **The re-sort was our own.** `make_rebuild` kicked the
   `SortListModel` (`sorter.changed(Different)`) after **every** refresh.
   In GTK's `gtk_sort_list_model_sorter_changed`, a kick runs
   `gtk_sort_list_model_start_sorting(self, NULL)` — the **slow path with
   no runs**: the whole list re-sorted from scratch. Any reorder in the
   new order then commits as a **whole-list** `items-changed` (range
   `(0, N, N)`), not just the moved rows.

2. **A whole-list commit touches every row widget.** GTK's
   `GtkListItemManager` (`gtk418_listitemmanager.c`) is *fully
   synchronous* inside the signal — it does not batch to idle: it removes
   all row widgets, runs the change tracker, re-adds all, materializes
   every row in the visible range (`ensure_items`), queues a resize and
   re-enters layout. With 514 rows that is ~500 widget mutations per
   tick. The per-item widget cache is **identity key** (not position) —
   so GTK *does* recycle the `GtkListItem` widgets — but it still has to
   unparent/reparent all of them, redo layout and re-run the
   adjustment-clamp dance. **Fresh start (≈30 live widgets) this is
   invisible; after scrolling (hundreds of live widgets) it is the
   visible glitch.** That matches the user's observation exactly
   (no flicker until the list has been scrolled).

3. **Value-only changes cannot trigger a re-sort at all.** In
   `gtksortlistmodel.c`, `gtk_sort_list_model_items_changed_cb` bails out
   when `removed == 0 && added == 0`. Only a `sorter -> changed`
   notification forces a re-sort. So the only way for a value update to
   move rows in order was the kick — and the kick always paid the
   whole-list price.

## What "no kick" proved (`--no-resort-kick`, commit `8e5e720`)

With the kick disabled, the list is pixel-stable across refreshes
(0 removed / 0 moved / 0 added; only label re-text). Confirmed on the
user's machine for several minutes — including after scrolling. (The
`--no-resort-kick` flag was later deleted: it only masked the problem —
rows froze at their last order instead of re-sorting on value change.)

## The fix: own the display order (`top`-style)

`top` keeps its own sorted array and re-sorts it in its own loop on
every sample; there is no view layer to negotiate with. We did the same:

1. **`compare_values` became a total order** — every comparator arm breaks
   remaining ties on `pid`. This is the prerequisite: if ordering of tied
   rows depended on sort *stability*, the Rust sort (stable over `/proc`
   order) and GTK's `SortListModel` sort (stable over the base order)
   could land in different orders and the view would visibly re-sort the
   base store on the next commit. With an explicit `pid` tie-break the
   order is independent of the underlying input order (commit `4b8ae5b`).
2. **The refresh path builds the target in display order** — it reads the
   active sort column and direction from the view's own
   `ColumnViewSorter`, sorts the fresh values with
   `ProcessList::sort_in_display_order` (the exact
   `ColumnViewSorter` arithmetic: `compare_values` fed the live
   direction, negated for the descending view), and splices the base
   store into that order. GTK's `SortListModel` therefore finds the base
   *already in view order* and its re-sort is identity → **no
   `items-changed` at all** (commit `9b8bbfc`).
3. **The kick and the `--no-resort-kick` flag are gone** (commit
   `46fc7f1`).

Result per refresh tick, in order of cost:

| Case | Commit | Cost |
| --- | --- | --- |
| values changed, order unchanged | none (label re-text only) | minimal |
| order changed | `items-changed` only for the moved rows (widget cache recycles them) | small |
| membership added/removed | membership splice | small |
| header click (user-initiated re-sort) | one whole-list commit | expected — the list visibly re-sorts exactly when asked |

## Verification

* Unit tests: `compare_values` is a total order for every column ×
  direction; `sort_in_display_order` reproduces the
  `ColumnViewSorter` result (CPU buckets, `None` pinning in both
  directions, tie-break negation) and is independent of input order.
* Live app (`-v`): refreshes log 0 removed / 0 moved / 0 added and only
  label updates; scrolling no longer makes refresh flicker; a header
  click still re-sorts in one visible animation.
