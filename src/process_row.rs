use glib::subclass::prelude::*;
use gtk4::{prelude::*, Box, Label};

use crate::process::ProcessItem;

mod imp {
    use glib::subclass::basic;
    use glib::subclass::prelude::*;
    use std::cell::RefCell;

    use crate::process::ProcessItem;

    #[derive(Default)]
    pub struct ProcessRow {
        pub data: RefCell<ProcessItem>,
        pub labels: RefCell<Option<Vec<gtk4::Label>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessRow {
        const NAME: &'static str = "SimpleTaskMgrProcessRow";
        type Type = super::ProcessRow;
        type ParentType = glib::Object;
        type Interfaces = ();
        type Instance = basic::InstanceStruct<Self>;
        type Class = basic::ClassStruct<Self>;
    }

    impl ObjectImpl for ProcessRow {}
}

glib::wrapper! {
    /// A `glib::Object` subclass wrapping one `ProcessItem` row of data.
    ///
    /// Used as the item type of the `gio::ListStore` backing the process
    /// `ListView`, so each row can be selected and read back via
    /// `SingleSelection::selected_item`.
    ///
    /// A row object is created **once per process** and then mutated in place
    /// for its whole life: [`ProcessRow::set_item`] updates the data and
    /// repaints the row's live cell labels (when on screen) instead of
    /// replacing the object, so a `gtk4::ListView` refresh that only moves or
    /// re-numbers a process recycles the existing row widget rather than
    /// rebuilding it (which is what causes the blank-flash flicker).
    pub struct ProcessRow(ObjectSubclass<imp::ProcessRow>);
}

/// The cell labels of one row, in UI order (`pid, username, name, cpu,
/// read, write`), as text for a `ProcessItem`.
fn displayed(item: &ProcessItem) -> [String; 6] {
    let p = &item.value;
    [
        p.pid.to_string(),
        p.username.clone(),
        p.name.clone(),
        p.cpu_percent_str(),
        p.disk_read_str(),
        p.disk_write_str(),
    ]
}

impl ProcessRow {
    /// Creates a new row object from a `ProcessItem` snapshot.
    pub fn from_item(item: &ProcessItem) -> Self {
        let r: Self = glib::Object::new::<ProcessRow>();
        *imp::ProcessRow::from_obj(&r).data.borrow_mut() = item.clone();
        r
    }

    /// The current data snapshot held by this row.
    pub fn item(&self) -> ProcessItem {
        imp::ProcessRow::from_obj(self).data.borrow().clone()
    }

    /// Whether the row's current data is exactly equal to `item` (a no-clone
    /// comparison, for refresh bookkeeping).
    pub fn has_value(&self, item: &crate::process::ProcessItem) -> bool {
        *imp::ProcessRow::from_obj(self).data.borrow() == *item
    }

    /// Replaces this row's data with a fresh copy. If the data actually
    /// changed and the row's cell labels are currently on screen, they are
    /// re-texted in place — no object replacement, no row-widget rebuild.
    pub fn set_item(&self, item: &ProcessItem) {
        let imp = imp::ProcessRow::from_obj(self);
        if *imp.data.borrow() == *item {
            return;
        }
        *imp.data.borrow_mut() = item.clone();
        if let Some(labels) = imp.labels.borrow().as_ref() {
            for (label, text) in labels.iter().zip(displayed(item)) {
                label.set_label(&text);
            }
        }
    }

    /// Records the row's on-screen cell labels (in the UI order
    /// `pid, username, name, cpu, read, write`) and (re)applies the current
    /// data to them. Called by the factory whenever this row's widget is
    /// (re)bound, and again if the row is moved while data has also changed.
    pub fn bind_labels(&self, box_: Box) {
        let imp = imp::ProcessRow::from_obj(self);
        let mut labels = Vec::new();
        let mut cur = box_.first_child();
        for _ in 0..6 {
            let next = cur.as_ref().and_then(|w| w.next_sibling());
            if let Some(widget) = cur {
                if let Ok(label) = widget.downcast::<Label>() {
                    labels.push(label);
                }
            }
            cur = next;
        }
        let item = imp.data.borrow().clone();
        for (label, text) in labels.iter().zip(displayed(&item)) {
            label.set_label(&text);
        }
        *imp.labels.borrow_mut() = Some(labels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::TaskMgrProcess;

    fn item(pid: i32) -> ProcessItem {
        ProcessItem::new(&TaskMgrProcess::new(
            format!("name{pid}"),
            pid,
            1000,
            "paul".to_string(),
            1.0,
        ))
    }

    #[test]
    fn test_process_row_roundtrip() {
        let i = item(123);
        let r = ProcessRow::from_item(&i);
        let back = r.item();
        assert_eq!(back.pid, 123);
        assert_eq!(back, i);
    }

    #[test]
    fn test_process_row_set_item_updates_in_place() {
        let r = ProcessRow::from_item(&item(9));
        assert_eq!(r.item().value.cpu_percent, 1.0);
        let mut i = item(9);
        i.value.cpu_percent = 42.5;
        r.set_item(&i);
        assert_eq!(r.item().value.cpu_percent, 42.5);
        // Setting an equal item is a no-op that does not change anything.
        r.set_item(&i);
        assert_eq!(r.item(), i);
    }
}
