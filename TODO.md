# TODO

Tracked known-issues and planned improvements, roughly ordered by priority.

## Non-reactive process rows (performance)

**Status:** deferred / needs a visual test before merging

Each process row captures its `TaskMgrProcess` value by value and renders non-reactively
(`src/process.rs:IntoView`, `src/ui.rs:process_item_view`). The `dyn_stack` key is the whole
struct (`src/ui.rs:process_list_view`), so a row is only rebuilt when one of its displayed
values (e.g. CPU%) actually changes. This is correct but means a changed row is rebuilt rather
than updated in place.

**Proposed approach (B):** key rows on the stable `pid` and make each label read a per-process
value that is swapped reactively on refresh (the idiomatic floem model: stable identity +
reactive values). This would stop all row churn.

**Note:** this is a UI change that cannot be verified headlessly (`cargo test`); it must be
validated visually once implemented.
