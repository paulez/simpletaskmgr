use glib::prelude::*;
use glib::subclass::prelude::*;

use crate::process::ProcessItem;

mod imp {
    use glib::subclass::basic;
    use glib::subclass::prelude::*;
    use std::cell::RefCell;

    use crate::process::ProcessItem;
    use glib::prelude::*;

    pub struct ProcessRow {
        pub data: RefCell<ProcessItem>,
    }

    impl Default for ProcessRow {
        fn default() -> Self {
            Self {
                data: RefCell::new(Default::default()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessRow {
        const NAME: &'static str = "SimpleTaskMgrProcessRow";
        type Type = super::ProcessRow;
        type ParentType = glib::Object;
        type Instance = basic::InstanceStruct<Self>;
        type Class = basic::ClassStruct<Self>;
    }

    fn property_specs() -> &'static [glib::ParamSpec] {
        static PROPS: std::sync::OnceLock<Vec<glib::ParamSpec>> = std::sync::OnceLock::new();
        PROPS.get_or_init(|| {
            vec![
                glib::ParamSpecString::builder("pid")
                    .nick("PID")
                    .blurb("The PID, as text")
                    .build(),
                glib::ParamSpecString::builder("username")
                    .nick("Username")
                    .blurb("The owning user, as text")
                    .build(),
                glib::ParamSpecString::builder("name")
                    .nick("Name")
                    .blurb("The process name, as text")
                    .build(),
                glib::ParamSpecString::builder("cpu")
                    .nick("CPU")
                    .blurb("The CPU usage, as text")
                    .build(),
                glib::ParamSpecString::builder("mem")
                    .nick("Memory")
                    .blurb("The memory usage, as text")
                    .build(),
                glib::ParamSpecString::builder("disk-read")
                    .nick("Disk read")
                    .blurb("The disk read rate, as text")
                    .build(),
                glib::ParamSpecString::builder("disk-write")
                    .nick("Disk write")
                    .blurb("The disk write rate, as text")
                    .build(),
            ]
        })
    }

    impl ObjectImpl for ProcessRow {
        fn properties() -> &'static [glib::ParamSpec] {
            property_specs()
        }

        fn set_property(&self, _id: usize, _value: &glib::Value, _pspec: &glib::ParamSpec) {
            // Read-only: the data only moves via
            // [`super::ProcessRow::set_item`], which also re-notifies.
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let item = self.data.borrow();
            match pspec.name() {
                "pid" => glib::Value::from(item.pid.to_string()),
                "username" => glib::Value::from(item.value.username.clone()),
                "name" => glib::Value::from(item.value.name.clone()),
                "cpu" => glib::Value::from(item.value.cpu_percent_str()),
                "mem" => glib::Value::from(item.value.mem_percent_str()),
                "disk-read" => glib::Value::from(item.value.disk_read_str()),
                "disk-write" => glib::Value::from(item.value.disk_write_str()),
                _ => unreachable!("unhandled property {}", pspec.name()),
            }
        }
    }
}

glib::wrapper! {
    /// A `glib::Object` subclass wrapping one `ProcessItem` row of data.
    ///
    /// Used as the item type of the `gio::ListStore` backing the process
    /// `ColumnView`. Every displayed cell in that view is a plain `Label`
    /// whose `label` property is property-bound to one of the row's read-only
    /// string properties (`pid`, `username`, `name`, `cpu`, `disk_read`,
    /// `disk_write`), so re-texting a cell is a matter of updating the data
    /// and calling `g_object_notify`.
    ///
    /// A row object is created **once per process** and then mutated in place
    /// for its whole life: [`ProcessRow::set_item`] updates the data and
    /// re-emits the properties that changed instead of replacing the object,
    /// so a `gtk4::ColumnView` refresh that only moves or re-numbers a
    /// process recycles the existing row widget rather than rebuilding it
    /// (which is what causes the blank-flash flicker).
    pub struct ProcessRow(ObjectSubclass<imp::ProcessRow>);
}

impl ProcessRow {
    fn imp(&self) -> &imp::ProcessRow {
        imp::ProcessRow::from_obj(self)
    }

    /// Creates a new row object from a `ProcessItem` snapshot.
    pub fn from_item(item: &ProcessItem) -> Self {
        let r: Self = glib::Object::new::<ProcessRow>();
        *r.imp().data.borrow_mut() = item.clone();
        r
    }

    /// The current data snapshot held by this row.
    pub fn item(&self) -> ProcessItem {
        self.imp().data.borrow().clone()
    }

    /// The row's stable `pid` without cloning the whole value — the refresh
    /// and selection-match paths only need the identity, so this avoids the
    /// `ProcessItem` clone (two `String` copies) that [`Self::item`] incurs.
    pub fn pid(&self) -> i32 {
        self.imp().data.borrow().pid
    }

    /// Whether the row's current data is exactly equal to `item` (a no-clone
    /// comparison, for refresh bookkeeping).
    pub fn has_value(&self, item: &crate::process::ProcessItem) -> bool {
        *self.imp().data.borrow() == *item
    }

    /// Replaces this row's data with a fresh copy. For each display property
    /// whose text changed, a `notify` signal is emitted so any widget bound
    /// to it re-renders — no object replacement, no store mutation, no
    /// row-widget rebuild.
    pub fn set_item(&self, item: &ProcessItem) {
        let imp = self.imp();
        if *imp.data.borrow() == *item {
            return;
        }
        // Compute the per-field `changed` flags *under an immutable borrow*,
        // so the diff never clones the old `ProcessItem` (two `String`
        // copies). The flags are `Bool` (`Copy`), so they outlive the borrow
        // safely; only after the borrow is dropped do we overwrite the cell.
        let o = imp.data.borrow();
        let p = &item.value;
        let (
            pid_changed,
            username_changed,
            name_changed,
            cpu_changed,
            mem_changed,
            read_changed,
            write_changed,
        ) = {
            let ov = &o.value;
            (
                ov.pid != p.pid,
                ov.username != p.username,
                ov.name != p.name,
                ov.cpu_percent.total_cmp(&p.cpu_percent) != std::cmp::Ordering::Equal,
                // `Option<f64>` needs an explicit `None` arm — two `None`s
                // must not fire a notify — and `total_cmp` keeps the `Some`
                // comparison NaN-safe like cpu.
                match (ov.mem_percent, p.mem_percent) {
                    (None, None) => false,
                    (Some(a), Some(b)) => a.total_cmp(&b) != std::cmp::Ordering::Equal,
                    (Some(_), None) | (None, Some(_)) => true,
                },
                ov.disk_read_speed != p.disk_read_speed,
                ov.disk_write_speed != p.disk_write_speed,
            )
        };
        drop(o);
        *imp.data.borrow_mut() = item.clone();
        if pid_changed {
            self.notify("pid");
        }
        if username_changed {
            self.notify("username");
        }
        if name_changed {
            self.notify("name");
        }
        if cpu_changed {
            self.notify("cpu");
        }
        if mem_changed {
            self.notify("mem");
        }
        if read_changed {
            self.notify("disk-read");
        }
        if write_changed {
            self.notify("disk-write");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pid: i32) -> ProcessItem {
        crate::testutil::test_item(pid)
    }

    #[test]
    fn test_process_row_roundtrip() {
        let i = item(123);
        let r = ProcessRow::from_item(&i);
        let back = r.item();
        assert_eq!(back.pid, 123);
        assert_eq!(back, i);
    }

    /// `pid()` gives the row's stable identity directly, matching the
    /// value clone, so callers that only need the pid avoid the `item()` clone.
    #[test]
    fn test_process_row_pid_accessor_matches_item() {
        let r = ProcessRow::from_item(&item(4242));
        assert_eq!(r.pid(), 4242);
        assert_eq!(r.pid(), r.item().pid);
        // After a data change the accessor still reports the (unchanged) pid.
        let mut i = item(4242);
        i.value.cpu_percent = 99.0;
        r.set_item(&i);
        assert_eq!(r.pid(), 4242);
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

    /// The read-only string properties mirror the row data the way the
    /// on-screen cells display it.
    #[test]
    fn test_properties_mirror_row_data() {
        let mut i = item(7);
        i.value.disk_read_speed = Some(120.0 * 1024.0);
        let r = ProcessRow::from_item(&i);
        assert_eq!(r.property::<String>("pid"), "7");
        assert_eq!(r.property::<String>("username"), "paul".to_string());
        assert_eq!(r.property::<String>("name"), "name7".to_string());
        assert_eq!(r.property::<String>("cpu"), "1.0%".to_string());
        // `mem_percent` is `None` in the fixture, so the cell is blank.
        assert_eq!(r.property::<String>("mem"), String::new());
        assert_eq!(r.property::<String>("disk-read"), "120.0 KiB/s".to_string());
        assert_eq!(r.property::<String>("disk-write"), String::new());
    }

    /// `set_item` notifies the properties whose text changed — and only
    /// those — so a cell bound to `cpu` repaints without unrelated churn.
    #[test]
    fn test_set_item_notifies_only_changed_properties() {
        let r = ProcessRow::from_item(&item(4));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        r.connect_notify_local(Some("cpu"), move |row, _| {
            let _ = tx.send(row.property::<String>("cpu"));
        });

        let mut changed = item(4);
        changed.value.cpu_percent = 99.5;
        r.set_item(&changed);
        let fired: Vec<String> = rx.try_iter().collect();
        assert_eq!(fired, vec!["99.5%".to_string()]);

        // A change to an unrelated field must not re-fire `cpu`.
        let mut other = changed.clone();
        other.value.username = "root".to_string();
        r.set_item(&other);
        let idle: Vec<String> = rx.try_iter().collect();
        assert!(idle.is_empty(), "an unrelated change must not notify cpu");
    }

    /// `mem` fires only when the displayed MEM% actually changes: a known
    /// value updates the cell, an unrelated change stays quiet, and two
    /// `None`s (or the same `Some(x)`) never fire — mirroring the CPU%
    /// behavior for a known-value cell.
    #[test]
    fn test_set_item_notifies_mem_only_when_displayed_value_changes() {
        let r = ProcessRow::from_item(&item(5));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        r.connect_notify_local(Some("mem"), move |row, _| {
            let _ = tx.send(row.property::<String>("mem"));
        });

        // None -> Some(1.5) displays "1.5%" and fires once.
        let mut known = item(5);
        known.value.mem_percent = Some(1.5);
        r.set_item(&known);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec!["1.5%".to_string()]);

        // Same value again: no notification.
        r.set_item(&known);
        assert!(rx.try_iter().collect::<Vec<String>>().is_empty());

        // Unrelated field changes: still quiet.
        let mut other = known.clone();
        other.value.username = "root".to_string();
        r.set_item(&other);
        assert!(
            rx.try_iter().collect::<Vec<String>>().is_empty(),
            "an unrelated change must not notify mem"
        );

        // Some -> None flips the cell back to blank and fires once.
        let blank = item(5);
        r.set_item(&blank);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            vec![String::new()],
            "Some -> None must re-fire mem with the blank cell"
        );
    }
}
