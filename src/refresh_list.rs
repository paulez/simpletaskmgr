//! Refreshes the process list `gio::ListStore` in place.
//!
//! Instead of tearing down and rebuilding the whole store (which empties it
//! momentarily, clamps the surrounding scroll adjustment, and makes the
//! `gtk4::ColumnView` flash empty), [`refresh`] diffs the old and new pid
//! sequence and mutates only the rows whose pid, position, or data changed.
//!
//! Every process keeps **one** [`ProcessRow`] `glib::Object` for its whole
//! life. A data change writes into that object via
//! [`ProcessRow::set_item`] (re-emitting the changed properties, so the
//! property-bound cell labels follow) — the object keeps its identity, so
//! *no store signal fires at all* for a stable-position value update. A position change is applied
//! with the `splice` primitive (see [`store_splice`]): because GTK 4.18's
//! `GtkListItemManager` only reuses an existing row widget when the remove
//! *and* the re-add of an item arrive inside **one** `items-changed` signal
//! (its `deleted_items` widget cache is created per signal and unparented at
//! the end of each signal handler), a single splice that removes and re-adds
//! the same item pointer makes GTK keep the same `GtkListItem` widget —
//! reparenting it into its new position — instead of destroying it and
//! allocating a fresh one.
//!
//! (A plain `remove` + `insert` move does *not* enjoy that: the two
//! separate signals mean the widget is unparented — and freed — by the time
//! the insert runs, so `GtkListItemManager` falls back to allocating a
//! fresh row. That destroy/recreate pair is exactly the blank flash seen
//! when the sort order changes.)

use std::collections::{HashMap, HashSet};

use glib::prelude::*;

use crate::process::ProcessItem;
use crate::process_row::ProcessRow;
use gtk4::gio::prelude::*;
use gtk4::gio::ListStore;

type GObjectPtr = *mut glib::gobject_ffi::GObject;

/// Applies one `g_list_store_splice` mutation: removes `n_removals` items at
/// `position` and inserts the `additions` in their place, emitting a
/// **single** `items-changed(position, n_removals, n_additions)` signal.
///
/// `g_list_store_splice` (glib ≥ 2.50, present in the 4.18 runtime) is
/// declared by `gio-sys` but not wrapped by the 0.22 `ListStore` bindings,
/// so we call it through the FFI. The single-signal emission is what lets
/// GTK 4.18's `GtkListItemManager` reuse a row widget that is removed and
/// re-added inside the same signal — two separate `remove`/`insert` calls
/// (two signals) lose the widget (see the module docs).
///
/// # Safety
/// `additions` must outlive the call and hold one `g_object_ref` each;
/// `n_removals` + `additions.len()` must fit the store.
fn splice(store: &ListStore, position: u32, n_removals: u32, additions: &[GObjectPtr]) {
    unsafe {
        // `additions` is only read by the C function; the `as_mut_ptr`
        // signature is an artifact of the FFI declaration.
        gtk4::gio::ffi::g_list_store_splice(
            store.as_ptr(),
            position,
            n_removals,
            additions.as_ptr() as *mut GObjectPtr,
            additions.len() as u32,
        )
    };
}

/// Moves the row currently at `src` to `dst` (a *left* move, `src > dst`)
/// as one `items-changed` signal: the window `dst..=src` is removed and
/// re-added with the moved row first.
fn splice_move(store: &ListStore, dst: u32, src: u32) {
    let block: Vec<GObjectPtr> = (dst..=src)
        .map(|p| store.item(p).expect("row present in window").as_ptr())
        .collect();
    // Desired window: the moved row (last element of `block`) first, then
    // the original `dst..src` in order — so every slot is re-filled by an
    // existing item pointer and GTK reuses each slot's widget.
    let mut additions = block[block.len() - 1..].to_vec();
    additions.extend_from_slice(&block[..block.len() - 1]);
    splice(store, dst, block.len() as u32, &additions);
}

/// Replace the contents of `store` with `items`, mutating it in place so the
/// `gtk4::ColumnView` rendering it does not lose its rows (or its scroll
/// position) one tick at a time.
///
/// Selection is intentionally **not** handled here — the caller decides what
/// to do with it around this call.
///
/// In a left-to-right walk over the target, once position `t` is fixed no
/// later fix can touch indices < t (moves are `splice_move`(window `t..=src`)
/// with `src > t`, and new pids only append into the tail), so the
/// already-fixed prefix never changes again. Pids are unique, therefore at
/// step `t` the row that should sit at `t` can only be at index `t` or
/// somewhere after it.
///
/// Rows whose pid **and data** are unchanged at their current position are
/// left completely untouched — zero store mutations, zero widget churn for
/// them. A row whose data changed but that stays put is re-texted in place
/// with [`ProcessRow::set_item`] (no store mutation). A row that moves keeps
/// its existing [`ProcessRow`] object across the reinsert (and is re-texted
/// if its data changed, too), so GTK recycles its widget instead of creating
/// a new one.
///
/// `order` mirrors the store contents and is mutated in lock-step with the
/// store, so row lookups are plain `Vec` scans, not FFI round-trips.
pub fn refresh(store: &ListStore, items: &[ProcessItem]) {
    let old_n = store.n_items();
    let mut order: Vec<i32> = Vec::with_capacity(old_n as usize);
    let mut rows: HashMap<i32, ProcessRow> = HashMap::with_capacity(old_n as usize);
    for i in 0..old_n {
        let o = store.item(i).expect("row present in store");
        let r = o.downcast::<ProcessRow>().expect("a ProcessRow");
        let pid = r.item().pid;
        order.push(pid);
        rows.insert(pid, r);
    }

    let target_pids: HashSet<i32> = items.iter().map(|i| i.pid).collect();

    // Pass 1: drop rows whose pid is not in the target set. Contiguous runs
    // are removed with a single `splice` (one `items-changed` signal each).
    let mut to_remove: Vec<u32> = Vec::new();
    for (i, pid) in order.iter().enumerate() {
        if !target_pids.contains(pid) {
            to_remove.push(i as u32);
        }
    }
    to_remove.sort_unstable();
    // Group contiguous runs, then remove the runs back-to-front so the
    // earlier runs' indices stay valid as the store shrinks.
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut i = 0usize;
    while i < to_remove.len() {
        let mut end = to_remove[i];
        let mut j = i + 1;
        while j < to_remove.len() && to_remove[j] == end + 1 {
            end = to_remove[j];
            j += 1;
        }
        runs.push((to_remove[i], end - to_remove[i] + 1));
        i = j;
    }
    for (pos, len) in runs.iter().rev() {
        splice(store, *pos, *len, &[]);
    }
    order.retain(|p| target_pids.contains(p));
    rows.retain(|p, _| target_pids.contains(p));

    // Pass 2: walk the target left-to-right (invariants above).
    for (t, item) in items.iter().enumerate() {
        let pid = item.pid;

        if t < order.len() && order[t] == pid {
            // Same pid, same position. If the data differs, re-text the
            // object in place (its on-screen labels repaint via
            // `set_item`); otherwise there is nothing to do at all. No store
            // mutation means no `items-changed` signal and no widget churn.
            if !rows[&pid].has_value(item) {
                rows[&pid].set_item(item);
            }
            continue;
        }

        if let Some(src) = order
            .iter()
            .skip(t)
            .position(|p| *p == pid)
            .map(|rel| t + rel)
        {
            // A move: reuse the *same* row object. `splice_move` removes and
            // re-adds the window `t..=src` in **one** `items-changed` signal,
            // which is what makes GTK 4.18's row manager keep every slot's
            // existing widget and reparent it — instead of a `remove` +
            // `insert` pair (two signals) that destroys the widget and
            // rebuilds a blank one. No blank flash.
            let row = rows.remove(&pid).expect("row for a kept pid");
            if !row.has_value(item) {
                // Its data changed while it moved: re-text it.
                row.set_item(item);
            }
            splice_move(store, t as u32, src as u32);
            order.remove(src);
            order.insert(t, pid);
            rows.insert(pid, row);
        } else {
            // A brand-new pid: insert a fresh row here (or at the tail).
            let fresh = ProcessRow::from_item(item);
            if (t as u32) < store.n_items() {
                store.insert(t as u32, &fresh);
            } else {
                store.append(&fresh);
            }
            order.insert(t, pid);
            rows.insert(pid, fresh);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(p: i32) -> ProcessItem {
        crate::testutil::test_item(p)
    }

    /// Seed a fresh `ListStore` with `ProcessRow`s for the given pids in order.
    fn seed(pids: &[i32]) -> ListStore {
        let s = ListStore::new::<ProcessRow>();
        for p in pids {
            s.append(&ProcessRow::from_item(&item(*p)));
        }
        s
    }

    fn pids_of(s: &ListStore) -> Vec<i32> {
        (0..s.n_items())
            .map(|i| {
                s.item(i)
                    .expect("row present")
                    .downcast::<ProcessRow>()
                    .expect("a ProcessRow")
                    .item()
                    .pid
            })
            .collect()
    }

    /// A property-style invariant: for a handful of starting lengths and
    /// target lengths, `refresh` must always land the store exactly at the
    /// requested target — same pids, same order.
    #[test]
    fn test_refresh_end_state_small() {
        for start_len in [0, 1, 3, 5, 10] {
            for target_len in [0, 1, 3, 5, 10] {
                let start: Vec<i32> = (1..=start_len).collect();
                // A fixed target that overlaps the start by construction.
                let target: Vec<i32> = (start_len - target_len + 2..=start_len + 2).collect();
                let items: Vec<ProcessItem> = target.iter().map(|p| item(*p)).collect();

                let s = seed(&start);
                refresh(&s, &items);

                assert_eq!(pids_of(&s), target, "start={start:?}");
            }
        }
    }

    /// A randomised end-state test: N random start lists, N random target
    /// lists, `refresh` must always produce the exact target store.
    #[test]
    fn test_refresh_end_state_random() {
        let mut state: u32 = 0x9E37_79B9;
        let mut next = move || {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223)
                .wrapping_add(state >> 16);
            state
        };

        for _trial in 0..200 {
            let start_len = (next() % 30) as i32;
            let mut start: Vec<i32> = (1..=start_len).collect();
            // Shuffle start by a simple LCG-based partial Fisher-Yates.
            for i in (1..start_len).rev() {
                let j = (next() as usize % (i as usize + 1)) as i32;
                start.swap(i as usize, j as usize);
            }

            let target_len = (next() % 30) as i32;
            let offset = (next() % 20) as i32;
            let target: Vec<i32> = (1 + offset..=target_len + offset).collect();

            let items: Vec<ProcessItem> = target.iter().map(|p| item(*p)).collect();

            let s = seed(&start);
            refresh(&s, &items);

            assert_eq!(
                pids_of(&s),
                target,
                "start_len={start_len}, target_len={target_len}, offset={offset}"
            );
        }
    }

    /// The anti-flicker contract, stated in GTK's own terms: a move must
    /// reach the model as **one** `items-changed` signal in which the
    /// removed and added counts are equal (so the row manager can pair the
    /// removal with the re-addition and keep the existing row widgets). Two
    /// signals (a separate remove and insert) would destroy and recreate
    /// the widgets — the blank flash.
    #[test]
    fn test_refresh_move_is_one_combined_items_changed_signal() {
        let s = seed(&[1, 2, 3]);
        let (tx, rx) = std::sync::mpsc::channel::<(u32, u32, u32)>();
        s.connect_items_changed(move |_store, pos, removed, added| {
            let _ = tx.send((pos, removed, added));
        });

        // A pure move: pid 3 goes to the front.
        refresh(&s, [item(3), item(1), item(2)].as_slice());
        assert_eq!(pids_of(&s), vec![3, 1, 2]);

        let events: Vec<(u32, u32, u32)> = rx.try_iter().collect();
        assert_eq!(
            events.len(),
            1,
            "a pure move must be a single items-changed signal; got {events:?}"
        );
        let (_, removed, added) = events[0];
        assert!(
            removed >= 1 && added >= 1,
            "the signal must both remove and add within itself"
        );
        assert_eq!(
            removed, added,
            "removed and added must balance so the row widgets can be reused"
        );
    }

    /// A data-only change (same pids, same positions) must produce **no**
    /// `items-changed` signal at all — the data is written into the existing
    /// row objects in place.
    #[test]
    fn test_refresh_data_change_emits_no_items_changed_signal() {
        let s = seed(&[1, 2, 3]);
        let (tx, rx) = std::sync::mpsc::channel::<(u32, u32, u32)>();
        s.connect_items_changed(move |_store, pos, removed, added| {
            let _ = tx.send((pos, removed, added));
        });

        let mut it2 = item(2);
        it2.value.cpu_percent = 99.0;
        refresh(&s, [item(1), it2, item(3)].as_slice());
        assert_eq!(pids_of(&s), vec![1, 2, 3]);

        let events: Vec<(u32, u32, u32)> = rx.try_iter().collect();
        assert!(
            events.is_empty(),
            "a data-only refresh must not mutate the store; got {events:?}"
        );
    }

    /// A real refresh changes the *data* inside every row (CPU%, I/O rates)
    /// even when the order is stable. `refresh` must therefore update the
    /// row's data in place — not just preserve it.
    #[test]
    fn test_refresh_updates_row_data() {
        let s = seed(&[1, 2, 3]);

        // Same ordering, different data.
        let mut it2 = item(2);
        it2.value.cpu_percent = 42.0;
        it2.value.disk_read_speed = Some(123456.0);
        it2.value.disk_write_speed = Some(6543.0);
        let new_items = vec![item(1), it2, item(3)];

        refresh(&s, &new_items);
        assert_eq!(pids_of(&s), vec![1, 2, 3], "order must be preserved");

        let row2 = s
            .item(1)
            .expect("row present")
            .downcast::<ProcessRow>()
            .expect("a ProcessRow");
        assert_eq!(row2.item().value.cpu_percent, 42.0, "row data must refresh");
        assert_eq!(row2.item().value.disk_read_speed, Some(123456.0));
    }

    /// Moving rows around with identical data must **reuse the existing
    /// row objects** (pointer equality) so GTK can recycle their widgets
    /// instead of recreating them — the anti-flicker contract.
    #[test]
    fn test_refresh_pure_move_reuses_row_objects() {
        let pids = [1, 2, 3];
        let s = seed(&pids);
        // Capture each pid's pre-refresh row object (by pointer).
        let orig: HashMap<i32, usize> = pids
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    *p,
                    s.item(i as u32)
                        .expect("row present")
                        .downcast::<ProcessRow>()
                        .expect("a ProcessRow")
                        .as_ptr() as usize,
                )
            })
            .collect();

        // Reverse the order (a pure move) with unchanged data.
        let items: Vec<ProcessItem> = [3, 2, 1].iter().map(|p| item(*p)).collect();
        refresh(&s, &items);
        assert_eq!(pids_of(&s), vec![3, 2, 1]);

        let after: HashMap<i32, usize> = (0..s.n_items())
            .map(|i| {
                let r = s
                    .item(i)
                    .expect("row present")
                    .downcast::<ProcessRow>()
                    .expect("a ProcessRow");
                (r.item().pid, r.as_ptr() as usize)
            })
            .collect();
        for (pid, before) in &orig {
            assert!(
                after[pid] == *before,
                "pid {pid} moved with unchanged data must keep its row object"
            );
        }
    }

    /// A row whose data changed must be updated **in place** — the existing
    /// [`ProcessRow`] object is kept (so GTK recycles its widget) and only
    /// its data is replaced with the new values.
    #[test]
    fn test_refresh_data_change_updates_in_place_keeps_object() {
        let s = seed(&[1, 2, 3]);
        let ptr = |s: &ListStore, i: u32| {
            s.item(i)
                .expect("row present")
                .downcast::<ProcessRow>()
                .expect("a ProcessRow")
                .as_ptr() as usize
        };
        let p2_before = ptr(&s, 1);

        // Same order, same pids, but pid 2's data changed.
        let mut it2 = item(2);
        it2.value.cpu_percent = 99.0;
        let items = vec![item(1), it2, item(3)];
        refresh(&s, &items);
        assert_eq!(pids_of(&s), vec![1, 2, 3]);

        let row2 = s
            .item(1)
            .expect("row present")
            .downcast::<ProcessRow>()
            .expect("a ProcessRow");
        assert_eq!(
            row2.item().value.cpu_percent,
            99.0,
            "changed data must land"
        );
        assert!(
            ptr(&s, 1) == p2_before,
            "a data change keeps the existing row object (updated in place)"
        );
    }

    /// A row whose data changed **and** that moved keeps its object across
    /// the reinsert (widget reuse) while its data is replaced in place.
    #[test]
    fn test_refresh_data_change_and_move_keeps_object() {
        let s = seed(&[1, 2, 3]);
        let ptr = |s: &ListStore, i: u32| {
            s.item(i)
                .expect("row present")
                .downcast::<ProcessRow>()
                .expect("a ProcessRow")
                .as_ptr() as usize
        };
        let p2_before = ptr(&s, 1);
        let p3_before = ptr(&s, 2);

        // Reorder (2,3 swap) + change pid 2's data; drop pid 1.
        let mut it2 = item(2);
        it2.value.cpu_percent = 99.0;
        let items = vec![it2, item(3)];
        refresh(&s, &items);
        assert_eq!(pids_of(&s), vec![2, 3]);

        let row2 = s
            .item(0)
            .expect("row present")
            .downcast::<ProcessRow>()
            .expect("a ProcessRow");
        assert_eq!(
            row2.item().value.cpu_percent,
            99.0,
            "changed data must land"
        );
        assert!(
            ptr(&s, 0) == p2_before,
            "data change + move keeps the existing row object (widget reuse)"
        );
        assert!(
            ptr(&s, 1) == p3_before,
            "a pure move must keep the existing row object (widget reuse)"
        );
    }

    /// Applying `refresh` twice in a row must converge to the final target —
    /// the second refresh operates on the (already fresh) row objects of the
    /// first.
    #[test]
    fn test_refresh_twice_converges() {
        let s = seed(&[5, 3, 8, 1, 4]);
        let a: Vec<ProcessItem> = vec![item(2), item(6), item(8), item(3), item(1)];
        refresh(&s, &a);
        let b: Vec<ProcessItem> = vec![item(7), item(9), item(8), item(3), item(1)];
        refresh(&s, &b);
        assert_eq!(pids_of(&s), vec![7, 9, 8, 3, 1]);
    }

    #[test]
    fn test_refresh_reorder_only() {
        let s = seed(&[1, 2, 3]);
        refresh(&s, [item(3), item(1), item(2)].as_slice());
        assert_eq!(pids_of(&s), vec![3, 1, 2]);
    }

    #[test]
    fn test_refresh_shrink_then_grow() {
        let s = seed(&[1, 2, 3, 4]);
        refresh(&s, [item(4)].as_slice());
        assert_eq!(pids_of(&s), vec![4]);
        refresh(&s, [item(4), item(5), item(6)].as_slice());
        assert_eq!(pids_of(&s), vec![4, 5, 6]);
    }
}
