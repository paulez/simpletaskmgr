use glib::subclass::prelude::*;

use crate::process::ProcessItem;

mod imp {
    use glib::subclass::basic;
    use glib::subclass::prelude::*;
    use std::cell::RefCell;

    use crate::process::ProcessItem;

    #[derive(Default)]
    pub struct ProcessRow {
        pub data: RefCell<ProcessItem>,
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
    pub struct ProcessRow(ObjectSubclass<imp::ProcessRow>);
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

    /// The latest data snapshot for this row, replacing the old one.
    pub fn set_item(&self, item: &ProcessItem) {
        *imp::ProcessRow::from_obj(self).data.borrow_mut() = item.clone();
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
}
