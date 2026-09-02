use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gio::prelude::*;
use gtk4::prelude::*;

use crate::config::RefreshInterval;
use crate::metrics::SystemMetrics;
use crate::process::ProcessItem;
use crate::process_list::ProcessList;
use crate::process_row::ProcessRow;
use crate::refresh_list;
use crate::settings::{settings_path, UserSettings};
use crate::signal::Signal;
use crate::usage_graph::{paint_usage_chart, ChartConfig, ChartPane};
use crate::SortColumn;

const CSS: &str = include_str!("ui.css");

/// The widgets of the detail pane, kept addressable by name so the pane can
/// be updated from anywhere without tuple index bookkeeping. `pane` is the
/// `ScrolledWindow` carrying the whole pane so it can be hidden to let the
/// list expand to full width when nothing is selected.
#[derive(Clone)]
struct DetailLabels {
    pane: gtk4::ScrolledWindow,
    pid: gtk4::Label,
    name: gtk4::Label,
    uid: gtk4::Label,
    username: gtk4::Label,
    cpu: gtk4::Label,
    mem: gtk4::Label,
    disk_read: gtk4::Label,
    disk_write: gtk4::Label,
    status: gtk4::Label,
}

/// The widgets the `Settings` popover exposes so its change handlers and the
/// reset path can address them without name lookup.
struct SettingsWidgets {
    button: gtk4::Button,
    check: gtk4::CheckButton,
    list: gtk4::ListBox,
    rows: Vec<gtk4::ListBoxRow>,
    reset: gtk4::Button,
}

/// The `ColumnView`-backed process list and the model chain in front of it,
/// kept addressable so the refresh/timer paths can drive the store and ask the
/// view to re-sort. `store` holds the concrete `ListStore<ProcessRow>` — both
/// `refresh_list::refresh` and the `SortListModel`/`SingleSelection` models
/// built on top of it.
struct ListView {
    store: gtk4::gio::ListStore,
    column_view: gtk4::ColumnView,
    sort_model: gtk4::SortListModel,
    selection: gtk4::SingleSelection,
    cpu_column: gtk4::ColumnViewColumn,
    list_scroll: gtk4::ScrolledWindow,
}

/// The widgets of the detail pane (the value labels, the signal buttons, and
/// the pane's `ScrolledWindow`), so the selection/signal/refresh paths can
/// update it without holding a list of widget clones.
struct DetailPane {
    scroll: gtk4::ScrolledWindow,
    labels: DetailLabels,
    sighup: gtk4::Button,
    sigkill: gtk4::Button,
}

struct State {
    process_list: ProcessList,
    metrics: SystemMetrics,
    selected_pid: Option<i32>,
    settings: UserSettings,
    save_path: std::path::PathBuf,
    timer_id: Cell<Option<glib::SourceId>>,
    /// Top of the frequency-axis domain in MHz, read once at launch from
    /// `scaling_max_freq`. The fallback (4.0 GHz) covers hosts without a
    /// `cpufreq` interface (e.g. VMs); in that case the series is absent
    /// anyway and the domain is unused.
    freq_max_mhz: f64,
    /// Top of the memory-axis domain in MB — the installed RAM, read once at
    /// launch from `/proc/meminfo` `MemTotal`. The fallback (16 GB) covers
    /// hosts where `/proc/meminfo` is unavailable.
    mem_max_mb: f64,
}

#[derive(Debug)]
enum KillStatus {
    Sent,
    NoSelection,
    Failed(String),
}

impl State {
    fn new() -> Self {
        Self::with_settings_path(settings_path())
    }

    /// Constructs state that loads and persists settings at `path`.
    /// Production code uses the conventional location via [`State::new`];
    /// tests point this at a temporary path so they never touch the real
    /// configuration file.
    pub fn with_settings_path(path: std::path::PathBuf) -> Self {
        let settings = UserSettings::load(&path);
        let mut process_list = ProcessList::init();
        process_list.set_show_all(settings.show_all);
        let mut metrics = SystemMetrics::new();
        metrics.push_sample();
        Self {
            process_list,
            metrics,
            selected_pid: None,
            settings,
            save_path: path,
            timer_id: Cell::new(None),
            freq_max_mhz: crate::cpu_status::read_max_freq_mhz().unwrap_or(4000.0),
            mem_max_mb: crate::metrics::read_mem_total_mb().unwrap_or(16.0 * 1024.0),
        }
    }

    /// Returns the id of the running refresh timer, if any, and clears the
    /// slot. `build_window` fills the slot; the timer-callback and
    /// interval-change paths take it out when they swap sources.
    pub fn take_timer_id(&self) -> Option<glib::SourceId> {
        self.timer_id.take()
    }

    fn refresh(&mut self) {
        self.process_list.update_process_list();
        self.metrics.push_sample();
    }

    fn find(&self, pid: i32) -> Option<&ProcessItem> {
        self.process_list.processes.iter().find(|p| p.pid == pid)
    }

    /// Sets the show-all filter and persists it. Caller is responsible for
    /// refreshing + republishing so the change takes effect immediately.
    fn set_show_all(&mut self, show_all: bool) {
        if self.settings.show_all != show_all {
            self.settings.show_all = show_all;
            self.save_settings();
        }
        self.process_list.set_show_all(show_all);
    }

    /// Sets the refresh-interval preset and persists it. The caller restarts
    /// the running timer (see `take_timer_id`) so the new period applies
    /// immediately. Returns whether the value actually changed.
    fn set_refresh_interval(&mut self, interval: RefreshInterval) -> bool {
        if self.settings.refresh != interval {
            self.settings.refresh = interval;
            self.save_settings();
            true
        } else {
            false
        }
    }

    /// Resets all settings to defaults, applies them, and persists. Returns
    /// whether the refresh interval changed (so the caller knows it must
    /// restart the running timer).
    fn reset_settings(&mut self) -> bool {
        let defaults = UserSettings::default();
        self.set_show_all(defaults.show_all);
        self.set_refresh_interval(defaults.refresh)
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.save_path) {
            log::error!("Failed to save settings: {e:?}");
        }
    }

    fn kill(&mut self, sig: Signal) -> KillStatus {
        match self.selected_pid {
            Some(pid) => match crate::signal::send_signal(pid, sig) {
                Ok(()) => KillStatus::Sent,
                Err(e) => KillStatus::Failed(format!("{} failed: {e:?}", sig.name())),
            },
            None => KillStatus::NoSelection,
        }
    }
}

fn detail_status(status: &KillStatus) -> String {
    match status {
        KillStatus::Sent => "Sent.".to_string(),
        KillStatus::NoSelection => "No process selected.".to_string(),
        KillStatus::Failed(m) => m.clone(),
    }
}

/// (Re)starts the refresh timer at the interval currently held in `state`.
/// Must be called on the main loop thread.
fn restart_timer(
    state: &Rc<RefCell<State>>,
    rebuild: &Rc<dyn Fn()>,
    graph_areas: &[gtk4::DrawingArea],
) {
    if let Some(old) = state.borrow().take_timer_id() {
        old.remove();
    }
    let interval = state.borrow().settings.refresh.as_duration();
    let state_t = state.clone();
    let rebuild_t = rebuild.clone();
    let graphs = graph_areas.to_vec();
    let state_cb = state_t.clone();
    let id = glib::timeout_add_local(interval, move || {
        state_cb.borrow_mut().refresh();
        rebuild_t();
        for a in &graphs {
            a.queue_draw();
        }
        glib::ControlFlow::Continue
    });
    state_t.borrow_mut().timer_id.set(Some(id));
}

/// Builds one pane of the split usage-graph row: a short centered title
/// label above a [`gtk4::DrawingArea`]. The label is a standard GTK `Label`
/// (theme color, theme font, centered) so it stays readable in both light
/// and dark themes; the draw area below paints the pane's series via
/// `paint_usage_chart`.
fn build_graph_pane(
    state: &Rc<RefCell<State>>,
    title: &'static str,
    pane: ChartPane,
) -> (gtk4::Box, gtk4::DrawingArea) {
    let box_v = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    box_v.set_hexpand(true);

    let label = gtk4::Label::new(Some(title));
    label.set_halign(gtk4::Align::Center);
    box_v.append(&label);

    let drawing = gtk4::DrawingArea::new();
    drawing.set_content_height(80);
    drawing.set_hexpand(true);
    drawing.add_css_class("graph-area");
    let st = state.clone();
    drawing.set_draw_func(move |_da, cr: &gtk4::cairo::Context, w: i32, h: i32| {
        let samples = st.borrow().metrics.history();
        let cfg = ChartConfig {
            freq_max_mhz: st.borrow().freq_max_mhz,
            mem_max_mb: st.borrow().mem_max_mb,
            capacity: SystemMetrics::MAX_HISTORY,
        };
        paint_usage_chart(cr, w as f64, h as f64, &samples, &cfg, pane);
    });
    box_v.append(&drawing);

    (box_v, drawing)
}

/// Builds the split usage-graph row — a horizontal strip of two panes
/// (each pane: a centered title label above a [`gtk4::DrawingArea`]),
/// separated by a GTK default vertical `Separator`. The left pane draws the
/// CPU + memory utilization series (percent); the right pane draws the CPU
/// frequency + temperature series (MHz / °C), each on its own dedicated
/// axis. Both panes read the same rolling history and are redrawn in
/// lockstep on every refresh.
fn build_graph_row(state: &Rc<RefCell<State>>) -> (gtk4::Box, Vec<gtk4::DrawingArea>) {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    row.set_hexpand(true);

    let (pane_l, left) = build_graph_pane(state, "CPU & Memory", ChartPane::CpuMem);
    row.append(&pane_l);

    let sep = gtk4::Separator::new(gtk4::Orientation::Vertical);
    row.append(&sep);

    let (pane_r, right) = build_graph_pane(state, "CPU Freq & Temp", ChartPane::FreqTemp);
    row.append(&pane_r);

    (row, vec![left, right])
}

/// One shared `SignalListItemFactory` for a text column: a single `Label`
/// whose `label` is property-bound to the row's `ProcessRow` string property
/// (`prop_name`). Re-texting a cell on refresh is a pure `g_object_notify`.
fn make_cell_factory(prop_name: &'static str) -> gtk4::SignalListItemFactory {
    let f = gtk4::SignalListItemFactory::new();
    f.connect_setup(move |_f, li| {
        let li = li.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let label = gtk4::Label::new(None);
        label.set_xalign(0.0);
        li.set_child(Some(&label));
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
            .downcast::<ProcessRow>()
            .expect("a ProcessRow");
        // `g_object_bind_property` auto-drops the binding when either the
        // row object or this cell widget is destroyed. `sync_create`
        // copies the current value into the label immediately (the
        // factory may run its `setup`/`bind` in either order, so we do
        // not rely on the `notify` order).
        row.bind_property(prop_name, &label, "label")
            .sync_create()
            .build();
    });
    f
}

/// Per-column `Sorter` that drives the native header: a header click asks the
/// view to sort by that column, and the `SortListModel` re-orders the rows.
/// All columns share one comparator source of truth —
/// `ProcessList::compare_values` — so the header ordering can never disagree
/// with the data.
fn make_column_sorter(sort_col: SortColumn) -> gtk4::CustomSorter {
    gtk4::CustomSorter::new(move |a: &glib::Object, b: &glib::Object| {
        let ra = a.downcast_ref::<ProcessRow>().expect("a ProcessRow");
        let rb = b.downcast_ref::<ProcessRow>().expect("a ProcessRow");
        let ia = ra.item();
        let ib = rb.item();
        ProcessList::compare_values(&ia, &ib, sort_col).into()
    })
}

/// Builds the `ColumnView`-backed process list and its model chain:
/// `ListStore<ProcessRow> -> SortListModel -> SingleSelection -> ColumnView`.
///
/// The header row is the one `GtkColumnView` draws natively: it is
/// interactive because every column carries a `Sorter` and the view's
/// `SortListModel` carries the view's own `ColumnViewSorter`. Clicking a
/// header cell re-sorts exactly that column (and toggles direction on a
/// second click) entirely inside GTK — no app-side sort state, no custom
/// header widgets. Columns are resizable via header drag.
///
/// The store is spliced in place by `refresh_list::refresh` (the
/// anti-flicker contract), and the `SortListModel` re-sorts it with a
/// single remove+re-add `items-changed` signal on a header click — GTK
/// keeps every row object (and its widgets) alive across both.
fn build_process_list() -> ListView {
    let store = gtk4::gio::ListStore::new::<ProcessRow>();
    let column_view = gtk4::ColumnView::builder().build();
    // `gtk_column_view_sort_by_column` sorts through this model, and the
    // header only becomes clickable once the view reports a sorter: the
    // `ColumnViewSorter` GTK hands out when the view is attached to a model.
    let sort_model = gtk4::SortListModel::new(
        Some(store.clone()),
        Some(column_view.sorter().expect("ColumnView exposes a sorter")),
    );
    let selection = gtk4::SingleSelection::new(Some(sort_model.clone()));
    // `GtkSingleSelection` autoselects the first row by default — so a row is
    // already selected when the list is first populated. Disable it so
    // launching shows no selection and the list takes the full width.
    selection.set_autoselect(false);

    // (title, property of `ProcessRow`, fixed width in px, expand?, sort
    // column) — column widths keep the numeric columns compact and let the
    // Name column absorb the remaining width. Every column is user-resizable
    // by dragging its header.
    const COLS: [(&str, &str, i32, bool, SortColumn); 7] = [
        ("PID", "pid", 70, false, SortColumn::Pid),
        ("User", "username", 90, false, SortColumn::Username),
        ("Name", "name", -1, true, SortColumn::Name),
        ("CPU%", "cpu", 70, false, SortColumn::CpuPercent),
        ("MEM%", "mem", 70, false, SortColumn::MemPercent),
        ("Disk R", "disk-read", 80, false, SortColumn::DiskRead),
        ("Disk W", "disk-write", 80, false, SortColumn::DiskWrite),
    ];

    let mut columns: Vec<gtk4::ColumnViewColumn> = Vec::new();
    for (title, prop, width, expand, sort_col) in COLS.iter() {
        let col = gtk4::ColumnViewColumn::new(Some(title), Some(make_cell_factory(prop)));
        if *expand {
            col.set_expand(true);
        } else if *width > 0 {
            col.set_fixed_width(*width);
        }
        col.set_resizable(true);
        col.set_sorter(Some(&make_column_sorter(*sort_col)));
        column_view.append_column(&col);
        columns.push(col);
    }
    // The CPU% column, kept so the initial sort can address it.
    let cpu_column = columns[3].clone();
    column_view.set_model(Some(&selection));

    // GTK starts every *freshly activated* column ascending (the header-click
    // path sets `inverted = FALSE` for a new column). For a process list that
    // is backwards on the numeric columns: the interesting value (most CPU,
    // most RAM, fastest I/O) belongs at the top. Flip a first-time-activated
    // numeric column to descending, while leaving PID/User/Name ascending and
    // keeping the second-click toggle. We watch the view sorter's `changed`
    // and re-issue `sort_by_column` (which honors the direction explicitly)
    // only for a column that newly became primary and is still ascending; a
    // toggle or a sort refresh leaves the primary unchanged and is left alone.
    use std::collections::HashSet;
    let numeric_ptrs: HashSet<usize> = COLS
        .iter()
        .zip(&columns)
        .filter(|(spec, _)| {
            matches!(
                spec.4,
                SortColumn::CpuPercent
                    | SortColumn::MemPercent
                    | SortColumn::DiskRead
                    | SortColumn::DiskWrite
            )
        })
        .map(|(_, col)| col.as_ptr() as usize)
        .collect();

    let view_sorter = column_view.sorter().expect("ColumnView exposes a sorter");
    let prev_primary = Rc::new(RefCell::new(None::<usize>));
    let flipping = Rc::new(RefCell::new(false));
    let cv = column_view.clone();
    view_sorter.connect_changed(move |sorter, _change| {
        // `sort_by_column` below re-emits `changed`; ignore that re-entrancy.
        if *flipping.borrow() {
            return;
        }
        let Some(s) = sorter.downcast_ref::<gtk4::ColumnViewSorter>() else {
            return;
        };
        let Some(col) = s.primary_sort_column() else {
            *prev_primary.borrow_mut() = None;
            return;
        };
        let ptr = col.as_ptr() as usize;
        let new_primary = Some(ptr) != *prev_primary.borrow();
        *prev_primary.borrow_mut() = Some(ptr);
        if new_primary
            && numeric_ptrs.contains(&ptr)
            && s.primary_sort_order() == gtk4::SortType::Ascending
        {
            *flipping.borrow_mut() = true;
            cv.sort_by_column(Some(&col), gtk4::SortType::Descending);
            *flipping.borrow_mut() = false;
        }
    });

    column_view.add_css_class("process-list");

    let list_scroll = gtk4::ScrolledWindow::new();
    list_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    list_scroll.set_child(Some(&column_view));
    list_scroll.set_hexpand(true);
    list_scroll.set_vexpand(true);

    ListView {
        store,
        column_view,
        sort_model,
        selection,
        cpu_column,
        list_scroll,
    }
}

/// Builds the process detail pane: the value labels, the SIGHUP/SIGKILL
/// buttons, and the pane's `ScrolledWindow`. The pane starts hidden so the
/// list takes the full width at launch (selection re-shows it via
/// `apply_detail`).
fn build_detail_pane() -> DetailPane {
    let detail_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    detail_box.add_css_class("detail-pane");

    let title = gtk4::Label::new(Some("Process Details"));
    title.add_css_class("detail-title");
    detail_box.append(&title);

    fn mk_detail(initial: &str) -> gtk4::Label {
        let l = gtk4::Label::new(Some(initial));
        l.add_css_class("detail-row");
        l.set_xalign(0.0);
        l
    }
    let d_pid = mk_detail("PID: —");
    let d_name = mk_detail("Name: —");
    let d_uid = mk_detail("UID: —");
    let d_user = mk_detail("Username: —");
    let d_cpu = mk_detail("CPU%: —");
    let d_mem = mk_detail("MEM%: —");
    let d_disk_read = mk_detail("Disk read: —");
    let d_disk_write = mk_detail("Disk write: —");
    detail_box.append(&d_pid);
    detail_box.append(&d_name);
    detail_box.append(&d_uid);
    detail_box.append(&d_user);
    detail_box.append(&d_cpu);
    detail_box.append(&d_mem);
    detail_box.append(&d_disk_read);
    detail_box.append(&d_disk_write);

    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.add_css_class("detail-buttons");
    let b_sighup = gtk4::Button::new();
    b_sighup.set_label("Send SIGHUP");
    b_sighup.add_css_class("signal-btn");
    b_sighup.add_css_class("suggested-action");
    let b_sigkill = gtk4::Button::new();
    b_sigkill.set_label("Send SIGKILL");
    b_sigkill.add_css_class("signal-btn");
    b_sigkill.add_css_class("destructive-action");
    btn_box.append(&b_sighup);
    btn_box.append(&b_sigkill);
    detail_box.append(&btn_box);

    let d_status = gtk4::Label::new(Some(""));
    d_status.add_css_class("detail-status");
    d_status.set_xalign(0.0);
    d_status.set_wrap(true);
    detail_box.append(&d_status);

    let detail_scroll = gtk4::ScrolledWindow::new();
    detail_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    detail_scroll.set_child(Some(&detail_box));
    detail_scroll.set_hexpand(true);
    detail_scroll.set_vexpand(true);
    // Nothing is selected at launch — start with the pane hidden so the
    // list uses the full width (selection re-shows it via `apply_detail`).
    detail_scroll.set_visible(false);

    let labels = DetailLabels {
        pane: detail_scroll.clone(),
        pid: d_pid,
        name: d_name,
        uid: d_uid,
        username: d_user,
        cpu: d_cpu,
        mem: d_mem,
        disk_read: d_disk_read,
        disk_write: d_disk_write,
        status: d_status,
    };

    DetailPane {
        scroll: detail_scroll,
        labels,
        sighup: b_sighup,
        sigkill: b_sigkill,
    }
}

/// Builds the `Settings` button and its popover (show-all toggle, refresh
/// interval list, and reset button).
fn build_settings_popover() -> SettingsWidgets {
    let settings_btn = gtk4::Button::new();
    settings_btn.set_label("Settings");
    settings_btn.add_css_class("settings-btn");

    let popover = gtk4::Popover::new();
    popover.set_has_arrow(true);
    popover.set_position(gtk4::PositionType::Bottom);
    let pop_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    pop_box.add_css_class("settings-popover");

    let show_all_check = gtk4::CheckButton::new();
    show_all_check.set_label(Some("Show all processes"));
    pop_box.append(&show_all_check);

    let refresh_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let refresh_lbl = gtk4::Label::new(Some("Refresh interval"));
    refresh_lbl.add_css_class("settings-label");
    refresh_row.append(&refresh_lbl);
    let refresh_list = gtk4::ListBox::new();
    refresh_list.add_css_class("settings-refresh");
    refresh_list.set_selection_mode(gtk4::SelectionMode::Single);
    let mut refresh_rows: Vec<gtk4::ListBoxRow> = Vec::new();
    for interval in RefreshInterval::ALL {
        let row = gtk4::ListBoxRow::new();
        let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        let row_lbl = gtk4::Label::new(Some(interval.label()));
        row_lbl.add_css_class("settings-refresh-row");
        row_lbl.set_xalign(0.0);
        row_box.append(&row_lbl);
        row.set_child(Some(&row_box));
        refresh_list.append(&row);
        refresh_rows.push(row);
    }
    refresh_row.append(&refresh_list);
    pop_box.append(&refresh_row);

    let reset_btn = gtk4::Button::new();
    reset_btn.set_label("Reset to defaults");
    reset_btn.add_css_class("settings-reset");
    pop_box.append(&reset_btn);

    popover.set_child(Some(&pop_box));
    settings_btn.set_child(Some(&popover));

    SettingsWidgets {
        button: settings_btn,
        check: show_all_check,
        list: refresh_list,
        rows: refresh_rows,
        reset: reset_btn,
    }
}

/// Builds the shared "republish the store from `state`" closure: it updates
/// the row values in place, kicks the `SortListModel` to re-sort, restores the
/// scroll offset, re-pins the selection, and refreshes the detail pane.
fn make_rebuild(
    state: Rc<RefCell<State>>,
    list: &ListView,
    dp: &Rc<DetailLabels>,
    adj: gtk4::Adjustment,
) -> Rc<dyn Fn()> {
    let store_r = list.store.clone();
    let sel_r = list.selection.clone();
    let sort_r = list.sort_model.clone();
    let state_r = state.clone();
    // The view's own sorter, captured so the refresh path can ask the
    // `SortListModel` to re-run it after an in-place value update (see the
    // `changed` call below).
    let view_sorter_r = list
        .column_view
        .sorter()
        .expect("ColumnView exposes a sorter");
    let dp_r = dp.clone();
    let adj_r = adj.clone();
    Rc::new(move || {
        // Remember the currently selected pid (if any) so the highlight can be
        // re-pinned to the same process after the refresh, in case GTK
        // dropped or displaced it while mutating the store.
        let prev_pid: Option<i32> = sel_r
            .selected_item()
            .as_ref()
            .and_then(|o| o.downcast_ref::<ProcessRow>())
            .map(|r| r.item().pid);
        let items = state_r.borrow().process_list.processes.clone();

        // A refresh changes the store's size, and `GtkAdjustment` clamps
        // `value` to the valid range whenever `upper`/`page` change at layout
        // time (which is *after* this synchronous call returns). Save the
        // current offset, and on the next main-loop tick — once GTK has
        // re-allocated the rows and applied the clamp — restore it if it
        // drifted. The adjustment auto-clamps to the valid range anyway.
        let saved = adj_r.value();

        refresh_list::refresh(&store_r, &items);

        // `refresh` updated the row *values* in place — it deliberately emits
        // no `items-changed` for a stable-position value update (that is the
        // anti-flicker contract). But `GtkSortListModel` only re-sorts when
        // the store fires `items-changed` or its sorter emits `changed`; it
        // *never* watches item properties. Without this kick the list would
        // freeze at the order captured during the last membership change and
        // drift away from the CPU% the labels are showing. Firing `changed`
        // makes the `SortListModel` re-run our comparator; `gtk_column_view_
        // sorter_set_column` does exactly this on every header click, so this
        // reuses the same, proven path. If no column is sorted yet the
        // sorter reports order NONE and the `changed` signal is a no-op.
        view_sorter_r.changed(gtk4::SorterChange::Different);

        let adj_idle = adj_r.clone();
        let saved_idle = saved;
        glib::idle_add_local(move || {
            if (adj_idle.value() - saved_idle).abs() > 0.5 {
                adj_idle.set_value(saved_idle);
            }
            glib::ControlFlow::Break
        });

        if let Some(p) = prev_pid {
            let still_selected = sel_r.selected_item().as_ref().is_some_and(|o| {
                o.downcast_ref::<ProcessRow>()
                    .is_some_and(|r| r.item().pid == p)
            });
            if !still_selected {
                // The selection wraps the *sorted* model, so resolve the
                // position in the `SortListModel`'s space, not the store's.
                if let Some(i) = (0..sort_r.n_items()).find(|i| {
                    sort_r
                        .item(*i)
                        .and_then(|o| o.downcast::<ProcessRow>().ok())
                        .is_some_and(|row| row.item().pid == p)
                }) {
                    // The row's position changed (or its object was
                    // replaced) and the highlight dropped — re-pin it. We
                    // deliberately skip the work when the highlight is still
                    // on the same pid: re-invoking `set_selected` on every
                    // refresh would fire the `selected` change and make the
                    // `ColumnView` re-scroll the row into view, which is what
                    // made the list jump around. (If the *position* of the
                    // selected row changes, we do re-pin it here and accept
                    // the one-time scroll as the price of keeping the
                    // highlight glued to the process.)
                    sel_r.set_selected(i);
                }
            }
        }
        apply_detail(&dp_r, &state_r, state_r.borrow().selected_pid);
    })
}

/// Builds the main window and wires the refresh timer.
/// Call from the `activate` handler (main loop thread only).
pub fn build_window(app: &gtk4::Application) -> gtk4::ApplicationWindow {
    let state = Rc::new(RefCell::new(State::new()));

    let window = gtk4::ApplicationWindow::new(app);
    window.set_title(Some("Simple Task Manager"));
    window.set_default_size(940, 600);
    window.add_css_class("app-root");

    load_css();

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    window.set_child(Some(&root));

    let (graph_row, graph_areas) = build_graph_row(&state);
    let list = build_process_list();
    let detail = build_detail_pane();
    let settings = build_settings_popover();

    // ---- Body row (list | detail) ---------------------------------------------
    let body = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    body.add_css_class("body");
    body.append(&list.list_scroll);
    body.append(&detail.scroll);

    // ---- Assemble root ---------------------------------------------------------
    root.append(&graph_row);

    // ---- Settings button + popover ---------------------------------------------
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    toolbar.add_css_class("toolbar");
    toolbar.append(&settings.button);
    root.insert_child_after(&toolbar, Some(&graph_row));

    root.append(&body);

    // ---- Shared closure: republish the store from state ------------------------
    let dp_labels = Rc::new(detail.labels);
    let adj = list.list_scroll.vadjustment();
    let rebuild = make_rebuild(state.clone(), &list, &dp_labels, adj);

    // ---- Row selection handler -------------------------------------------------
    {
        let state_s = state.clone();
        let dp_s = dp_labels.clone();
        let sel_n = list.selection.clone();
        let sel_inner = sel_n.clone();
        sel_n.connect_selected_notify(move |_| {
            let pid = sel_inner
                .selected_item()
                .as_ref()
                .and_then(|o| o.downcast_ref::<ProcessRow>())
                .map(|r| r.item().pid);
            state_s.borrow_mut().selected_pid = pid;
            apply_detail(&dp_s, &state_s, pid);
        });
    }

    // ---- Settings popover handlers ----------------------------------------------
    // Initialize the widgets from the loaded settings.
    let loaded = state.borrow().settings.clone();
    settings.check.set_active(loaded.show_all);
    let initial_idx = RefreshInterval::ALL
        .iter()
        .position(|i| *i == loaded.refresh)
        .unwrap_or(1);
    settings.list.select_row(Some(&settings.rows[initial_idx]));
    {
        let state_t = state.clone();
        let rebuild_t = rebuild.clone();
        settings.check.connect_toggled(move |chk| {
            let active = chk.is_active();
            state_t.borrow_mut().set_show_all(active);
            // Refresh now so the filter change takes effect immediately rather
            // than waiting up to the next refresh tick.
            state_t.borrow_mut().refresh();
            rebuild_t();
        });
    }
    {
        let state_c = state.clone();
        let rows_c = settings.rows.clone();
        let rebuild_c = rebuild.clone();
        let graphs_c = graph_areas.clone();
        settings.list.connect_row_activated(move |_list, row| {
            if let Some(pos) = rows_c.iter().position(|r| *r == *row) {
                if let Some(interval) = RefreshInterval::ALL.get(pos) {
                    let changed = state_c.borrow_mut().set_refresh_interval(*interval);
                    if changed {
                        restart_timer(&state_c, &rebuild_c, &graphs_c);
                    }
                }
            }
        });
    }
    {
        let state_r = state.clone();
        let rebuild_r = rebuild.clone();
        let graphs_r = graph_areas.clone();
        let check_w = settings.check.clone();
        let rows_w = settings.rows.clone();
        let list_w = settings.list.clone();
        settings.reset.connect_clicked(move |_| {
            // `reset_settings` reports whether the refresh interval changed;
            // only then is the running timer restarted.
            let changed = state_r.borrow_mut().reset_settings();
            if changed {
                restart_timer(&state_r, &rebuild_r, &graphs_r);
            }
            sync_settings_widgets(&state_r, &check_w, &list_w, &rows_w);
            state_r.borrow_mut().refresh();
            rebuild_r();
        });
    }

    // ---- Signal buttons -----------------------------------------------------------
    {
        let state_k = state.clone();
        let dp_k = dp_labels.clone();
        detail.sighup.connect_clicked(move |_| {
            let status = state_k.borrow_mut().kill(Signal::Sighup);
            dp_k.status.set_label(&detail_status(&status));
        });
        let state_k = state.clone();
        let dp_k = dp_labels.clone();
        let rebuild_k = rebuild.clone();
        detail.sigkill.connect_clicked(move |_| {
            let status = state_k.borrow_mut().kill(Signal::Sigkill);
            dp_k.status.set_label(&detail_status(&status));
            // A killed process disappears on the next refresh; force one now
            // so the row is removed immediately rather than waiting up to 1.5s.
            state_k.borrow_mut().refresh();
            rebuild_k();
        });
    }

    // ---- Initial paint -----------------------------------------------------------
    // Populate the store first (which also sorts it via the SortListModel
    // once a sort is active), then apply the default sort — CPU% descending,
    // matching the previous "highest first" launch state. `sort_by_column`
    // is a no-op on an empty model, so run it after the data exists.
    rebuild();
    list.column_view
        .sort_by_column(Some(&list.cpu_column), gtk4::SortType::Descending);
    // GTK4's layout pass re-scrolls the list as the initial sort reorders
    // rows, leaving it parked in the middle at launch. A raw
    // `adjustment.set_value(0)` gets clobbered by that layout commit, so
    // route through GTK's own `scroll_to` at the deterministic "just got
    // mapped" point — after layout has resolved — instead of fighting it.
    let cv = list.column_view.clone();
    let win = window.clone();
    win.connect_map(move |_| {
        cv.scroll_to(
            0,
            Option::<&gtk4::ColumnViewColumn>::None,
            gtk4::ListScrollFlags::NONE,
            None,
        );
    });

    // ---- Refresh timer ------------------------------------------------------------
    restart_timer(&state, &rebuild, &graph_areas);

    window
}

fn apply_detail(dp: &Rc<DetailLabels>, state: &Rc<RefCell<State>>, pid: Option<i32>) {
    let item = pid.and_then(|p| {
        let s = state.borrow();
        s.find(p).cloned()
    });
    match item {
        Some(item) => {
            dp.pane.set_visible(true);
            let p = &item.value;
            dp.pid.set_label(&format!("PID: {}", p.pid));
            dp.name.set_label(&format!("Name: {}", p.name));
            dp.uid.set_label(&format!("UID: {}", p.ruid));
            dp.username.set_label(&format!("Username: {}", p.username));
            dp.cpu.set_label(&format!("CPU%: {}", p.cpu_percent_str()));
            dp.mem.set_label(&if p.mem_percent_str().is_empty() {
                "MEM%: —".to_string()
            } else {
                format!("MEM%: {}", p.mem_percent_str())
            });
            // An empty speed is not yet measured (first sample) or not
            // readable — show a placeholder rather than a zero.
            dp.disk_read.set_label(&if p.disk_read_str().is_empty() {
                "Disk read: —".to_string()
            } else {
                format!("Disk read: {}", p.disk_read_str())
            });
            dp.disk_write.set_label(&if p.disk_write_str().is_empty() {
                "Disk write: —".to_string()
            } else {
                format!("Disk write: {}", p.disk_write_str())
            });
            dp.status.set_label("");
        }
        None => {
            // No process selected — hide the detail pane so the list takes
            // the full width.
            dp.pane.set_visible(false);
            dp.pid.set_label("PID: —");
            dp.name.set_label("Name: —");
            dp.uid.set_label("UID: —");
            dp.username.set_label("Username: —");
            dp.cpu.set_label("CPU%: —");
            dp.mem.set_label("MEM%: —");
            dp.disk_read.set_label("Disk read: —");
            dp.disk_write.set_label("Disk write: —");
            dp.status.set_label("");
        }
    }
}

fn sync_settings_widgets(
    state: &Rc<RefCell<State>>,
    check: &gtk4::CheckButton,
    list: &gtk4::ListBox,
    rows: &[gtk4::ListBoxRow],
) {
    let s = state.borrow();
    if check.is_active() != s.settings.show_all {
        check.set_active(s.settings.show_all);
    }
    let idx = RefreshInterval::ALL
        .iter()
        .position(|i| *i == s.settings.refresh)
        .unwrap_or(1);
    let already = list.selected_row().is_some_and(|r| r == rows[idx]);
    if !already {
        list.select_row(Some(&rows[idx]));
    }
}

fn load_css() {
    let provider = gtk4::CssProvider::new();
    let data = glib::Bytes::from_static(CSS.as_bytes());
    provider.load_from_bytes(&data);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pid: i32) -> ProcessItem {
        crate::testutil::test_item(pid)
    }

    fn temp_settings_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "simpletaskmgr-ui-test-{}-{}-settings.toml",
            std::process::id(),
            tag
        ))
    }

    /// A `State` whose settings round-trip to a throwaway path, so tests
    /// never read or write the real `~/.config/simpletaskmgr` file.
    fn test_state(tag: &str) -> State {
        let path = temp_settings_path(tag);
        let _ = std::fs::remove_file(&path);
        State::with_settings_path(path)
    }

    #[test]
    fn test_state_new_primes_metrics() {
        let s = test_state("metrics");
        assert!(!s.metrics.history().is_empty());
        assert!(s.selected_pid.is_none());
    }

    #[test]
    fn test_kill_no_selection() {
        let mut s = test_state("kill_none");
        assert!(matches!(s.kill(Signal::Sighup), KillStatus::NoSelection));
    }

    /// A signal to a pid that will not resolve to a live process reports
    /// `Failed` (the syscall returns ESRCH/EPERM) — never a panic and never a
    /// false `Sent`.
    #[test]
    fn test_kill_unknown_pid_reports_failure() {
        let mut s = test_state("kill_unknown");
        // A pid far beyond typical allocations that is not going to be live.
        s.selected_pid = Some(2_147_483_647);
        let status = s.kill(Signal::Sighup);
        assert!(
            matches!(status, KillStatus::Failed(_)),
            "signalling a dead pid must report Failed, got {status:?}"
        );
    }

    #[test]
    fn test_find_returns_item() {
        let mut s = test_state("find");
        s.process_list.processes.push(item(123));
        assert_eq!(s.find(123).unwrap().value.name, "name123");
        assert!(s.find(999).is_none());
    }

    /// refresh() appends exactly one sample to the rolling metrics history each
    /// call — this is what keeps the usage graph advancing one tick per refresh.
    #[test]
    fn test_refresh_appends_one_metric_sample() {
        let mut s = test_state("refresh_sample");
        let before = s.metrics.history().len();
        s.refresh();
        let after = s.metrics.history().len();
        assert_eq!(after, before + 1, "one refresh appends exactly one sample");
    }

    /// refresh() preserves the selected pid the caller used; it only
    /// re-derives the process list and advances the metric history. (Sort
    /// state is no longer tracked in `State` — GTK's `SortListModel` owns it,
    /// and the `CompareValues` comparator backs it.)
    #[test]
    fn test_refresh_preserves_selection() {
        let mut s = test_state("refresh_preserve");
        s.selected_pid = Some(1);

        s.refresh();

        assert_eq!(s.selected_pid, Some(1));
    }

    #[test]
    fn test_kill_sigkill_on_sleep_child() {
        let mut s = test_state("kill_kill");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;
        s.selected_pid = Some(pid);
        match s.kill(Signal::Sigkill) {
            KillStatus::Sent => {}
            other => panic!("expected Sent, got {other:?}"),
        }
        let _ = child.wait();
    }

    #[test]
    fn test_detail_status_strings() {
        assert_eq!(detail_status(&KillStatus::Sent), "Sent.");
        assert_eq!(
            detail_status(&KillStatus::NoSelection),
            "No process selected."
        );
        assert_eq!(detail_status(&KillStatus::Failed("boom".into())), "boom");
    }

    /// Encoding the #4 contract: once a selected process is killed and a refresh
    /// runs (which the SIGKILL button now triggers immediately), the process is
    /// gone from the visible list. Without the forced refresh, it would linger
    /// up to 1.5s (the timer interval).
    #[test]
    fn test_refresh_drops_killed_process() {
        let mut s = test_state("kill_drops");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;

        // Simulate the list already containing the process (as a prior
        // `refresh_process_list` pass would have left it).
        s.process_list.processes.push(item(pid));
        s.selected_pid = Some(pid);

        // Send SIGKILL — the very call the button makes.
        match s.kill(Signal::Sigkill) {
            KillStatus::Sent => {}
            other => panic!("expected Sent, got {other:?}"),
        }

        // Give the kernel a beat to drop the /proc entry, then reap.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = child.wait();

        // A forced refresh (which the button now does immediately) must drop it.
        s.refresh();
        assert!(
            s.process_list.processes.iter().all(|p| p.pid != pid),
            "killed process should be dropped on the refresh the button triggers"
        );
    }

    #[test]
    fn test_state_initializes_from_saved_settings() {
        let path = temp_settings_path("load_all");
        UserSettings {
            show_all: true,
            refresh: RefreshInterval::Slow,
        }
        .save(&path)
        .unwrap();
        let s = State::with_settings_path(path);
        let _ = std::fs::remove_file(temp_settings_path("load_all"));
        assert!(s.settings.show_all);
        assert_eq!(s.settings.refresh, RefreshInterval::Slow);
        assert!(
            s.process_list.show_all,
            "show_all setting must seed the process list on startup"
        );
    }

    #[test]
    fn test_set_show_all_persists() {
        let mut s = test_state("set_all");
        s.set_show_all(true);
        assert!(s.settings.show_all);
        assert!(s.process_list.show_all);
        let reloaded = UserSettings::load(&s.save_path);
        assert!(
            reloaded.show_all,
            "changing the toggle must persist immediately"
        );
        let _ = std::fs::remove_file(temp_settings_path("set_all"));
    }

    #[test]
    fn test_set_refresh_interval_persists() {
        let mut s = test_state("set_refresh");
        assert!(
            s.set_refresh_interval(RefreshInterval::Fast),
            "a different interval must report a change"
        );
        assert_eq!(s.settings.refresh, RefreshInterval::Fast);
        let reloaded = UserSettings::load(&s.save_path);
        assert_eq!(
            reloaded.refresh,
            RefreshInterval::Fast,
            "changing the interval must persist immediately"
        );
        let _ = std::fs::remove_file(temp_settings_path("set_refresh"));
    }

    #[test]
    fn test_set_refresh_interval_noop_when_unchanged() {
        let mut s = test_state("refresh_noop");
        assert!(
            !s.set_refresh_interval(RefreshInterval::default()),
            "setting the current interval must report no change"
        );
        assert_eq!(s.settings.refresh, RefreshInterval::Normal);
        let _ = std::fs::remove_file(temp_settings_path("refresh_noop"));
    }

    /// The timer id slot must round-trip: a caller can park a running
    /// `SourceId` and take it back out when swapping sources. This is the
    /// plumbing `restart_timer` relies on to avoid leaking a stale polling
    /// source. We never let the test timer fire (30s) since it would try to
    /// refresh and repaint widgets that only exist inside `build_window`.
    #[test]
    fn test_timer_id_slot_round_trip() {
        let s = test_state("timer_slot");
        assert!(s.take_timer_id().is_none(), "no timer at construction");
        let id = glib::timeout_add_local(std::time::Duration::from_secs(30), || {
            glib::ControlFlow::Break
        });
        s.timer_id.set(Some(id));
        let taken = s.take_timer_id();
        assert!(s.take_timer_id().is_none(), "take must clear the slot");
        let taken = taken.expect("stored id must come back out");
        taken.remove();
        let _ = std::fs::remove_file(temp_settings_path("timer_slot"));
    }

    #[test]
    fn test_reset_settings_restores_defaults_and_persists() {
        let mut s = test_state("reset");
        s.set_show_all(true);
        s.set_refresh_interval(RefreshInterval::Slow);
        assert!(
            s.reset_settings(),
            "reset must report an interval change after the user picked Slow"
        );
        assert!(!s.settings.show_all);
        assert_eq!(s.settings.refresh, RefreshInterval::Normal);
        assert!(
            !s.process_list.show_all,
            "reset must also re-apply the list filter"
        );
        let reloaded = UserSettings::load(&s.save_path);
        assert!(
            !reloaded.show_all && reloaded.refresh == RefreshInterval::Normal,
            "reset must persist defaults"
        );
        let _ = std::fs::remove_file(temp_settings_path("reset"));
    }
}
