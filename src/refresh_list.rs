//! Refreshes the process list `gio::ListStore` in place.
//!
//! Instead of tearing down and rebuilding the whole store (which empties it
//! momentarily, clamps the surrounding scroll adjustment, and makes the
//! `gtk4::ListView` flash empty), [`refresh`] diffs the old and new pid
//! sequence and mutates only the rows whose pid, position, or data changed.
//!
//! When a row's position changes but its data is identical, [`refresh`]
//! reinserts the **same** [`ProcessRow`] `glib::Object` into the new slot.
//! GTK's `GtkListItemManager` pools widgets by model-item pointer, so it
//! recycles the existing `GtkListItem` widget instead of recreating one —
//! no blank flash, no label re-layout.
//!
//! When data has changed, a fresh [`ProcessRow`] is required because
//! `GtkListItem::bind_to_model` early-returns when re-bound to the same
//! model object; a new pointer forces the `bind` signal to fire and the
//! labels to update.

use std::collections::{HashMap, HashSet};

use crate::process::ProcessItem;
use crate::process_row::ProcessRow;
use gtk4::gio::prelude::*;
use gtk4::gio::ListStore;

/// Replace the contents of `store` with `items`, mutating it in place so the
/// `gtk4::ListView` rendering it does not lose its rows (or its scroll
/// position) one tick at a time.
///
/// Selection is intentionally **not** handled here — the caller decides what
/// to do with it around this call.
///
/// In a left-to-right walk over the target, once position `t` is fixed no
/// later fix can touch indices < t (moves are `remove(src) + insert(t)` with
/// `src > t`, and size changes only append), so the already-fixed prefix
/// never changes again. Pids are unique, therefore at step `t` the row that
/// should sit at `t` can only be at index `t` or somewhere after it.
///
/// Rows whose pid **and data** are unchanged at their current position are
/// left completely untouched — zero store mutations, zero widget churn for
/// them. A row that only moved (data unchanged) keeps its existing
/// [`ProcessRow`] object across the reinsert so GTK recycles its widget.
/// A row whose data changed is satisfied by a fresh [`ProcessRow`]:
/// `gtk4::ListItem` deliberately skips its `bind` signal when it is rebound
/// to the *same* model object, so a data change must come with a new
/// object for the `ListView` to re-render its labels.
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

    // Pass 1: drop rows whose pid is not in the target set, in descending
    // index order so the earlier indices stay valid while we mutate.
    let mut to_remove: Vec<u32> = Vec::new();
    for (i, pid) in order.iter().enumerate() {
        if !target_pids.contains(pid) {
            to_remove.push(i as u32);
        }
    }
    to_remove.sort_unstable_by_key(|i| std::cmp::Reverse(*i));
    for idx in to_remove {
        store.remove(idx);
    }
    order.retain(|p| target_pids.contains(p));
    rows.retain(|p, _| target_pids.contains(p));

    // Pass 2: walk the target left-to-right (invariants above).
    for (t, item) in items.iter().enumerate() {
        let pid = item.pid;

        if t < order.len() && order[t] == pid && rows[&pid].has_value(&item.value) {
            // Same pid, same position, same data: nothing to do.
            continue;
        }

        if let Some(src) = order
            .iter()
            .skip(t)
            .position(|p| *p == pid)
            .map(|rel| t + rel)
        {
            let same_object = rows[&pid].has_value(&item.value);
            let row = if same_object {
                // A pure move (data identical): reuse the *same* object.
                // GTK's row manager pools widgets by model-item pointer, so
                // it recycles the existing row widget here instead of
                // creating a new one — no blank flash and no rebind.
                rows.remove(&pid).expect("row for a kept pid")
            } else {
                // Data changed: gtk4 does not rebind an item that still
                // points at the *same* model object, so the object itself
                // has to change to force the `bind` signal to update.
                ProcessRow::from_item(item)
            };
            store.remove(src as u32);
            store.insert(t as u32, &row);
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
        ProcessItem::new(&crate::process::TaskMgrProcess::new(
            format!("name{p}"),
            p,
            1000,
            "paul".to_string(),
            1.0,
        ))
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

    /// A row whose data changed must be a **fresh object** (so the `bind`
    /// signal re-fires), while a row that only moved with unchanged data
    /// keeps its object.
    #[test]
    fn test_refresh_data_change_gets_new_row_object_move_keeps_it() {
        let s = seed(&[1, 2, 3]);
        let before = |s: &ListStore, i: u32| {
            s.item(i)
                .expect("row present")
                .downcast::<ProcessRow>()
                .expect("a ProcessRow")
                .as_ptr() as usize
        };
        let p2_before = before(&s, 1);
        let p3_before = before(&s, 2);

        // Reorder (2,3 swap) + change pid 2's data; drop pid 1.
        let mut it2 = item(2);
        it2.value.cpu_percent = 99.0;
        let items = vec![it2, item(3)];
        refresh(&s, &items);
        assert_eq!(pids_of(&s), vec![2, 3]);

        let p2_after = before(&s, 0);
        let p3_after = before(&s, 1);
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
            p2_after != p2_before,
            "a data change must come with a fresh row object (rebind)"
        );
        assert!(
            p3_after == p3_before,
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
