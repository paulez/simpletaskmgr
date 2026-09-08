//! Type-safe parking slot for a [`glib::Binding`] on a [`glib::Object`].
//!
//! `glib::Object` only exposes [`glib::Object::set_data`] /
//! [`glib::Object::steal_data`] for attaching arbitrary Rust values to a
//! glib object, and both are `unsafe`. This module provides the same
//! single-per-object parking slot (park, take, take-and-unbind) with
//! **no** `unsafe` anywhere on the caller side, by keying a thread-local
//! [`std::collections::HashMap`] on the object's
//! [`glib::Object::as_ptr`] cast to `usize` (a *safe* method — the
//! pointer is only used as an integer key, never dereferenced).
//!
//! # Why this exists
//!
//! [`gtk4::SignalListItemFactory`]'s `connect_bind`/`connect_unbind`
//! pair lets us attach a `glib::Binding` from a source object to a
//! `Label` that will be recycled. GTK does **not** own those bindings.
//! Dropping the Rust [`glib::Binding`] handle does not disconnect the
//! C-side binding (established in `doc/GTK_REFRESH_BUG.md`), so if the
//! factory simply lets the handle drop, each recycled cell accrues one
//! live binding per previous row it was bound to — the "persistent
//! wrong-name" symptom (a row's cells disagree and never self-heal).
//!
//! The correct contract is:
//!
//! - `connect_bind`: retire any parking slot on this object (disconnect
//!   the binding), then park the new binding.
//! - `connect_unbind`: retire the parking slot (disconnect the binding).
//!
//! A *defensive* `warn!` fires in `connect_bind` if a binding is found
//! parked at all — that is the signal that the *previous*
//! `connect_unbind` did not run for this recycled cell, which is what
//! produced a desync.
//!
//! All three accessor functions take a `usize` key. To obtain the key
//! from any `glib::Object` (e.g. `gtk4::Label`, `ProcessRow`), the
//! caller runs `obj.as_ptr() as usize` — both steps are *safe*.
//!
//! # Leak profile
//!
//! If an object is destroyed before `take_and_unbind` is called, the
//! entry leaks one [`glib::Binding`] in the thread-local map. The
//! binding itself is cheap, and importantly the source object is no
//! longer targeting a live label. The slab is only ever used for the
//! finite handful of recycled cells in a single `ColumnView`, so the
//! bound on the leak is small.

use std::cell::RefCell;
use std::collections::HashMap;

// The thread-local parking slab: one `glib::Binding` per object
// (identified by its pointer as a `usize` key).
// (A `thread_local!` binding, not a doc-comment-able item.)
thread_local! {
    static PARKED: RefCell<HashMap<usize, glib::Binding>> = RefCell::new(HashMap::new());
}

/// Stores `binding` as the one parked at `key`, replacing any earlier
/// entry in the registry. The caller is assumed to have retired any
/// earlier parking first (`take_and_unbind` does both) — passing a
/// non-`None` `binding` on an already-parked key without first retiring
/// the old one will leak the old binding (and the factory's own
/// tripwire `warn!` will fire so a repro log can confirm this).
pub fn park(key: usize, binding: glib::Binding) {
    PARKED.with(|p| {
        p.borrow_mut().insert(key, binding);
    })
}

/// Returns the parked binding at `key`, **without** disconnecting it
/// (useful in tests that want to inspect the raw `glib::Binding`).
pub fn take(key: usize) -> Option<glib::Binding> {
    PARKED.with(|p| p.borrow_mut().remove(&key))
}

/// Returns any parked binding at `key` and disconnects it (the
/// factory's `connect_unbind` and `connect_bind` tripwire both use this).
/// If no binding was parked, returns [`Option::None`].
pub fn take_and_unbind(key: usize) -> Option<glib::Binding> {
    let b = take(key);
    if let Some(b) = &b {
        b.unbind();
    }
    b
}

/// Clears all parking in the thread-local registry.
/// **Only used by tests** that need a fresh registry after exercising
/// the factory — the registry is per-thread state, shared across every
/// object the same thread created/used.
#[doc(hidden)]
pub fn reset_for_tests() {
    PARKED.with(|p| {
        p.borrow_mut().clear();
    })
}

/// A minimal writable `glib::Object` for tests — a stand-in for a
/// `gtk4::Label` as the *target* of a property binding, with one
/// writable string property (`name`). Used to exercise the parking
/// slab in unit tests without requiring GTK initialization.
#[cfg(test)]
mod imp {
    use glib::prelude::*;
    use glib::subclass::basic;
    use glib::subclass::prelude::*;
    use std::cell::Cell;

    pub struct TestTarget {
        pub name: Cell<Option<String>>,
    }

    impl Default for TestTarget {
        fn default() -> Self {
            Self {
                name: Cell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TestTarget {
        const NAME: &'static str = "SimpleTaskMgrTestTarget";
        type Type = super::TestTarget;
        type ParentType = glib::Object;
        type Instance = basic::InstanceStruct<Self>;
        type Class = basic::ClassStruct<Self>;
    }

    fn property_specs() -> &'static [glib::ParamSpec] {
        static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
        PROPS.get_or_init(|| {
            vec![glib::ParamSpecString::builder("name")
                .nick("Name")
                .blurb("writable name, for tests")
                .build()]
        })
    }

    impl ObjectImpl for TestTarget {
        fn properties() -> &'static [glib::ParamSpec] {
            property_specs()
        }

        fn set_property(&self, _id: usize, value: &glib::Value, _pspec: &glib::ParamSpec) {
            if let Ok(name) = value.get::<String>() {
                self.name.set(Some(name));
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            if pspec.name() == "name" {
                return glib::Value::from(self.name.take().unwrap_or_default());
            }
            unreachable!("unhandled property {}", pspec.name())
        }
    }
}

#[cfg(test)]
glib::wrapper! {
    pub struct TestTarget(ObjectSubclass<imp::TestTarget>);
}

#[cfg(test)]
#[serial_test::serial]
mod tests {
    use super::*;
    use crate::process::ProcessItem;
    use glib::prelude::*;

    fn item(pid: i32) -> ProcessItem {
        crate::testutil::test_item(pid)
    }

    /// The parking key (an `as_ptr` pointer cast to `usize`) is unique
    /// per `glib::Object` and stable across calls for the same object.
    #[test]
    fn test_key_unique_and_stable() {
        let a = glib::Object::new::<TestTarget>();
        let b = glib::Object::new::<TestTarget>();
        assert_ne!(a.as_ptr() as usize, b.as_ptr() as usize);
        assert_eq!(a.as_ptr() as usize, a.as_ptr() as usize);
        assert_eq!(b.as_ptr() as usize, b.as_ptr() as usize);
    }

    /// The parking slot round-trips a `glib::Binding`: a fresh object has
    /// no parking; parking one makes it the sole visible entry; a second
    /// `take` sees `None`.
    #[test]
    fn test_parking_slot_roundtrip() {
        reset_for_tests();
        let target = glib::Object::new::<TestTarget>();
        let key = target.as_ptr() as usize;

        assert!(take(key).is_none());

        let row = crate::process_row::ProcessRow::from_item(&item(1));
        let binding = row
            .bind_property("name", &target, "name")
            .sync_create()
            .build();
        park(key, binding);

        assert!(take(key).is_some());
        assert!(take(key).is_none());
    }

    /// `take_and_unbind` returns the parked binding **and** disconnects it
    /// from its source object — the slab's `Binding` is what makes the
    /// C-side binding actually go away, not just the Rust handle.
    #[test]
    fn test_take_and_unbind_disconnects() {
        reset_for_tests();
        let target = glib::Object::new::<TestTarget>();
        let key = target.as_ptr() as usize;
        let src = crate::process_row::ProcessRow::from_item(&item(1));

        let binding = src
            .bind_property("name", &target, "name")
            .sync_create()
            .build();
        park(key, binding);

        let _taken = take_and_unbind(key).expect("a parking was set");
        assert!(take(key).is_none());
    }

    /// The exact desync scenario from the GTK refresh investigation:
    /// a recycled "target" whose *previous* binding is not disconnected
    /// would let a stale `ProcessRow`'s `notify` still write into it.
    /// Retiring the previous binding on every bind/unbind cycle must fully
    /// seal off the stale source.
    ///
    /// Uses `TestTarget` (a writable `glib::Object`) as the target —
    /// the parking slab and the `glib::Binding` both only need
    /// `glib::Object` instances, so no `gtk4::Label` or GTK
    /// initialization is required.
    #[test]
    fn test_recycled_target_stale_source_sealed() {
        reset_for_tests();
        let target = glib::Object::new::<TestTarget>();
        let key = target.as_ptr() as usize;
        let row_a = crate::process_row::ProcessRow::from_item(&item(1));
        let row_b = crate::process_row::ProcessRow::from_item(&item(2));

        // Phase 1: factory binds the target to row A and parks it.
        let binding_a = row_a
            .bind_property("name", &target, "name")
            .sync_create()
            .build();
        park(key, binding_a);

        // The `TestTarget.name` should reflect row A's initial name
        // (via `sync_create`).
        assert_eq!(target.property::<String>("name"), "name1");

        // Phase 2: factory's `connect_unbind` retires the binding.
        let _retired = take_and_unbind(key).expect("parking from phase 1");

        // Phase 3: factory rebinds the recycled target to row B and parks.
        let binding_b = row_b
            .bind_property("name", &target, "name")
            .sync_create()
            .build();
        park(key, binding_b);

        // Phase 4: stale row A changes (a task-manager refresh that
        // re-notifies row A). Binding A was retired in Phase 2, so row B's
        // `notify` remains the only writer into the target's `name`
        // property.
        let mut item_a = item(1);
        item_a.value.name = "STALE".into();
        row_a.set_item(&item_a);

        // The target's `name` should be row B's value (from the *second*
        // binding), not row A's value (from the retired first binding).
        assert_eq!(
            target.property::<String>("name"),
            "name2",
            "a stale source row's notify must not overwrite a recycled \
             target whose previous binding was retired"
        );
    }
}
