# Refresh flicker — Option A failure and the F1 fix

## The problem

After switching to top-style order ownership (Option A: the base store is
spliced into the active column's display order on every refresh, so GTK's
`SortListModel` re-sort becomes a no-op), the app regressed:

1. Repeated `Gtk-CRITICAL: Comparison method violates its general contract`
   from GTK's timsort merge (`gtk/timsort/gtktimsort-impl.c`, `merge_lo`/
   `merge_hi`, emitted when one run is exhausted while its merge partner
   still has >1 elements — a mid-merge transitivity violation).
2. A visibly wrong row order after such a merge (the violated merge
   `return`s early, leaving the array partially merged).

We reverted Option A and landed **F1** instead.

## What the GTK source actually says (4.18.6)

- `GtkSortListModel` never watches item properties: only `items-changed` or
  the sorter's `changed` triggers a re-sort (`gtksortlistmodel.c`,
  `items_changed_cb` bails on `removed==0 && added==0`). So a value-only
  refresh must ask the sorter to re-run — the "kick".
- `GtkSortListModel`'s compare (`sort_func`) delegates to the sorter and
  breaks comparator **ties by item-pointer order** (`*sa < *sb ? -1 : 1`).
  A stable sort with the same comparator (whose own tie-breaks differ per
  column) therefore settles ties differently than GTK.
- The model sorts **synchronously** by default (`incremental` is
  `false`), so a kick is one whole-array pass that commits the entire
  `(0, N, N)` range when the order moves.
- `GtkColumnViewSorter` negates the primary comparator's result for the
  descending direction (`gtkcolumnviewsorter.c`).

## Why Option A's order still wasn't the no-op promise (best hypothesis)

The refresh spliced the base store into a target order and relied on GTK's
re-sort being an identity. The engines can legitimately take different tie
orderings (pointer order vs. base-position stability vs. a column's own
tie-break), and any residual reorder flows through the *incremental* merge
path — the path that hit the timsort critical and left the list partially
merged. Owning the order in Rust while GTK still re-sorts in parallel was
the unstable half of the design; F1 drops that split.

## F1: skip the kick when the order is unchanged

Keep GTK as the owner of the view order (the proven path). On each refresh,
*before* kicking, compute whether the kick would actually move anything:

- Build the target order exactly as GTK would: replicate the comparator
  (`ProcessList::compare_values`, negated for a descending primary) and GTK's
  pointer tie-break, over the rows' current values.
- Read the `SortListModel`'s current row order (pid sequence).
- If they are identical, the re-sort is an identity: **skip the kick**. The
  refresh then commits nothing and the list stays pixel-stable — including
  scrolled down, where the whole-list commit is the visible glitch.
- If they differ, fire the kick (the same `sorter -> changed` path a header
  click takes) so the list re-sorts deliberately, once.

`src/ui.rs::view_order_changed` is the gate; a focused test
(`test_view_order_changed_tracks_gtk_view`) proves the replica tracks the
real GTK view order — ties included — and that the kicked re-sort converges
back to "unchanged".

The `--no-resort-kick` flag remains as a diagnostic (it forces the skip).
