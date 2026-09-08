# Fix plan — persistent cell desync (wrong process name on a row)

Status: **COMPLETED (2026-09-08) — the desync no longer reproduces.**
`main` is at the fix. See `GTK_REFRESH_BUG.md` "Phase 1.5" for the outcome
and the "why did this work where v1 did not" analysis. The two deviations
from this original plan are noted inline (the `CellLabel` subclass was not
possible; the tests were placed in the `cell_label` module).

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

1. **New module `src/cell_label.rs`** — a safe, thread-local parking slab
   keyed by `label.as_ptr() as usize` (a `std::collections::HashMap<usize,
   glib::Binding>` in a `thread_local!` `RefCell`) plus `park`, `take`,
   `take_and_unbind`, `reset_for_tests`, and a minimal `TestTarget`
   writable `glib::Object` for the tests. **Deviated from this plan:** the
   originally-sketch `CellLabel` `glib::Object` subclass wrapping
   `gtk4::Label` was not possible — `gtk4::Label` is *sealed* in gtk4-rs
   0.11 (not `IsSubclassable`), so we cannot subclass it. The slab provides
   the same single-per-object park/take contract with **no** `unsafe`.

2. **Rewrite `make_cell_factory` in `src/ui.rs`** — replace all
   `unsafe` qdata access (`set_data`/`steal_data`) and the
   `BIND_PID_KEY` (i32) diagnostic with the slab's `park` /
   `take_and_unbind`, and keep the corrected tripwire `warn!` (a stale
   binding still parked on a recycled label at bind time → the previous
   `connect_unbind` did not run). Retirement happens in **both**
   `connect_unbind` (happy path) and `connect_bind` (defensive).

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

### Unit tests for the parking slab (`src/cell_label.rs::tests`)

- `test_key_unique_and_stable` — the parking key (an `as_ptr` pointer cast
  to `usize`) is unique per `glib::Object` and stable across calls for the
  same object.
- `test_parking_slot_roundtrip` — empty target: `take()` returns `None`;
  `park()` stores a `glib::Binding` on the key; a second
  `take()` returns the binding; a second `take()` returns `None`.
- `test_take_and_unbind_disconnects` — park a binding, `take_and_unbind`
  removes it from the slab **and** the C-side `glib::Binding` is
  disconnected (the slab's `Binding` is what makes the C-side binding go
  away, not just the Rust handle).
- `test_recycled_target_stale_source_sealed` — the exact desync scenario.
  Two `ProcessRow`s (`row_a`, `row_b`) and one `TestTarget` (a writable
  `glib::Object` that stands in for a `gtk4::Label`):
  1. `park(row_a's binding)`. `TestTarget.name` == `row_a.name` (via
     `sync_create`).
  2. `take_and_unbind(key)` (simulates the factory's `connect_unbind`).
  3. `park(row_b's binding)`.
  4. `row_a.set_item(other)` (a stale `notify` from a row that is no longer
     the target's binding source).
  5. Assert `TestTarget.name` == `row_b.name`, not `row_a.name`.

Because `gtk4::Label` is sealed (cannot be subclassed/constructed in a
unit-test context without GTK initialization) and the parking slab only
needs any `glib::Object` as a binding target, `TestTarget` (a minimal
writable `glib::Object` with one `name` string property) is used instead of
a full `gtk4::Label`. This exercises the parking slab and the
`glib::Binding` lifecycle in isolation — it does *not* exercise GTK's real
`GtkColumnView` factory signal ordering or the `GtkListItemManager` recycle
lifecycle. The empirical proof is the user's running-app observation (see
`GTK_REFRESH_BUG.md` Phase 1.5 for the caveat).

### Factory-level regression test for `make_cell_factory`

The factory in `src/ui.rs` is exercised indirectly by the integration
tests in `tests/integration_tests.rs`. The unit tests above cover the slab
API. No separate `test_cell_factory_*` tests were added because a
`SignalListItemFactory` requires a full `GtkColumnView` + `GtkList` in
flight, which is not exercised by the unit-test harness (see the "Notes"
section of `GTK_REFRESH_BUG.md` for the same caveat from the earlier v1
attempt). The `make_cell_factory` rewrite is covered by the compilation
itself (clippy, fmt) and the running-app observation.

## Execution checklist (per AGENTS.md workflow)

All items done and committed on `fix/gtk-refresh-baseline` (merged into
`main`); the branch and `main` both sit at the fix + docs commits.

- [x] Write `GTK_REFRESH_FIX_PLAN.md` (this file).
- [x] Add `src/cell_label.rs` with the parking-slab module + unit tests
      (the original `CellLabel` subclass plan is not possible; see
      Deliverable 1 for the deviation note).
- [x] Rewrite `make_cell_factory` in `src/ui.rs` to use the slab; remove
      the `unsafe` qdata and the `BIND_PID_KEY` (i32) diagnostic.
- [x] Add the parking-slab regression tests (see Test strategy section).
- [x] Update `GTK_REFRESH_BUG.md` with the corrected root cause and a
      "Phase 1.5 — resolution" section noting why v1 did not work and
      this fix does.
- [x] Run the gate: `cargo check && cargo test && cargo clippy --all-targets
      && cargo fmt --check` (321 lib + 6 doc + 3 integration tests pass;
      clippy and fmt clean).
- [x] Commit logical changes in order:
  - (a) `src/cell_label.rs` (new module + tests), `src/lib.rs`
    (mod addition), `src/ui.rs` (factory rewrite) → commit `38d0b96`.
  - (b) `GTK_REFRESH_BUG.md` (Phase 1.5 + earlier status lines) and this
    plan file → commit `af3c353`.
  - `main` fast-forwards to `af3c353` (no push performed; `origin/main`
    is still a pre-fix commit behind — push it on the next deploy cycle).
