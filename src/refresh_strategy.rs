//! Strategies for republishing the process list into the `gio::ListStore`.
//!
//! Two strategies are implemented so they can be run side-by-side and
//! benchmarked against each other with `examples/liststore_bench.rs`.
//!
//! * [`Strategy::InPlace`] (default) — diff the old and new pid sequence and
//!   mutate only the positions whose row changed, reusing the existing
//!   [`ProcessRow`] `glib::Object` wherever the pid is unchanged. Because the
//!   store is never emptied in one step, the `GtkAdjustment` is never reset,
//!   so the scroll position is preserved automatically and the list does not
//!   flash empty. When the order is stable (e.g. sorted by PID) the number of
//!   store mutations is near zero; when the order is unstable (e.g. sorted by
//!   CPU% each refresh) it approaches the cost of `RebuildAll`.
//!
//! * [`Strategy::RebuildAll`] — the historical behavior: remove every row and
//!   re-append the new ones. Simplest and constant-cost, but it destroys and
//!   rebuilds every `gtk4::ListView` row in one step (selection must be
//!   re-resolved by pid by the caller). The scroll position of the surrounding
//!   `ScrolledWindow` is clamped to 0 while the store is briefly empty, so the
//!   caller is expected to save and restore the `adjustment.value` around this
//!   call; the still-visible list can flash empty between the empty phase and
//!   the repaint.

use std::collections::{HashMap, HashSet};

use crate::process::ProcessItem;
use crate::process_row::ProcessRow;
use gtk4::gio::prelude::*;
use gtk4::gio::ListStore;

/// A strategy for refreshing the process list `ListStore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strategy {
    /// Replace every row in the store (historical behavior).
    RebuildAll,
    /// Reorder / mutate only the rows whose pid changed position or
    /// appeared / disappeared, reusing existing [`ProcessRow`] objects.
    #[default]
    InPlace,
}

impl Strategy {
    /// Read the chosen strategy from the `STM_REFRESH_STRATEGY` env var.
    ///
    /// `rebuild` selects [`RebuildAll`]; `inplace`, `in-place`, or `in_place`
    /// (or an unset / unrecognized value) selects [`InPlace`].
    pub fn from_env_var() -> Self {
        match std::env::var_os("STM_REFRESH_STRATEGY").as_deref() {
            Some(v) if v == "rebuild" || v == "rebuild-all" || v == "rebuild_all" => {
                Self::RebuildAll
            }
            _ => Self::InPlace,
        }
    }

    /// Apply `items` to `store`, replacing its contents.
    ///
    /// Selection is intentionally **not** handled here — the caller keeps
    /// its existing "restore the previous selection by pid" bookkeeping
    /// around this call so both paths are compared on equal footing.
    pub fn apply(&self, store: &ListStore, items: &[ProcessItem]) {
        match self {
            Strategy::RebuildAll => rebuild_all(store, items),
            Strategy::InPlace => in_place(store, items),
        }
    }
}

/// Historical behavior: tear down every row and re-append the new ones.
fn rebuild_all(store: &ListStore, items: &[ProcessItem]) {
    store.remove_all();
    for item in items {
        store.append(&ProcessRow::from_item(item));
    }
}

/// Diff-based update.
///
/// In a left-to-right walk over the target, once position `t` is fixed no
/// later fix can touch indices < t (moves are `remove(src) + insert(t)` with
/// `src > t`, and size changes only append), so the already-fixed prefix
/// never changes again. Pids are unique, therefore at step `t` the row that
/// should sit at `t` can only be at index `t` or somewhere after it.
///
/// Rows whose pid **and data** are unchanged at their current position are
/// left completely untouched — zero store mutations, zero widget churn for
/// them. Every other target row is satisfied by a fresh [`ProcessRow`]:
/// `gtk4::ListItem` deliberately skips its `bind` signal when it is rebound
/// to the *same* model object, so a row whose data changed must be supplied
/// as a new object for the `ListView` to re-render its labels.
///
/// `order` mirrors the store contents and is mutated in lock-step with the
/// store, so row lookups are plain `Vec` scans, not FFI round-trips.
fn in_place(store: &ListStore, items: &[ProcessItem]) {
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
            // A kept row, at or after position `t`, whose data must refresh:
            // drop it and put a fresh object in its slot. gtk4 does not
            // rebind an item that still points at the *same* model object, so
            // the object itself has to change.
            store.remove(src as u32);
            let fresh = ProcessRow::from_item(item);
            store.insert(t as u32, &fresh);
            order.remove(src);
            order.insert(t, pid);
            rows.insert(pid, fresh);
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
    use crate::process::TaskMgrProcess;

    fn item(p: i32) -> ProcessItem {
        ProcessItem::new(&TaskMgrProcess::new(
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

    /// A property-style invariant: for a handful of input / target sizes, both
    /// strategies must land the store in the same end-state, and that end-state
    /// must equal the target. This is the single correctness check that the
    /// in-place algorithm relies on.
    #[test]
    fn test_strategies_match_on_small_cases() {
        for start_len in [0, 1, 3, 5, 10] {
            for target_len in [0, 1, 3, 5, 10] {
                let start: Vec<i32> = (1..=start_len).collect();
                // A fixed target that overlaps the start by construction.
                let target: Vec<i32> = (start_len - target_len + 2..=start_len + 2).collect();
                let items: Vec<ProcessItem> = target.iter().map(|p| item(*p)).collect();

                let s_a = seed(&start);
                Strategy::RebuildAll.apply(&s_a, &items);
                let s_b = seed(&start);
                Strategy::InPlace.apply(&s_b, &items);

                assert_eq!(
                    pids_of(&s_a),
                    pids_of(&s_b),
                    "start={start:?}, target={target:?}"
                );
                assert_eq!(pids_of(&s_a), target.to_vec());
            }
        }
    }

    /// A randomised equivalence test: N random start lists, N random target
    /// lists, both strategies must produce an identical, correct store.
    #[test]
    fn test_strategies_match_on_random_inputs() {
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
            let expected: Vec<i32> = target.to_vec();

            let s_a = seed(&start);
            Strategy::RebuildAll.apply(&s_a, &items);
            let s_b = seed(&start);
            Strategy::InPlace.apply(&s_b, &items);

            assert_eq!(
                pids_of(&s_a),
                pids_of(&s_b),
                "start_len={start_len}, target_len={target_len}, offset={offset}"
            );
            assert_eq!(pids_of(&s_a), expected);
        }
    }

    /// Sanity: the two strategies must be interchangeable — applying one and
    /// then the other to the same store must converge to the same end-state.
    /// A real refresh changes the *data* inside every row (CPU%, I/O rates)
    /// even when the order is stable. InPlace must therefore update the
    /// row's data in place — not just preserve it.
    #[test]
    fn test_in_place_refreshes_row_data() {
        let s = seed(&[1, 2, 3]);

        // Same ordering, different data.
        let mut it2 = item(2);
        it2.value.cpu_percent = 42.0;
        it2.value.disk_read_speed = Some(123456.0);
        it2.value.disk_write_speed = Some(6543.0);
        let new_items = vec![item(1), it2, item(3)];

        Strategy::InPlace.apply(&s, &new_items);
        assert_eq!(pids_of(&s), vec![1, 2, 3], "order must be preserved");

        let row2 = s
            .item(1)
            .expect("row present")
            .downcast::<ProcessRow>()
            .expect("a ProcessRow");
        assert_eq!(row2.item().value.cpu_percent, 42.0, "row data must refresh");
        assert_eq!(row2.item().value.disk_read_speed, Some(123456.0));
    }

    #[test]
    fn test_strategies_compose() {
        let s = seed(&[5, 3, 8, 1, 4]);
        let a: Vec<ProcessItem> = vec![item(2), item(6), item(8), item(3), item(1)];
        Strategy::RebuildAll.apply(&s, &a);
        let b: Vec<ProcessItem> = vec![item(7), item(9), item(8), item(3), item(1)];
        Strategy::InPlace.apply(&s, &b);
        assert_eq!(pids_of(&s), vec![7, 9, 8, 3, 1]);
    }

    #[test]
    fn test_rebuild_all_replaces() {
        let s = seed(&[1, 2, 3]);
        Strategy::RebuildAll.apply(&s, [item(4), item(5)].as_slice());
        assert_eq!(pids_of(&s), vec![4, 5]);
    }

    #[test]
    fn test_in_place_reorder_only() {
        let s = seed(&[1, 2, 3]);
        Strategy::InPlace.apply(&s, [item(3), item(1), item(2)].as_slice());
        assert_eq!(pids_of(&s), vec![3, 1, 2]);
    }

    #[test]
    fn test_in_place_shrink_then_grow() {
        let s = seed(&[1, 2, 3, 4]);
        Strategy::InPlace.apply(&s, [item(4)].as_slice());
        assert_eq!(pids_of(&s), vec![4]);
        Strategy::InPlace.apply(&s, [item(4), item(5), item(6)].as_slice());
        assert_eq!(pids_of(&s), vec![4, 5, 6]);
    }
}
