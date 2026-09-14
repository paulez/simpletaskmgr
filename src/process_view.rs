//! Process list view (clean-room): columns, per-column sort, 1.5 s refresh,
//! click-to-details.
//!
//! Implements [`doc/PROCESS_LIST_VIEW_SPEC.md`](doc/PROCESS_LIST_VIEW_SPEC.md).
//! The widget chain is GTK-native end to end:
//! ```text
//! ListStore<ViewRow> -> SortListModel(view's ColumnViewSorter)
//!                    -> SingleSelection (autoselect off) -> ColumnView
//! ```
//! so sorting, selection, and row-widget recycling all belong to GTK. One
//! [`ProcessView::update`] is one refresh tick:
//!
//! 1. update surviving rows **in place** — their six display properties are
//!    `notify`-only on change (no store signal at all for value changes),
//! 2. splice/remove + append rows for the membership change (GTK
//!    auto-resorts on those commits),
//! 3. one re-sort "kick" — proven to commit *nothing* when the visible
//!    order did not change (spec T3 evidence in `doc/PROCESS_LIST_VIEW_SPEC.md`),
//! 4. re-pin the single selection *by PID*, not by index (it survived the
//!    reorder as a position and may now point at another row),
//! 5. restore the scroll offset on idle (GTK clamps the adjustment during
//!    the layout pass that runs after this call returns).
//!
//! A separate diff/splice engine is unnecessary: the no-op kick is free,
//! and a genuine reorder commit is one `items-changed` signal that GTK can
//! pair with the re-additions to keep all row widgets (spec R2).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use glib::prelude::*;
use glib::subclass::prelude::*;

use crate::cell_label;
use crate::process::{ProcessItem, TaskMgrProcess};
use crate::SortColumn;
use gtk4::gio::prelude::*;
use gtk4::prelude::*;

glib::wrapper! {
    /// One list item. A `glib::Object` subclass holding one [`ProcessItem`],
    /// exposing one read-only string property per displayed column so each
    /// cell is a `Label` bound to one. A process keeps its one row object
    /// for its whole lifetime: refreshes rewrite it in place
    /// ([`ViewRow::set_item`]) and cells re-render via property notify —
    /// the row's object identity survives every refresh (spec R2).
    pub struct ViewRow(ObjectSubclass<imp::ViewRow>);
}

impl ViewRow {
    fn imp(&self) -> &imp::ViewRow {
        imp::ViewRow::from_obj(self)
    }

    /// Create a fresh row object from a snapshot.
    pub fn from_item(item: &ProcessItem) -> Self {
        let r: Self = glib::Object::new::<ViewRow>();
        *r.imp().data.borrow_mut() = item.clone();
        r
    }

    /// The row's current data snapshot (clone).
    pub fn item(&self) -> ProcessItem {
        self.imp().data.borrow().clone()
    }

    /// The row's stable identity (PID).
    pub fn pid(&self) -> i32 {
        self.imp().data.borrow().pid
    }

    /// Whether the row's data equals `item` (no-clone refresh bookkeeping).
    pub fn has_value(&self, item: &ProcessItem) -> bool {
        *self.imp().data.borrow() == *item
    }

    /// Replace the row's data and re-emit **only** the properties whose
    /// displayed text changed (spec R2: unchanged cells do not repaint; an
    /// old value is a notify, not a widget teardown).
    pub fn set_item(&self, item: &ProcessItem) {
        let imp = self.imp();
        let changed = {
            let o = imp.data.borrow();
            if *o == *item {
                return;
            }
            if o.pid != item.pid {
                log::warn!(
                    "set_item: PID {} -> {} on a live row — a live row must stay \
                     on the same process (data desync)",
                    o.pid,
                    item.pid
                );
            }
            [
                ("pid", o.pid != item.pid),
                ("name", o.value.name != item.value.name),
                (
                    "cpu",
                    o.value.cpu_percent_str() != item.value.cpu_percent_str(),
                ),
                (
                    "mem",
                    o.value.mem_percent_str() != item.value.mem_percent_str(),
                ),
                (
                    "disk-read",
                    o.value.disk_read_str() != item.value.disk_read_str(),
                ),
                (
                    "disk-write",
                    o.value.disk_write_str() != item.value.disk_write_str(),
                ),
            ]
            .into_iter()
            .filter(|(_, c)| *c)
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
        };
        *imp.data.borrow_mut() = item.clone();
        for name in changed {
            self.notify(name);
        }
    }
}

mod imp {
    use glib::prelude::*;
    use glib::subclass::basic;
    use glib::subclass::prelude::*;
    use std::cell::RefCell;

    use crate::process::ProcessItem;

    pub struct ViewRow {
        pub data: RefCell<ProcessItem>,
    }

    impl Default for ViewRow {
        fn default() -> Self {
            Self {
                data: RefCell::new(Default::default()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ViewRow {
        const NAME: &'static str = "SimpleTaskMgrViewRow";
        type Type = super::ViewRow;
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
                    .blurb("PID as text")
                    .build(),
                glib::ParamSpecString::builder("name")
                    .nick("Name")
                    .blurb("Process name")
                    .build(),
                glib::ParamSpecString::builder("cpu")
                    .nick("CPU %")
                    .blurb("CPU percent")
                    .build(),
                glib::ParamSpecString::builder("mem")
                    .nick("MEM %")
                    .blurb("MEM percent")
                    .build(),
                glib::ParamSpecString::builder("disk-read")
                    .nick("Disk read")
                    .blurb("Disk read speed")
                    .build(),
                glib::ParamSpecString::builder("disk-write")
                    .nick("Disk write")
                    .blurb("Disk write speed")
                    .build(),
            ]
        })
    }

    impl ObjectImpl for ViewRow {
        fn properties() -> &'static [glib::ParamSpec] {
            property_specs()
        }

        fn set_property(&self, _id: usize, _v: &glib::Value, _pspec: &glib::ParamSpec) {
            // Read-only: the data changes only through ViewRow::set_item.
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let item = self.data.borrow();
            match pspec.name() {
                "pid" => glib::Value::from(item.pid.to_string()),
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

/// Shared `SignalListItemFactory` for one text column: a single `Label` bound
/// to the row's string property `prop`.
///
/// GTK does not own a `glib::Binding`: dropping the Rust handle does *not*
/// dispose the binding. The factory therefore
/// parks the live binding in the label (via [`cell_label::park`]) and
/// disposes it on recycle (`[`cell_label::take_and_unbind`]`) — in
/// `connect_unbind` on the happy path and as a defensive tripwire in
/// `connect_bind` (spec B1).
fn cell_factory(prop_name: &'static str) -> gtk4::SignalListItemFactory {
    let f = gtk4::SignalListItemFactory::new();
    f.connect_setup(move |_f, li| {
        let li = li.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let label = gtk4::Label::new(None);
        label.set_xalign(0.0);
        li.set_child(Some(&label));
    });
    f.connect_unbind(move |_f, li| {
        let li = li.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let Some(child) = li.child() else {
            return;
        };
        let Ok(label) = child.downcast::<gtk4::Label>() else {
            return;
        };
        // Happy path: retire the parking (and its last reference) so the
        // recycled label holds no live binding to its previous row.
        if let Some(b) = cell_label::take_and_unbind(label.as_ptr() as usize) {
            b.unbind();
        }
    });
    f.connect_bind(move |_f, li| {
        let li = li.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let label = li
            .child()
            .expect("this row has a child")
            .downcast::<gtk4::Label>()
            .expect("this row's child is a label");
        let row = li
            .item()
            .expect("a row object")
            .downcast::<ViewRow>()
            .expect("a ViewRow");
        let key = label.as_ptr() as usize;
        // Defensive tripwire: a healthy factory always retires the previous
        // binding in `connect_unbind`, so a parking still alive here means
        // the previous unbind for this recycled slot did not run — the
        // stale source's binding could still `notify` into and overwrite
        // the new one. Retire it (the desync source) and `warn!` the event
        // (spec B1). A fresh label has no parking here.
        if cell_label::take_and_unbind(key).is_some() {
            log::warn!(
                "cell recycling: a stale binding to a previous row (property: {prop_name}) \n                 was still parked on this label — the previous unbind did not run; \n                 retired the stale source before binding a new row",
            );
        }
        // `g_object_bind_property` auto-drops the binding when either the row
        // object or the cell widget is destroyed — but GTK keeps the binding
        // C-side by itself, so park the handle to
        // let `connect_unbind` retire it by reference. `sync_create` copies
        // the current value into the label immediately (setup and bind may
        // run in either order).
        let binding = row
            .bind_property(prop_name, &label, "label")
            .sync_create()
            .build();
        cell_label::park(key, binding);
    });
    f
}

/// Per-column `Sorter` driving the native header. All columns share
/// [`compare_values`] as the sole order source (total order —
/// spec S5: equal rows keep their order under stable sort, so no two
/// "equal" rows can ever swap). Each closes over the **live** sort order so
/// `None` values are pinned to the bottom in *either* direction (spec S3).
fn column_sorter(
    sort_col: SortColumn,
    column: &gtk4::ColumnViewColumn,
    view_sorter: &gtk4::Sorter,
) -> gtk4::CustomSorter {
    let column_ptr = column.as_ptr() as usize;
    let view_sorter = view_sorter.clone();
    gtk4::CustomSorter::new(move |a: &glib::Object, b: &glib::Object| {
        let ra = a.downcast_ref::<ViewRow>().expect("a ViewRow");
        let rb = b.downcast_ref::<ViewRow>().expect("a ViewRow");
        let ia = ra.item();
        let ib = rb.item();
        // `true` only when this very column is the active, descending primary
        // sort; `false` otherwise — so the optional-value columns keep a
        // missing value pinned to the list bottom in either direction
        // (spec S3).
        let descending = view_sorter
            .downcast_ref::<gtk4::ColumnViewSorter>()
            .and_then(|cs| {
                let pc = cs.primary_sort_column()?;
                (pc.as_ptr() as usize == column_ptr).then_some(cs)
            })
            .map(|cs| cs.primary_sort_order() == gtk4::SortType::Descending)
            .unwrap_or(false);
        let ord = compare_values(&ia, &ib, sort_col, descending);
        ord.into()
    })
}

/// Compares two rows' values by the given column, in ascending order.
///
/// `f64` is compared with `total_cmp` so `NaN` values sort without
/// panicking (unlike `partial_cmp().unwrap()`). This is the single
/// source of truth for row ordering: every native `GtkColumnView`
/// header sorter wraps it. The comparator is a **total** order (spec S5):
/// equal rows compare `Equal`, so a stable sort never swaps two
/// "equal" rows.
///
/// `descending` reports whether the column is currently sorted
/// descending (most-first). The always-present columns (`Pid`,
/// `CpuPercent`, `Name`) ignore it; the columns that may be
/// empty (`MemPercent`, `DiskRead`, `DiskWrite`) use it to pin a missing
/// value to the bottom of the visible list in either direction (see
/// [`rank_optional`]).
fn compare_values(
    a: &ProcessItem,
    b: &ProcessItem,
    column: SortColumn,
    descending: bool,
) -> std::cmp::Ordering {
    match column {
        SortColumn::Pid => a.pid.cmp(&b.pid),
        SortColumn::CpuPercent => {
            // `top`-style stable ordering: compare the integer tick delta
            // of the latest sample (coarse — quantized to the sampling
            // window — so rows that display the same value are tied), and
            // tie-break on `pid` so tied rows keep a deterministic position
            // across refreshes instead of swapping. The displayed `f64`
            // percent is deliberately never compared here, so a `NaN`
            // value can't disturb the order either.
            a.value
                .cpu_ticks
                .cmp(&b.value.cpu_ticks)
                .then(a.pid.cmp(&b.pid))
        }
        SortColumn::MemPercent => {
            rank_optional(a.value.mem_percent, b.value.mem_percent, descending)
        }
        SortColumn::Name => a.value.name.cmp(&b.value.name),
        // A missing (`None`) rate is pinned to the bottom of the visible
        // list however the column points, so rows without I/O data never
        // float up ahead of rows with a real rate (see [`rank_optional`]).
        SortColumn::DiskRead => {
            rank_optional(a.value.disk_read_speed, b.value.disk_read_speed, descending)
        }
        SortColumn::DiskWrite => rank_optional(
            a.value.disk_write_speed,
            b.value.disk_write_speed,
            descending,
        ),
    }
}

/// Orders an optional numeric value (CPU-free: MEM% / disk r/w) against
/// another, always settling the missing (`None`) one at the *bottom* of the
/// visible list — whichever direction the column is pointed.
///
/// GTK4 applies the header's direction itself: for the descending (most-first)
/// direction it *negates* this comparator's whole result (see
/// `gtkcolumnviewsorter.c`). So a comparator that put `None` at the bottom in
/// ascending order would be flipped to the top in descending order. The two
/// `None`/`Some` cases must therefore return opposite orderings per direction:
///
/// * ascending (no negation): report `None` as *greater* ⇒ it lands last;
/// * descending (negated):    report `None` as *less* ⇒ negation lands it last.
///
/// `Some`/`Some` still orders by value (`total_cmp`, `NaN` safe) and `None`/`None`
/// is `Equal`, in both directions. This replaces the old `unwrap_or(0.0)`
/// shortcut, which tied a missing rate with a real zero and let it surface at
/// the top of a data-first (descending) sort — the "empty rows first" bug.
fn rank_optional(a: Option<f64>, b: Option<f64>, descending: bool) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (None, None) => std::cmp::Ordering::Equal,
        // `a` is the missing side: rank it last (greater when ascending,
        // least-then-negated-to-last when descending).
        (None, Some(_)) => {
            if descending {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        }
        // Mirror of the arm above for the case where `b` is the missing side.
        (Some(_), None) => {
            if descending {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        }
    }
}

/// `DetailLabels` keeps the detail-pane labels addressable, and the pane
/// itself can be hidden (spec D3).
#[derive(Clone)]
pub struct DetailLabels {
    pub pane: gtk4::ScrolledWindow,
    pub pid: gtk4::Label,
    pub name: gtk4::Label,
    pub cpu: gtk4::Label,
    pub mem: gtk4::Label,
    pub disk_read: gtk4::Label,
    pub disk_write: gtk4::Label,
}

/// Build the right-hand detail pane: one titled row per displayed field,
/// initially hidden (spec D1/D3).
fn build_detail_pane() -> DetailLabels {
    let box_v = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    box_v.add_css_class("detail-pane");

    let title = gtk4::Label::new(Some("Process Details"));
    title.add_css_class("detail-title");
    box_v.append(&title);

    fn mk_row(initial: &str) -> gtk4::Label {
        let l = gtk4::Label::new(Some(initial));
        l.add_css_class("detail-row");
        l.set_xalign(0.0);
        l
    }
    let d_pid = mk_row("PID: —");
    let d_name = mk_row("Name: —");
    let d_cpu = mk_row("CPU%: —");
    let d_mem = mk_row("MEM%: —");
    let d_disk_read = mk_row("Disk read: —");
    let d_disk_write = mk_row("Disk write: —");
    for row in [&d_pid, &d_name, &d_cpu, &d_mem, &d_disk_read, &d_disk_write] {
        box_v.append(row);
    }

    let pane = gtk4::ScrolledWindow::new();
    pane.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    pane.set_child(Some(&box_v));
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.set_visible(false); // nothing selected at startup

    DetailLabels {
        pane,
        pid: d_pid,
        name: d_name,
        cpu: d_cpu,
        mem: d_mem,
        disk_read: d_disk_read,
        disk_write: d_disk_write,
    }
}

/// Fill the pane fields from `item`; clear and hide when `None` (spec D2/D3). Show
/// "—" for unknown values (spec V4).
fn dash(s: &str) -> String {
    if s.is_empty() {
        "—".to_string()
    } else {
        s.to_string()
    }
}

fn apply_detail(d: &DetailLabels, item: Option<&ProcessItem>) {
    let Some(p) = item else {
        d.pane.set_visible(false);
        for l in [&d.pid, &d.name, &d.cpu, &d.mem, &d.disk_read, &d.disk_write] {
            l.set_label("");
        }
        return;
    };
    d.pane.set_visible(true);
    let p: &TaskMgrProcess = &p.value;
    d.pid.set_label(&format!("PID: {}", p.pid));
    d.name.set_label(&format!("Name: {}", p.name));
    d.cpu
        .set_label(&format!("CPU%: {}", dash(&p.cpu_percent_str())));
    d.mem
        .set_label(&format!("MEM%: {}", dash(&p.mem_percent_str())));
    d.disk_read
        .set_label(&format!("Disk read: {}", dash(&p.disk_read_str())));
    d.disk_write
        .set_label(&format!("Disk write: {}", dash(&p.disk_write_str())));
}

/// The list + detail view subtree to embed in the window, one process per
/// [`ViewRow`], with the refresh algorithm of the module docs. Owns no
/// `State`: the app feeds snapshot data via [`ProcessView::update`] and
/// learns the selection via the callback (spec D4/B2).
/// The single-selection callback (last registered wins — spec D4). Shared by
/// `Rc` so the GTK notify closure and `connect_selection_changed` both reach
/// the same slot (a plain `RefCell::clone()` would be a copy, not a share).
type SelectionCallback = Rc<RefCell<Option<Rc<dyn Fn(Option<i32>)>>>>;

/// Queue the S8 scroll-to-top so it runs on the *next* main-loop idle.
/// GTK settles a header click through the ColumnViewSorter `primary-sort-*`
/// properties first and commits the re-sort *after* (`gtk_sorter_changed`);
/// ListBase pins its per-item anchor from the *current* model order, so a
/// scroll made before the commit anchors the old top row's identity and the
/// viewport lands wherever that row moved to. An idle runs after the
/// commit, when row 0 is already the new top row. `scrolled` counts the
/// executions (a test/diagnostic hook; a refresh tick must never schedule
/// one — R3).
fn scroll_view_to_top_later(
    column_view: &gtk4::ColumnView,
    rows: &gtk4::SortListModel,
    scrolled: &Rc<Cell<u32>>,
) {
    let cv = column_view.clone();
    let rows = rows.clone();
    let scrolled = scrolled.clone();
    glib::idle_add_local(move || {
        if rows.n_items() > 0 {
            scroll_column_view_to_top(&cv);
            scrolled.set(scrolled.get() + 1);
        }
        glib::ControlFlow::Break
    });
}

/// Scroll the column view so the top row is at the very top (S8). The
/// empty-list guard lives in the deferral layer; `scroll_to(0)` asserts on
/// an empty list in release GTK builds too, so we never call it bare.
fn scroll_column_view_to_top(column_view: &gtk4::ColumnView) {
    column_view.scroll_to(
        0,
        Option::<&gtk4::ColumnViewColumn>::None,
        gtk4::ListScrollFlags::NONE,
        None,
    );
}

pub struct ProcessView {
    root: gtk4::Box,
    column_view: gtk4::ColumnView,
    store: gtk4::gio::ListStore,
    sort_model: gtk4::SortListModel,
    selection: gtk4::SingleSelection,
    sorter: gtk4::Sorter,
    /// Count of genuine sort-state changes (primary column or direction
    /// flipped) — the trigger for the S8 scroll-to-top. Diagnostic/test
    /// hook: an update-tick invalidation must **not** count as one (R3).
    sort_changes: Rc<Cell<u32>>,
    /// Executed S8 scroll-tops (the deferred idle ran with a non-empty
    /// list). Diagnostic/test hook for the deferral itself.
    scrolls_done: Rc<Cell<u32>>,
    list_scroll: gtk4::ScrolledWindow,
    detail: DetailLabels,
    /// One app callback at a time (last registered wins — spec D4). May fire
    /// *during* `update` (spec B2: the single-selection notify fires inside
    /// the model-change FFI) — registered closures must be re-entrancy safe.
    callback: SelectionCallback,
}

impl ProcessView {
    /// (title, row property, fixed width, stretch, sort column) for each view
    /// column — PID, Name, CPU%, MEM%, Disk R/W (spec V1). Name absorbs the
    /// remaining width; the rest stay fixed so `top`-like numbers never
    /// wobble (spec C3).
    const COLUMNS: [(&str, &str, i32, bool, SortColumn); 6] = [
        ("PID", "pid", 70, false, SortColumn::Pid),
        ("Name", "name", -1, true, SortColumn::Name),
        ("CPU%", "cpu", 70, false, SortColumn::CpuPercent),
        ("MEM%", "mem", 70, false, SortColumn::MemPercent),
        ("Disk R", "disk-read", 80, false, SortColumn::DiskRead),
        ("Disk W", "disk-write", 80, false, SortColumn::DiskWrite),
    ];

    /// Build the full view subtree. Call once on the GTK main thread (at
    /// window construction, GTK is already initialised by the app).
    pub fn new() -> Self {
        let store = gtk4::gio::ListStore::new::<ViewRow>();
        let column_view = gtk4::ColumnView::builder().build();

        // Interactive headers: every column gets a Sorter and the
        // SortListModel gets the view's own ColumnViewSorter.
        let sorter = column_view.sorter().expect("ColumnView exposes its sorter");
        let sort_model = gtk4::SortListModel::new(Some(store.clone()), Some(sorter.clone()));
        // S8: every header click settles the sort through these two
        // properties (new primary column, direction flip, or even a
        // redundant re-click) — and only header clicks do: an update-tick
        // invalidation kick (`Sorter::changed`) fires neither. Scrolling to
        // the top on them brings the new first row into view immediately,
        // without ever fighting a refresh (R3).
        let sort_changes = Rc::new(Cell::new(0u32));
        let scrolls_done = Rc::new(Cell::new(0u32));
        {
            let view_sorter = sorter
                .downcast_ref::<gtk4::ColumnViewSorter>()
                .expect("ColumnView's sorter is a GTK 4.10+ ColumnViewSorter");
            let cv = column_view.clone();
            let sm = sort_model.clone();
            let count = sort_changes.clone();
            let done = scrolls_done.clone();
            view_sorter.connect_primary_sort_column_notify(move |_| {
                count.set(count.get() + 1);
                scroll_view_to_top_later(&cv, &sm, &done);
            });
            let cv = column_view.clone();
            let sm = sort_model.clone();
            let count = sort_changes.clone();
            let done = scrolls_done.clone();
            view_sorter.connect_primary_sort_order_notify(move |_| {
                count.set(count.get() + 1);
                scroll_view_to_top_later(&cv, &sm, &done);
            });
        }
        let selection = gtk4::SingleSelection::new(Some(sort_model.clone()));
        selection.set_autoselect(false); // nothing selected at startup (spec D1)
        selection.set_can_unselect(true); // allow the NO_SELECTION sentinel (spec D2)

        for &(title, prop, width, stretch, sort_col) in Self::COLUMNS.iter() {
            let col = gtk4::ColumnViewColumn::new(Some(title), Some(cell_factory(prop)));
            if stretch {
                col.set_expand(true);
            } else {
                col.set_fixed_width(width);
            }
            col.set_resizable(true);
            col.set_sorter(Some(&column_sorter(sort_col, &col, &sorter)));
            column_view.append_column(&col);
        }
        column_view.set_model(Some(&selection));

        // S7: the list opens sorted by CPU%, highest-first (`top`-style).
        // `sort_by_column` *sets* the state (no toggle), then ordinary
        // header clicks work as documented (spec S2).
        let cpu_title = Self::COLUMNS
            .iter()
            .find(|c| c.4 == SortColumn::CpuPercent)
            .expect("CPU% is in COLUMNS")
            .0;
        let cols = column_view.columns();
        let cpu_col = (0..cols.n_items())
            .filter_map(|i| {
                cols.item(i)
                    .and_then(|o| o.downcast::<gtk4::ColumnViewColumn>().ok())
                    .filter(|c| c.title().as_deref() == Some(cpu_title))
            })
            .next()
            .expect("the CPU% column was appended above");
        column_view.sort_by_column(Some(&cpu_col), gtk4::SortType::Descending);
        column_view.add_css_class("process-list");

        let list_scroll = gtk4::ScrolledWindow::new();
        list_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        list_scroll.set_child(Some(&column_view));
        list_scroll.set_hexpand(true);
        list_scroll.set_vexpand(true);

        let detail = build_detail_pane();

        let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        root.add_css_class("body");
        root.append(&list_scroll);
        root.append(&detail.pane);

        // Selection -> detail pane + app callback. This is the path a real
        // click takes; `update` reenters it through the single-selection
        // notify raised by its own model change (spec B2).
        let d = detail.clone();
        let sel = selection.clone();
        let sel2 = sel.clone();
        let callback = Rc::new(RefCell::new(None::<Rc<dyn Fn(Option<i32>)>>));
        let cb = callback.clone();
        sel.connect_selected_notify(move |_| {
            let row = sel2
                .selected_item()
                .as_ref()
                .and_then(|o| o.downcast_ref::<ViewRow>())
                .cloned();
            let pid = row.as_ref().map(|r| r.pid());
            let item = row.as_ref().map(|r| r.item());
            apply_detail(&d, item.as_ref());
            let f = cb.borrow().clone();
            if let Some(f) = f {
                f(pid);
            }
        });

        Self {
            root,
            column_view,
            store,
            sort_model,
            selection,
            sorter,
            sort_changes,
            scrolls_done,
            list_scroll,
            detail,
            callback,
        }
    }

    /// The list + detail subtree to embed in the window.
    pub fn widget(&self) -> &gtk4::Box {
        &self.root
    }

    /// The right-hand detail pane (the app may re-size, scroll, or toggle
    /// it); its row labels are reachable through [`ProcessView::detail_labels`]
    /// in tests and for app-level inspection.
    pub fn detail_pane(&self) -> &gtk4::ScrolledWindow {
        &self.detail.pane
    }

    /// The detail pane's named labels (filled by GTK's own machinery on the
    /// current selection; the handle exists for inspection and so the app
    /// can decorate the pane, e.g. re-attach signal buttons).
    #[doc(hidden)]
    pub fn detail_labels(&self) -> &DetailLabels {
        &self.detail
    }

    /// The PID of the currently selected process (`None` if none / not a row).
    pub fn selected_pid(&self) -> Option<i32> {
        self.selection
            .selected_item()
            .as_ref()
            .map(|o| o.downcast_ref::<ViewRow>().expect("a ViewRow").pid())
    }

    /// Fire on selection change, with the new PID (`None` = deselected).
    pub fn connect_selection_changed(&self, f: impl Fn(Option<i32>) + 'static) {
        *self.callback.borrow_mut() = Some(Rc::new(f));
    }

    /// Start the first paint at the top of the list (spec R3 initial state).
    /// Safe on an empty list (the initial map can precede the first
    /// snapshot).
    pub fn scroll_to_top(&self) {
        if self.sort_model.n_items() > 0 {
            scroll_column_view_to_top(&self.column_view);
        }
    }

    /// Number of genuine sort changes (primary column or direction flipped;
    /// S8 schedules a scroll to the top on each). Test hook for that
    /// invariant.
    pub fn sort_change_count(&self) -> u32 {
        self.sort_changes.get()
    }

    /// Number of executed S8 scroll-tops (the deferred idle ran with a
    /// non-empty list) — the *deferred* half of `sort_change_count`.
    pub fn scroll_done_count(&self) -> u32 {
        self.scrolls_done.get()
    }

    /// One refresh tick (spec R): update survivors in place, apply the
    /// membership change, kick the re-sort (a free no-op when the order did
    /// not change — spec T3), re-pin the selection by PID (spec R4), and
    /// restore the scroll offset (spec R3).
    pub fn update(&self, procs: &[ProcessItem]) {
        let prev_pid = self.selected_pid();
        let stored_row = |store: &gtk4::gio::ListStore, pos: u32| -> ViewRow {
            store
                .item(pos)
                .expect("a row")
                .downcast::<ViewRow>()
                .expect("a ViewRow")
        };

        // Index current rows by PID before anything mutates the model.
        let old_n = self.store.n_items();
        let mut rows: HashMap<i32, ViewRow> = HashMap::with_capacity(old_n as usize);
        let mut order: Vec<i32> = Vec::with_capacity(old_n as usize);
        for i in 0..old_n {
            let row = stored_row(&self.store, i);
            order.push(row.pid());
            rows.insert(row.pid(), row);
        }
        let want: std::collections::HashSet<i32> = procs.iter().map(|p| p.pid).collect();

        // 1) Survivors: rewrite the value in place — no store signal,
        //    cells re-render via property notify (spec R2).
        for item in procs {
            if let Some(row) = rows.get(&item.pid) {
                if !row.has_value(item) {
                    row.set_item(item);
                }
            }
        }

        // 2) Membership: remove departed (single splice per contiguous run,
        //    back-to-front to keep earlier positions valid) and append the
        //    rest in snapshot order.
        let mut removed: Vec<u32> = order
            .iter()
            .enumerate()
            .filter(|(_, p)| !want.contains(*p))
            .map(|(i, _)| i as u32)
            .collect();
        removed.sort_unstable();
        let mut runs = Vec::new();
        let mut i = 0;
        while i < removed.len() {
            let mut end = removed[i];
            let mut j = i + 1;
            while j < removed.len() && removed[j] == end + 1 {
                end = removed[j];
                j += 1;
            }
            runs.push((removed[i], end - removed[i] + 1));
            i = j;
        }
        for (pos, len) in runs.iter().rev() {
            self.store.splice(*pos, *len, &[] as &[ViewRow]);
        }
        order.retain(|p| want.contains(p));
        for item in procs {
            if !order.contains(&item.pid) {
                self.store.append(&ViewRow::from_item(item));
                order.push(item.pid);
            }
        }

        // 3) Order: kick the re-sort. GTK commits nothing if the visible
        //    order is unchanged, so the common tick is free (spec T3).
        self.sorter.changed(gtk4::SorterChange::Different);

        // 4) Re-pin the selection by PID (spec R4): GTK kept a *position*;
        //    after the re-sort/replace it may point at another row (or no
        //    row). Resolve by process, not slot. If it is still on the right
        //    row, leave it alone — an extra `set_selected` would make
        //    ColumnView scroll the row back into view and the list jumps.
        if let Some(pid) = prev_pid {
            if self.selected_pid() != Some(pid) {
                match (0..self.sort_model.n_items()).find(|i| {
                    self.sort_model
                        .item(*i)
                        .and_then(|o| o.downcast::<ViewRow>().ok())
                        .is_some_and(|r| r.pid() == pid)
                }) {
                    Some(i) => self.selection.set_selected(i),
                    None => self.selection.set_selected(u32::MAX), // clear
                }
            }
        }

        // 5) Scroll restore (spec R3): the resize clamps the adjustment in
        //    the layout pass that ran *after* this call returned, so put the
        //    offset back next loop turn.
        let adj = self.list_scroll.vadjustment();
        let saved = adj.value();
        let save = saved;
        glib::idle_add_local(move || {
            if (adj.value() - save).abs() > 0.5 {
                adj.set_value(save);
            }
            glib::ControlFlow::Break
        });
    }
}

impl Default for ProcessView {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessView {
    /// The underlying list store (rows in snapshot order; not display order).
    pub fn store(&self) -> &gtk4::gio::ListStore {
        &self.store
    }
    /// The `SortListModel` wrapping the store (drives the visible order).
    pub fn sort_model(&self) -> &gtk4::SortListModel {
        &self.sort_model
    }
    /// The single-selection model (spec D2/D4, R4).
    pub fn selection(&self) -> &gtk4::SingleSelection {
        &self.selection
    }
    /// Activate a column sort like a header click (spec S/T).
    pub fn sort_like_click(&self, col: SortColumn, order: gtk4::SortType) {
        let title = Self::COLUMNS
            .iter()
            .find(|c| c.4 == col)
            .expect("a view column")
            .0;
        let cols = self.column_view.columns();
        for i in 0..cols.n_items() {
            let Ok(c) = cols
                .item(i)
                .expect("a column")
                .downcast::<gtk4::ColumnViewColumn>()
            else {
                continue;
            };
            if c.title().as_deref() == Some(title) {
                self.column_view.sort_by_column(Some(&c), order);
                return;
            }
        }
        panic!("column {} not found", title);
    }
    /// PIDs in the sort model, in display order.
    pub fn display_order(&self) -> Vec<i32> {
        (0..self.sort_model.n_items())
            .map(|i| {
                self.sort_model
                    .item(i)
                    .expect("a row")
                    .downcast::<ViewRow>()
                    .expect("a ViewRow")
                    .pid()
            })
            .collect()
    }
    /// The row object holding `pid`, wherever it sits in the store.
    pub fn row_of(&self, pid: i32) -> Option<ViewRow> {
        (0..self.store.n_items()).find_map(|i| {
            let r: ViewRow = self
                .store
                .item(i)
                .expect("a row")
                .downcast()
                .expect("a ViewRow");
            (r.pid() == pid).then_some(r)
        })
    }
}
