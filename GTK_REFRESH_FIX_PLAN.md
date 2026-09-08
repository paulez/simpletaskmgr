# Fix plan — persistent cell desync (wrong process name on a row)

Status: **IN PROGRESS** (this is the file the build is executing)

## Root cause (corrected earlier theory)

The Phase 1 baseline (`src/ui.rs::make_cell_factory`, `src/ui.rs:416-496`)
leaks a live `glib::Binding` on every GTK cell-recycle:

- `connect_bind` (line 491) creates a new
  `row.bind_property(prop_name, &label, "label").sync_create().build()`
  and drops the `Binding` handle. Dropping the Rust handle does **not**
  disconnect the C-side binding (established in the bug doc probe #2).
- `connect_unbind` (lines 425-438) only steals the `BIND_PID_KEY`
  diagnostic qdata. It **never disconnects the binding**.

Consequence: on `bind → unbind → bind` of a recycled cell, each cycle
piles up another live binding from a different `ProcessRow` on the same
`Label`. After N recycles, N stale rows can all push `notify` into one
label; last-writer-wins ⇒ persistent wrong-name symptom that never
self-heals, and that scrolling does not clear (scrolling re-realizes the
label's *current* value but does not retire the stale binding).

### Why the Phase 1 PID tripwire gave a false negative

`BIND_PID_KEY` parks the currently-bound PID on the label and the
`connect_bind` handler `warn!`s if `prev_pid != new_pid`. But the
`connect_unbind` handler clears the qdata. So on any legitimate
`bind → unbind → bind` cycle, the qdata is cleared in between and the
tripwire never fires — yet the underlying *binding* is still live on the
label. The diagnostic was checking the wrong indicator.

### Concluded rule

The `BIND_PID_KEY` qdata tripwire (and its `unsafe { set_data
/ steal_data }` blocks) are retired. The correct tripwire is
**"is there still a live `glib::Binding` on the label at bind time?"**
If yes → `warn!` and retire it. If no, we are on a healthy path.

## Goal

Fix the persistent cell desync **without `unsafe`**, by parking the
`glib::Binding` on a typed Rust field of a `Label` subclass rather than
in a raw qdata slot. This also gives us a clean, type-safe place to
install the real tripwire.

## Deliverable (one logical change, one branch, one commit series)

1. **New module `src/cell_label.rs`** — a small `glib::Object` subtype
   wrapping `gtk4::Label` (i.e. `ParentType = gtk4::Label`) with a
   typed `Cell<Option<glib::Binding>>` field plus `new()`,
   `take_binding()`, `set_binding(Option<glib::Binding>)` accessors.

2. **Rewrite `make_cell_factory` in `src/ui.rs`** — build
   `CellLabel` instead of `gtk4::Label`, replace all `unsafe` qdata
   accesses with the typed accessors, and keep *only* the corrected
   tripwire `warn!` (stale binding on recycled label at bind time).

3. **Regression tests** — a fresh test in `src/cell_label.rs` proving:
   - a bind → unbind → bind-on-different-row cycle leaves exactly one
     live binding on the label, and
   - a stale row's `notify` no longer writes to the label after the
     unbind.
   Both use the existing `crate::testutil::test_item` fixtures.

4. **Update `GTK_REFRESH_BUG.md`** — replace the "leading hypothesis"
   line with the confirmed root cause and note the false-negative
   tripwire so the next reader doesn't repeat Phase 0's mistake.
   Do not delete the earlier record; append a short "Phase 1.5 —
   confirmed root cause" section.

5. **README / public doc** — no user-visible change, so the existing
   README stays as-is unless `make_cell_factory`'s doc comment needs to
   be refreshed (it will — it currently describes the PID diagnostic).

6. **No changes** to `refresh_list.rs` or `process_row.rs` — they are
   not at fault; the churn-digest diagnostics and the `set_item`
   PID-change tripwire stay.

## Test strategy

Follow the repo's test discipline (`#[cfg(test)] mod tests`, `test_`
prefix, `super::*`).

### Unit tests for `CellLabel`

- `test_cell_label_take_and_set` — empty label: `take()` returns
  `None`; `set(Some(b))`; `take()` returns `Some(b)`; `take()` again
  returns `None`.
- `test_cell_label_bind_then_unbind_does_not_leak` — the exact
  desync scenario. Given two `ProcessRow`s (`row_a`, `row_b`) and one
  `CellLabel`:
  1. `bind(row_a) + take()` (simulate what the factory does); `set(...)`
     to park the binding; read the label text → must be `row_a`'s value.
  2. `take()` + `b.unbind()` (simulate the factory's unbind).
  3. `bind(row_b) + set(...)`.
  4. `row_a.set_item(other)` (a `notify` from a stale row).
  5. Assert the label still shows `row_b`'s value, not
     `row_a`'s.

### Unit tests for `make_cell_factory`

Keep the existing factory intact but route it through `CellLabel`. Add:

- `test_cell_factory_recycle_preserves_last_binding` — simulate
  `setup → bind(row_a) → unbind → bind(row_b)` once, then
  `row_a.set_item(other)`. Assert the label text matches `row_b`.
  The `warn!` (corrected tripwire) does not fire because we unbound
  properly.
- `test_cell_factory_recycle_fires_tripwire_if_unbind_missing` —
  simulate `setup → bind(row_a) → bind(row_b)` (no unbind):
  `captured_logs` (via `env_logger::try_init` or a simple
  `log` test harness) must contain the `stale binding` warning.
  If the log-capture harness is too heavy for this repo, assert the
  invariant on the label directly: after the second `bind`, the label
  must reflect `row_b` (the guard retired the stale binding).

## Execution checklist (per AGENTS.md workflow)

- [ ] Write `GTK_REFRESH_FIX_PLAN.md` (this file).
- [ ] Add `src/cell_label.rs` with the `CellLabel` module + unit tests.
- [ ] Rewrite `make_cell_factory` in `src/ui.rs` to use `CellLabel`;
      remove the `BIND_PID_KEY` qdata tripwire.
- [ ] Add / update the `make_cell_factory` tests.
- [ ] Update `GTK_REFRESH_BUG.md` with the corrected root cause.
- [ ] Run the gate:
  `cargo check && cargo test && cargo clippy --all-targets && cargo fmt --check`.
- [ ] Commit logical changes in order (new module, factory rewrite,
      tests) with clear messages. Do not push.
