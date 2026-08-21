use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;

use crate::config::Config;
use crate::metrics::SystemMetrics;
use crate::process::{ProcessItem, TaskMgrProcess};
use crate::process_list::ProcessList;
use crate::signal::Signal;
use crate::usage_graph::paint_usage_chart;
use crate::{SortColumn, SortDirection};

const CSS: &str = include_str!("ui.css");

struct State {
    process_list: ProcessList,
    sort_column: SortColumn,
    sort_direction: SortDirection,
    metrics: SystemMetrics,
    selected_pid: Option<i32>,
    rows: Vec<(i32, gtk4::ListBoxRow)>,
}

#[derive(Debug)]
enum KillStatus {
    Sent,
    NoSelection,
    Failed(String),
}

impl State {
    fn new() -> Self {
        let process_list = ProcessList::init();
        let mut metrics = SystemMetrics::new();
        metrics.push_sample();
        Self {
            process_list,
            sort_column: SortColumn::CpuPercent,
            sort_direction: SortDirection::Descending,
            metrics,
            selected_pid: None,
            rows: Vec::new(),
        }
    }

    fn refresh(&mut self) {
        let (col, dir) = (self.sort_column, self.sort_direction);
        self.process_list.update_process_list();
        self.process_list.sort_processes(col, dir);
        self.metrics.push_sample();
        self.rows.clear();
    }

    fn on_sort_click(&mut self, col: SortColumn) {
        let next = if self.sort_column == col {
            match self.sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            }
        } else {
            SortDirection::Ascending
        };
        self.sort_column = col;
        self.sort_direction = next;
        self.process_list.sort_processes(col, next);
    }

    fn find(&self, pid: i32) -> Option<&ProcessItem> {
        self.process_list.processes.iter().find(|p| p.pid == pid)
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

    // ---- Graph ---------------------------------------------------------------
    let graph_area = gtk4::DrawingArea::new();
    graph_area.set_content_height(110);
    graph_area.set_hexpand(true);
    graph_area.add_css_class("graph-area");
    let st_graph = state.clone();
    graph_area.set_draw_func(move |_da, cr: &gtk4::cairo::Context, w: i32, h: i32| {
        let samples = st_graph.borrow().metrics.history();
        paint_usage_chart(cr, w as f64, h as f64, &samples);
    });

    // ---- List header (4 sortable columns) ------------------------------------
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    header.add_css_class("list-header");

    // ---- Process list --------------------------------------------------------
    let list = gtk4::ListBox::new();
    list.add_css_class("process-list");
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.set_activate_on_single_click(true);
    let list_scroll = gtk4::ScrolledWindow::new();
    list_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    list_scroll.set_child(Some(&list));
    list_scroll.set_hexpand(true);
    list_scroll.set_vexpand(true);

    // ---- Detail pane ---------------------------------------------------------
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
    detail_box.append(&d_pid);
    detail_box.append(&d_name);
    detail_box.append(&d_uid);
    detail_box.append(&d_user);
    detail_box.append(&d_cpu);

    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.add_css_class("detail-buttons");
    let b_sighup = gtk4::Button::new();
    b_sighup.set_label("Send SIGHUP");
    b_sighup.add_css_class("signal-btn");
    let b_sigkill = gtk4::Button::new();
    b_sigkill.set_label("Send SIGKILL");
    b_sigkill.add_css_class("signal-btn-danger");
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

    // ---- Body row (list | detail) ---------------------------------------------
    let body = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    body.add_css_class("body");
    body.append(&list_scroll);
    body.append(&detail_scroll);

    // ---- Assemble root ---------------------------------------------------------
    root.append(&graph_area);
    root.append(&header);
    root.append(&body);

    // ---- Toolbar: "show all processes" toggle -----------------------------------
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    toolbar.add_css_class("toolbar");
    let toggle = gtk4::CheckButton::new();
    toggle.set_label(Some("Show all processes"));
    toggle.add_css_class("show-all-toggle");
    toggle.set_active(false);
    toolbar.append(&toggle);
    root.insert_child_after(&toolbar, Some(&header));

    // ---- Header buttons (created after list is available) ----------------------
    let columns: &[(SortColumn, &str)] = &[
        (SortColumn::Pid, "PID"),
        (SortColumn::Username, "User"),
        (SortColumn::Name, "Name"),
        (SortColumn::CpuPercent, "CPU%"),
    ];
    let mut header_buttons: Vec<(SortColumn, gtk4::Button)> = Vec::new();
    for (col, label_text) in columns.iter() {
        let b = gtk4::Button::new();
        b.add_css_class("list-header-cell");
        b.set_hexpand(true);
        let lbl = gtk4::Label::new(Some(label_text));
        lbl.add_css_class("list-header-title");
        b.set_child(Some(&lbl));
        header.append(&b);
        header_buttons.push((*col, b));
    }

    // ---- Shared closure: rebuild the list from state ----------------------------
    let list_r = list.clone();
    let state_r = state.clone();
    let dp_labels = Rc::new((
        d_pid.clone(),
        d_name.clone(),
        d_uid.clone(),
        d_user.clone(),
        d_cpu.clone(),
        d_status.clone(),
    ));

    let dp_r = dp_labels.clone();
    let rebuild: Rc<dyn Fn()> = Rc::new(move || {
        remove_all_rows(&state_r, &list_r);
        let items = {
            let mut s = state_r.borrow_mut();
            s.rows.clear();
            s.process_list.processes.clone()
        };
        for item in &items {
            let row = make_row(item);
            let pid = item.pid;
            let row_copy = row.clone();
            list_r.append(&row_copy);
            state_r.borrow_mut().rows.push((pid, row));
        }
        let selected = state_r.borrow().selected_pid;
        if let Some(pid) = selected {
            if let Some((_, row)) = state_r.borrow().rows.iter().find(|(p, _)| *p == pid) {
                list_r.select_row(Some(row));
            }
        }
        apply_detail(&dp_r, &state_r, selected);
    });

    // ---- Header button handlers -------------------------------------------------
    for (col, b) in header_buttons.iter() {
        let col = *col;
        let state_h = state.clone();
        let rebuild_h = rebuild.clone();
        let header_all = header_buttons.clone();
        b.connect_clicked(move |_| {
            state_h.borrow_mut().on_sort_click(col);
            rebuild_h();
            update_header_indicators(&header_all, &state_h.borrow());
        });
    }

    // ---- "Show all" toggle handler ----------------------------------------------
    {
        let state_t = state.clone();
        let rebuild_t = rebuild.clone();
        toggle.connect_toggled(move |chk| {
            let show_all = chk.is_active();
            state_t.borrow_mut().process_list.set_show_all(show_all);
            // Refresh now so the new filter takes effect immediately rather than
            // waiting for the next 1.5s tick.
            state_t.borrow_mut().refresh();
            rebuild_t();
        });
    }

    // ---- Row selected handler ----------------------------------------------------
    {
        let state_s = state.clone();
        let dp_s = dp_labels.clone();
        list.connect_row_selected(move |_lb, row| {
            let pid = row.as_ref().and_then(|r| {
                state_s
                    .borrow()
                    .rows
                    .iter()
                    .find(|(_p, w)| w.as_ptr() == r.as_ptr())
                    .map(|(p, _)| *p)
            });
            state_s.borrow_mut().selected_pid = pid;
            apply_detail(&dp_s, &state_s, pid);
        });
    }

    // ---- Signal buttons -----------------------------------------------------------
    {
        let state_k = state.clone();
        let dp_k = dp_labels.clone();
        b_sighup.connect_clicked(move |_| {
            let status = state_k.borrow_mut().kill(Signal::Sighup);
            dp_k.5.set_label(&detail_status(&status));
        });
        let state_k = state.clone();
        let dp_k = dp_labels.clone();
        let rebuild_k = rebuild.clone();
        b_sigkill.connect_clicked(move |_| {
            let status = state_k.borrow_mut().kill(Signal::Sigkill);
            dp_k.5.set_label(&detail_status(&status));
            // A killed process will disappear on the next refresh; force one now
            // so the row is removed immediately instead of waiting up to 1.5s.
            state_k.borrow_mut().refresh();
            rebuild_k();
        });
    }

    // ---- Initial paint -----------------------------------------------------------
    rebuild();
    update_header_indicators(&header_buttons, &state.borrow());

    // ---- Refresh timer ------------------------------------------------------------
    {
        let state_t = state.clone();
        let list_t = list.clone();
        let dp_t = dp_labels.clone();
        let graph_t = graph_area.clone();
        glib::timeout_add_local(Config::refresh_interval(), move || {
            state_t.borrow_mut().refresh();
            rebuild_for_timer(&state_t, &list_t, &dp_t);
            graph_t.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    window
}

fn rebuild_for_timer(
    state: &Rc<RefCell<State>>,
    list: &gtk4::ListBox,
    dp: &Rc<(
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
    )>,
) {
    remove_all_rows(state, list);
    let items = {
        let mut s = state.borrow_mut();
        s.rows.clear();
        s.process_list.processes.clone()
    };
    for item in &items {
        let row = make_row(item);
        let pid = item.pid;
        let row_copy = row.clone();
        list.append(&row_copy);
        state.borrow_mut().rows.push((pid, row));
    }
    let selected = state.borrow().selected_pid;
    if let Some(pid) = selected {
        if let Some((_, row)) = state.borrow().rows.iter().find(|(p, _)| *p == pid) {
            list.select_row(Some(row));
        } else {
            state.borrow_mut().selected_pid = None;
        }
    }
    apply_detail(dp, state, state.borrow().selected_pid);
}

/// Removes every row currently held in `state.rows` from `list`.
///
/// `GtkListBox::remove_all` requires GTK 4.12, which this project doesn't
/// target (v4_10). We instead iterate the tracked rows and call `remove` on
/// each.
fn remove_all_rows(state: &Rc<RefCell<State>>, list: &gtk4::ListBox) {
    let rows: Vec<gtk4::ListBoxRow> = {
        let s = state.borrow();
        s.rows.iter().map(|(_, r)| r.clone()).collect()
    };
    for row in rows.iter() {
        list.remove(row);
    }
}

fn apply_detail(
    dp: &Rc<(
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
        gtk4::Label,
    )>,
    state: &Rc<RefCell<State>>,
    pid: Option<i32>,
) {
    let item = pid.and_then(|p| {
        let s = state.borrow();
        s.find(p).cloned()
    });
    match item {
        Some(item) => {
            let p = &item.value;
            dp.0.set_label(&format!("PID: {}", p.pid));
            dp.1.set_label(&format!("Name: {}", p.name));
            dp.2.set_label(&format!("UID: {}", p.ruid));
            dp.3.set_label(&format!("Username: {}", p.username));
            dp.4.set_label(&format!("CPU%: {}", p.cpu_percent_str()));
            dp.5.set_label("");
        }
        None => {
            dp.0.set_label("PID: —");
            dp.1.set_label("Name: —");
            dp.2.set_label("UID: —");
            dp.3.set_label("Username: —");
            dp.4.set_label("CPU%: —");
        }
    }
}

fn make_row(item: &ProcessItem) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.add_css_class("process-row");

    let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    row_box.add_css_class("process-cell");

    let p: &TaskMgrProcess = &item.value;
    let cpu_str = p.cpu_percent_str();
    let tmp = [
        p.pid.to_string(),
        p.username.clone(),
        p.name.clone(),
        cpu_str.clone(),
    ];
    for (text, class) in tmp
        .iter()
        .zip(["col-pid", "col-user", "col-name", "col-cpu"])
    {
        let l = gtk4::Label::new(Some(text));
        l.add_css_class(class);
        l.set_hexpand(true);
        row_box.append(&l);
    }

    row.set_child(Some(&row_box));
    row
}

fn update_header_indicators(buttons: &[(SortColumn, gtk4::Button)], state: &State) {
    let (active, dir) = (state.sort_column, state.sort_direction);
    for (col, b) in buttons.iter() {
        if *col == active {
            b.add_css_class("sort-active");
            match dir {
                SortDirection::Ascending => {
                    b.remove_css_class("sort-desc");
                    b.add_css_class("sort-asc");
                }
                SortDirection::Descending => {
                    b.remove_css_class("sort-asc");
                    b.add_css_class("sort-desc");
                }
            }
        } else {
            b.remove_css_class("sort-active");
            b.remove_css_class("sort-asc");
            b.remove_css_class("sort-desc");
        }
    }
}

fn load_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(CSS);
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
        ProcessItem::new(&TaskMgrProcess::new(
            format!("name{pid}"),
            pid,
            1000,
            "paul".to_string(),
            1.0,
        ))
    }

    #[test]
    fn test_state_new_primes_metrics() {
        let s = State::new();
        assert!(!s.metrics.history().is_empty());
        assert_eq!(s.sort_column, SortColumn::CpuPercent);
        assert_eq!(s.sort_direction, SortDirection::Descending);
        assert!(s.selected_pid.is_none());
    }

    #[test]
    fn test_state_on_sort_click_toggles() {
        let mut s = State::new();
        let d0 = s.sort_direction;
        s.on_sort_click(SortColumn::CpuPercent);
        assert_ne!(s.sort_direction, d0);
        s.on_sort_click(SortColumn::CpuPercent);
        assert_eq!(s.sort_direction, d0);
    }

    #[test]
    fn test_kill_no_selection() {
        let mut s = State::new();
        assert!(matches!(s.kill(Signal::Sighup), KillStatus::NoSelection));
    }

    /// A signal to a pid that will not resolve to a live process reports
    /// `Failed` (the syscall returns ESRCH/EPERM) — never a panic and never a
    /// false `Sent`.
    #[test]
    fn test_kill_unknown_pid_reports_failure() {
        let mut s = State::new();
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
        let mut s = State::new();
        s.process_list.processes.push(item(123));
        assert_eq!(s.find(123).unwrap().value.name, "name123");
        assert!(s.find(999).is_none());
    }

    /// refresh() appends exactly one sample to the rolling metrics history each
    /// call — this is what keeps the usage graph advancing one tick per refresh.
    #[test]
    fn test_refresh_appends_one_metric_sample() {
        let mut s = State::new();
        let before = s.metrics.history().len();
        s.refresh();
        let after = s.metrics.history().len();
        assert_eq!(after, before + 1, "one refresh appends exactly one sample");
    }

    /// refresh() clears the tracked-rows Vec (the UI republishes it on the next
    /// rebuild) but preserves the sort state and selected pid the caller used.
    #[test]
    fn test_refresh_preserves_sort_and_selection() {
        let mut s = State::new();
        s.selected_pid = Some(1);
        s.sort_column = SortColumn::Name;
        s.sort_direction = SortDirection::Ascending;

        s.refresh();

        // Sort and selection are preserved; refresh only re-derived processes.
        assert_eq!(s.sort_column, SortColumn::Name);
        assert_eq!(s.sort_direction, SortDirection::Ascending);
        assert_eq!(s.selected_pid, Some(1));
        // The rows Vec is republished by the UI after refresh, so it's cleared.
        assert!(s.rows.is_empty());
    }

    /// Clicking a *different* column resets the direction back to the default
    /// for that column (ASCENDING), matching the header-indicator code.
    #[test]
    fn test_on_sort_click_new_column_starts_ascending() {
        let mut s = State::new();
        s.sort_column = SortColumn::CpuPercent;
        s.sort_direction = SortDirection::Descending;
        s.on_sort_click(SortColumn::Name);
        assert_eq!(s.sort_column, SortColumn::Name);
        assert_eq!(s.sort_direction, SortDirection::Ascending);
    }

    #[test]
    fn test_kill_sigkill_on_sleep_child() {
        let mut s = State::new();
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
        let mut s = State::new();
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
}
