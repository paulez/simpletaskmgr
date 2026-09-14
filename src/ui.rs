use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::config::RefreshInterval;
use crate::metrics::SystemMetrics;
use crate::process::ProcessItem;
use crate::process_list::ProcessList;
use crate::process_view::ProcessView;
use crate::settings::{settings_path, UserSettings};
use crate::signal::Signal;
use crate::usage_graph::{paint_usage_chart, ChartConfig, ChartPane};

const CSS: &str = include_str!("ui.css");

/// The widgets the `Settings` popover exposes so its change handlers and the
/// reset path can address them without name lookup.
struct SettingsWidgets {
    button: gtk4::MenuButton,
    check: gtk4::CheckButton,
    list: gtk4::ListBox,
    rows: Vec<gtk4::ListBoxRow>,
    reset: gtk4::Button,
}

pub struct State {
    pub process_list: ProcessList,
    pub metrics: SystemMetrics,
    pub selected_pid: Option<i32>,
    pub settings: UserSettings,
    pub save_path: std::path::PathBuf,
    /// Total number of [`State::refresh`] calls this state has performed.
    /// Cadence diagnostic: a correctly wired tick performs **exactly one**
    /// refresh — a second per tick shrinks the CPU-delta window to the
    /// first refresh's own duration, inflating every reported %CPU (the
    /// cadence test in `tests/process_view_gtk.rs` pins this down).
    pub refresh_count: u64,
    pub timer_id: Cell<Option<glib::SourceId>>,
    /// Top of the frequency-axis domain in MHz, read once at launch from
    /// `scaling_max_freq`. The fallback (4.0 GHz) covers hosts without a
    /// `cpufreq` interface (e.g. VMs); in that case the series is absent
    /// anyway and the domain is unused.
    pub freq_max_mhz: f64,
    /// Top of the memory-axis domain in MB — the installed RAM, read once at
    /// launch from `/proc/meminfo` `MemTotal`. The fallback (16 GB) covers
    /// hosts where `/proc/meminfo` is unavailable.
    pub mem_max_mb: f64,
    /// `true` when a GPU is present and readable via `rocm-smi` (probed once
    /// at startup). When `false`, the UI hides its tab entirely and the
    /// per-tick sampler skips the `rocm-smi` spawn, and `push_sample`
    /// receives `None` to carry over the (never-set) GPU fields.
    pub gpu_available: bool,
}

#[derive(Debug)]
pub enum KillStatus {
    /// The signal was delivered to the selected process.
    Sent,
    /// No process was selected.
    NoSelection,
    /// `kill(2)` refused the delivery.
    Failed {
        /// The signal that was requested.
        signal: Signal,
        /// The pid the signal was aimed at.
        pid: i32,
        /// The OS error from `kill(2)` (e.g. EPERM for a process owned by
        /// another user, ESRCH if it already left).
        cause: std::io::Error,
    },
}

impl State {
    /// Constructs state at the conventional settings location.
    pub fn new() -> Self {
        Self::with_settings_path(settings_path())
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    /// Constructs state that loads and persists settings at `path`.
    /// Production code uses the conventional location via [`State::new`];
    /// tests point this at a temporary path so they never touch the real
    /// configuration file.
    ///
    /// On a GPU host this probes `rocm-smi` once and samples it; on a
    /// non-GPU host the probe degrades to no spawn. The probe is the only
    /// unit-test cost of `State` that involves an external binary, and the
    /// `/proc/[pid]` walk in `ProcessList::init()` is the one expensive live
    /// I/O call — both are exercised by integration tests, not unit tests
    /// (see `tests/integration_tests.rs`).
    pub fn with_settings_path(path: std::path::PathBuf) -> Self {
        let settings = UserSettings::load(&path);
        let mut process_list = ProcessList::init();
        process_list.set_show_all(settings.show_all);
        let gpu_available = crate::gpu_status::gpu_available();
        let gpu = gpu_available
            .then(crate::gpu_status::read_gpu_card)
            .flatten();
        let mut metrics = SystemMetrics::new();
        // Initial sample: establish the /proc/stat baseline the same way the
        // CPU path does today; pass the probe result so the first
        // `Sample`'s GPU fields are populated if a GPU is present.
        metrics.push_sample(gpu);
        Self {
            process_list,
            metrics,
            selected_pid: None,
            settings,
            save_path: path,
            timer_id: Cell::new(None),
            freq_max_mhz: crate::cpu_status::read_max_freq_mhz().unwrap_or(4000.0),
            mem_max_mb: crate::metrics::read_mem_total_mb().unwrap_or(16.0 * 1024.0),
            gpu_available,
            refresh_count: 0,
        }
    }

    /// Test-safe constructor: builds a `State` around a **fixed** process
    /// list (fixture data) so **no** `rocm-smi` spawn occurs **and no live
    /// `/proc/[pid]` walk** happens at construction time — `processes` is
    /// populated directly from `items`, not from the live filesystem.
    ///
    /// `metrics` is created empty (no initial `push_sample`) so the caller
    /// decides exactly which samples to push — this is what lets unit tests
    /// assert on `metrics.history()` without a live `/proc/stat` or
    /// `/proc/meminfo` read.
    ///
    /// No `refresh` is triggered, so no CPU/freq/disk live reads happen
    /// either. Only the `UserSettings` load from disk (a single small file,
    /// already tested separately in `settings.rs`) and the one-shot
    /// `/sys` freq + `/proc/meminfo` "domain max" reads remain; both degrade
    /// gracefully on any host and do not walk the process table.
    pub fn with_processes(path: std::path::PathBuf, items: Vec<ProcessItem>) -> Self {
        let settings = UserSettings::load(&path);
        let mut process_list = ProcessList::new();
        process_list.set_show_all(settings.show_all);
        process_list.processes = items;
        let metrics = SystemMetrics::new();
        Self {
            process_list,
            metrics,
            selected_pid: None,
            settings,
            save_path: path,
            timer_id: Cell::new(None),
            freq_max_mhz: crate::cpu_status::read_max_freq_mhz().unwrap_or(4000.0),
            mem_max_mb: crate::metrics::read_mem_total_mb().unwrap_or(16.0 * 1024.0),
            gpu_available: false,
            refresh_count: 0,
        }
    }

    /// Returns the id of the running refresh timer, if any, and clears the
    /// slot. `build_window` fills the slot; the timer-callback and
    /// interval-change paths take it out when they swap sources.
    pub fn take_timer_id(&self) -> Option<glib::SourceId> {
        self.timer_id.take()
    }

    /// Runs one refresh tick: re-reads the process list from `/proc`, samples
    /// the GPU (if `gpu_available`), and pushes one CPU/mem/freq/temp/disk
    /// sample onto `metrics`. This is the live-I/O entry point the unit
    /// tests must avoid — the integration tests (which run in a single
    /// process outside the parallel test pool) exercise it instead.
    pub fn refresh(&mut self) {
        self.refresh_count += 1;
        self.process_list.update_process_list();
        // Per-tick GPU read. On non-GPU hosts (rocm-smi absent or no card0)
        // the startup probe already determined `gpu_available == false`, so
        // we skip the spawn entirely rather than burning a fork/exec we know
        // will fail. On GPU hosts we spawn and pass the result; `push_sample`
        // carries the previous reading forward when the spawn fails, so a
        // momentary `rocm-smi` hiccup doesn't blank the series (same rule as
        // `freq`/`temp`).
        let gpu = self
            .gpu_available
            .then(crate::gpu_status::read_gpu_card)
            .flatten();
        self.metrics.push_sample(gpu);
    }

    /// Sets the show-all filter and persists it. Caller is responsible for
    /// refreshing + republishing so the change takes effect immediately.
    pub fn set_show_all(&mut self, show_all: bool) {
        if self.settings.show_all != show_all {
            self.settings.show_all = show_all;
            self.save_settings();
        }
        self.process_list.set_show_all(show_all);
    }

    /// Sets the refresh-interval preset and persists it. The caller restarts
    /// the running timer (see `take_timer_id`) so the new period applies
    /// immediately. Returns whether the value actually changed.
    pub fn set_refresh_interval(&mut self, interval: RefreshInterval) -> bool {
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
    pub fn reset_settings(&mut self) -> bool {
        let defaults = UserSettings::default();
        self.set_show_all(defaults.show_all);
        self.set_refresh_interval(defaults.refresh)
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.save_path) {
            log::error!("Failed to save settings: {e:?}");
        }
    }

    /// Sends `sig` to `selected_pid`. Integration tests exercise the full
    /// kill + refresh path; unit tests assert only the pure-control-flow
    /// branch (no live I/O).
    pub fn kill(&mut self, sig: Signal) -> KillStatus {
        match self.selected_pid {
            Some(pid) => match crate::signal::send_signal(pid, sig) {
                Ok(()) => KillStatus::Sent,
                Err(e) => KillStatus::Failed {
                    signal: sig,
                    pid,
                    // `send_signal` wraps the `kill(2)` OS error in an
                    // anyhow context; recover the root cause so the UI can
                    // distinguish EPERM / ESRCH / anything else.
                    cause: e
                        .root_cause()
                        .downcast_ref::<std::io::Error>()
                        .map(|io| match io.raw_os_error() {
                            Some(code) => std::io::Error::from_raw_os_error(code),
                            None => std::io::Error::other(io.to_string()),
                        })
                        .unwrap_or_else(|| std::io::Error::other(e.root_cause().to_string())),
                },
            },
            None => KillStatus::NoSelection,
        }
    }
}

/// The error pop-up's title and detail for a failed signal delivery.
/// `kill(2)`'s specific errors get a plain-language line; the user's
/// real-world case (signalling a process owned by another user, EPERM)
/// is spelled out.
pub fn describe_failure(sig: Signal, pid: i32, cause: &std::io::Error) -> (String, String) {
    let name = sig.name();
    match cause.raw_os_error() {
        Some(code) if code == libc::EPERM => (
            format!("No permission to send {name} to pid {pid}"),
            format!("The process is owned by another user, so the signal was rejected.\n\n{cause}"),
        ),
        Some(code) if code == libc::ESRCH => (
            format!("Pid {pid} is no longer running"),
            format!("{name} could not be delivered: {cause}"),
        ),
        _ => (
            format!("Could not send {name} to pid {pid}"),
            cause.to_string(),
        ),
    }
}

/// Show the modal error pop-up for a failed signal delivery, parented to
/// the main window.
///
/// Built from core GTK widgets rather than the `Dialog`/`MessageDialog`
/// family: GTK deprecated that family (4.10) and its replacement
/// `gtk4::AlertDialog` exposes no synchronous response hook in our gtk-rs
/// binding, so a hand-rolled transient, modal window is the simplest
/// deprecation-free equivalent. The OK button holds only a weak reference
/// to the window to avoid a signal-handler reference cycle.
pub fn show_failure_popup(window: &impl IsA<gtk4::Window>, title: &str, detail: &str) {
    let dialog = gtk4::Window::builder()
        .transient_for(window)
        .modal(true)
        .resizable(false)
        .default_width(420)
        .title(title)
        .build();

    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    body.set_margin_top(24);
    body.set_margin_end(24);
    body.set_margin_bottom(12);
    body.set_margin_start(24);

    let icon = gtk4::Image::from_icon_name("dialog-error-symbolic");
    let detail_label = gtk4::Label::new(Some(detail));
    detail_label.set_wrap(true);
    detail_label.set_selectable(true);

    let ok = gtk4::Button::with_label("OK");
    // Keep the dialog alive past this function and destroy it on click.
    let held = RefCell::new(Some(dialog.clone()));
    ok.connect_clicked(move |_| {
        if let Some(dlg) = held.borrow_mut().take() {
            dlg.destroy();
        }
    });

    body.append(&icon);
    body.append(&detail_label);
    body.append(&ok);
    dialog.set_child(Some(&body));
    dialog.present();
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
    let id = glib::timeout_add_local(interval, move || {
        // One tick = exactly one `rebuild` (= one `State::refresh` + one
        // view update). A second `state.refresh()` here would shrink the
        // CPU-delta window of the tick to the first refresh's own duration,
        // inflating every reported %CPU (see the cadence test).
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
    // Every pane carries a colour+label legend band below the plot, so every
    // pane is `80 + LEGEND_PAD` tall. `80` keeps the plot visually the same
    // size as before; `LEGEND_PAD` is the band reserved for the swatch+label
    // row (see `usage_graph::paint_legend`).
    drawing.set_content_height(80 + crate::usage_graph::LEGEND_PAD as i32);
    drawing.set_hexpand(true);
    drawing.add_css_class("graph-area");
    let st = state.clone();
    let header = label.clone();
    drawing.set_draw_func(move |da, cr: &gtk4::cairo::Context, w: i32, h: i32| {
        let samples = st.borrow().metrics.history();
        let cfg = ChartConfig {
            freq_max_mhz: st.borrow().freq_max_mhz,
            mem_max_mb: st.borrow().mem_max_mb,
            // ~10 s to fill at the default 1.5 s refresh (7 samples), matching
            // `ChartConfig::default().fill`.
            fill: 7,
            capacity: SystemMetrics::MAX_HISTORY,
        };
        // Theme foreground (text) colour for the disk-legend device *names*,
        // so they read on both light and dark themes. The swatch carries each
        // disk's colour; the name itself stays in the theme's normal text
        // colour. Falls back to a mid-grey if the lookup is unavailable.
        // `style_context()` and `lookup_color` were deprecated in GTK 4.10 in
        // favour of `gtk::CSS::resolve`, but `resolve` only works on values
        // set by us, not on the theme's built-in `theme_fg_color` — so we
        // allow the deprecation here (it is the sanctioned way to read a
        // theme-defined colour) and pull the RGBA out with the `.red()`
        // accessor (not a field — the field accessors on `gdk::RGBA` were
        // removed in 0.11).
        #[allow(deprecated)]
        let text_rgba = da
            .style_context()
            .lookup_color("theme_fg_color")
            .map(|c| (c.red() as f64, c.green() as f64, c.blue() as f64))
            .unwrap_or((0.5, 0.5, 0.5));
        // Keep the pane header in sync with the newest sample so the current
        // values are always readable even when a low/flat line is hard to see.
        if let Some(last) = samples.last() {
            let text = match pane {
                ChartPane::CpuMem => {
                    crate::usage_graph::cpu_mem_readout(last.cpu, last.mem, cfg.mem_max_mb)
                }
                ChartPane::FreqTemp => crate::usage_graph::freq_temp_readout(last.freq, last.temp),
                ChartPane::GpuUseVram => {
                    crate::usage_graph::gpu_use_vram_readout(last.gpu_use, last.gpu_vram)
                }
                ChartPane::GpuTemp => crate::usage_graph::gpu_temp_readout(last.gpu_temp),
                ChartPane::DiskThroughput => {
                    crate::usage_graph::disk_throughput_readout(&last.disks)
                }
                ChartPane::DiskUtil => crate::usage_graph::disk_util_readout(&last.disks),
            };
            header.set_text(&text);
        }
        paint_usage_chart(cr, w as f64, h as f64, &samples, &cfg, pane, text_rgba);
    });
    box_v.append(&drawing);

    (box_v, drawing)
}

/// Builds the CPU / Mem usage-graph row — a horizontal strip of two panes
/// (each pane: a centered title label above a [`gtk4::DrawingArea`]),
/// separated by a GTK default vertical `Separator`. The left pane draws the
/// CPU + memory utilization series (percent); the right pane draws the CPU
/// frequency + temperature series (MHz / °C), each on its own dedicated
/// axis. Both panes read the same rolling history and are redrawn in
/// lockstep on every refresh.
fn build_cpu_graph_row(state: &Rc<RefCell<State>>) -> (gtk4::Box, Vec<gtk4::DrawingArea>) {
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

/// Builds the GPU usage-graph row, following the same two-pane layout as
/// [`build_cpu_graph_row`] but for `rocm-smi` readings. The left pane draws
/// GPU utilization + VRAM (two percent series); the right pane draws the
/// GPU's edge temperature (single auto-scaled °C series). Both read the same
/// rolling history as the CPU tab and are redrawn in the same tick so the
/// data is always fresh when the user switches between the two tabs.
fn build_gpu_graph_row(state: &Rc<RefCell<State>>) -> (gtk4::Box, Vec<gtk4::DrawingArea>) {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    row.set_hexpand(true);

    let (pane_l, left) = build_graph_pane(state, "GPU Use & VRAM", ChartPane::GpuUseVram);
    row.append(&pane_l);

    let sep = gtk4::Separator::new(gtk4::Orientation::Vertical);
    row.append(&sep);

    let (pane_r, right) = build_graph_pane(state, "GPU Temp", ChartPane::GpuTemp);
    row.append(&pane_r);

    (row, vec![left, right])
}

/// Builds the Disk I/O usage-graph row, following the same two-pane layout as
/// [`build_cpu_graph_row`]. The left pane draws the physical disks' read +
/// write throughput (two auto-scaled bytes/s series); the right pane draws the
/// busiest disk's utilization (a single 0–100% series). Both read the same
/// rolling history as the other tabs and are redrawn in the same tick, so the
/// data is always fresh when the user switches tabs. Present on every host
/// (no GPU-style availability gate) — a host with no physical disk simply
/// paints a blank pane with a `"no disk"` header.
fn build_disk_graph_row(state: &Rc<RefCell<State>>) -> (gtk4::Box, Vec<gtk4::DrawingArea>) {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    row.set_hexpand(true);

    let (pane_l, left) = build_graph_pane(state, "Disk Throughput", ChartPane::DiskThroughput);
    row.append(&pane_l);

    let sep = gtk4::Separator::new(gtk4::Orientation::Vertical);
    row.append(&sep);

    let (pane_r, right) = build_graph_pane(state, "Disk Utilization", ChartPane::DiskUtil);
    row.append(&pane_r);

    (row, vec![left, right])
}

/// Builds the `Settings` button and its popover (show-all toggle, refresh
/// interval list, and reset button).
fn build_settings_popover() -> SettingsWidgets {
    let settings_btn = gtk4::MenuButton::new();
    settings_btn.set_icon_name("open-menu-symbolic");
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
    settings_btn.set_popover(Some(&popover));

    SettingsWidgets {
        button: settings_btn,
        check: show_all_check,
        list: refresh_list,
        rows: refresh_rows,
        reset: reset_btn,
    }
}

/// Builds the main window and wires the refresh timer around `state`.
/// Call from the `activate` handler (main loop thread only). The `state`
/// parameter is the app's single source of truth (created in `main`) so
/// tests can inspect it — e.g. assert the one-refresh-per-tick cadence via
/// [`State::refresh_count`].
pub fn build_window(
    app: &gtk4::Application,
    state: &Rc<RefCell<State>>,
) -> gtk4::ApplicationWindow {
    let window = gtk4::ApplicationWindow::new(app);
    window.set_title(Some("Simple Task Manager"));
    window.set_default_size(940, 600);
    window.add_css_class("app-root");

    load_css();

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    window.set_child(Some(&root));

    // A `Stack` holds every graph row (CPU / Mem, Disk I/O, and — when a GPU
    // is present — GPU) and a `StackSwitcher` above it is the tab bar the
    // user clicks to pick which one to show. The stack always starts on the
    // CPU / Mem row; we deliberately don't persist the user's choice, so a
    // restart always lands there. The GPU tab is added only when a GPU was
    // actually detected (see `gpu_available`); the other two are present on
    // every host.
    let (cpu_row, mut graph_areas) = build_cpu_graph_row(state);
    let (disk_row, disk_areas) = build_disk_graph_row(state);
    graph_areas.extend(disk_areas);
    let gpu_available = state.borrow().gpu_available;

    let stack = gtk4::Stack::new();
    let switcher = gtk4::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    stack.add_titled(&cpu_row, Some("cpu"), "CPU / Mem");
    stack.add_titled(&disk_row, Some("disk"), "Disk I/O");
    stack.set_visible_child_name("cpu");
    if gpu_available {
        let (gpu_row, gpu_areas) = build_gpu_graph_row(state);
        graph_areas.extend(gpu_areas);
        stack.add_titled(&gpu_row, Some("gpu"), "GPU");
    }
    root.append(&switcher);
    root.append(&stack);
    // ---- Body: the process list + detail pane, one self-contained widget ----
    // Sorting, selection, row recycling, and the detail pane are all owned by
    // the view (natively in GTK); the app drives it with one `update` per tick.
    let view = Rc::new(ProcessView::new());
    root.append(view.widget());

    let settings = build_settings_popover();

    // ---- Header bar (titlebar) with the trailing Settings control ---------------
    let header_bar = gtk4::HeaderBar::new();
    header_bar.pack_end(&settings.button);
    window.set_titlebar(Some(&header_bar));

    // ---- Refresh closure: one tick = state refresh + one view update -----------
    let rebuild: Rc<dyn Fn()> = {
        let state_u = state.clone();
        let view_u = view.clone();
        Rc::new(move || {
            state_u.borrow_mut().refresh();
            // Clone the snapshot out *before* the GTK call and drop the borrow
            // first: `update` may fire the selection callback during its model
            // commit, and the callback below borrows `state` (spec B2).
            let procs = state_u.borrow().process_list.processes.clone();
            view_u.update(&procs);
        })
    };

    // ---- Selection -> app state -------------------------------------------------
    // The view owns the detail pane; the app keeps the selected PID (spec D4)
    // for the signal path.
    {
        let state_s = state.clone();
        view.connect_selection_changed(move |pid| {
            state_s.borrow_mut().selected_pid = pid;
        });
    }

    // ---- Signal buttons -> State::kill (spec D5) --------------------------------
    // The detail pane forwards its button clicks with the requested signal;
    // the app sends it to `selected_pid` and reports the outcome in the
    // pane's status line. On success, run one refresh so the doomed process
    // leaves the list without waiting for the next tick.
    {
        let state_s = state.clone();
        let view_s = view.clone();
        let view_in = view_s.clone(); // the closure's own handle (receiver borrows view_s)
        let rebuild_s = rebuild.clone();
        let window_in = window.clone();
        view_s.connect_signal_requested(move |sig| {
            let status = state_s.borrow_mut().kill(sig);
            let pid = state_s.borrow().selected_pid;
            match (pid, status) {
                (_, KillStatus::Sent) => {
                    if let Some(p) = pid {
                        view_in.set_status(&format!("{} sent to pid {p}", sig.name()));
                    }
                    rebuild_s();
                }
                // The buttons are visible only while a process is selected;
                // still handle the impossible case gracefully instead of
                // assuming.
                (_, KillStatus::NoSelection) => {}
                // Delivery failed (EPERM on another user's process, ESRCH
                // when the process left in the meantime, …): report it in the
                // pane's status line *and* surface an error pop-up.
                (_, KillStatus::Failed { signal, pid, cause }) => {
                    view_in.set_status(&format!("{} failed: {cause}", signal.name()));
                    let (title, detail) = describe_failure(signal, pid, &cause);
                    show_failure_popup(&window_in, &title, &detail);
                }
            }
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
            // Rebuild now (one refresh) so the filter change takes effect
            // immediately rather than waiting up to the next refresh tick.
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
                        // Apply immediately: one refresh + view update.
                        rebuild_c();
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
            rebuild_r();
        });
    }

    // ---- Initial paint -----------------------------------------------------------
    // Populate the list from the state we already hold, then start at the top
    // of the list: GTK's layout pass re-scrolls on the first draw (a raw
    // `adjustment.set_value(0)` would be clobbered by it), so route through
    // the view's own `scroll_to` at the "just got mapped" point instead.
    rebuild();
    let view_map = view.clone();
    let win = window.clone();
    win.connect_map(move |_| {
        view_map.scroll_to_top();
        // Headless render check: with this env var set, pre-select the first
        // row so `tools/test_ui_render.sh` screenshots capture the detail
        // pane (which is hidden until a row is selected).
        if std::env::var_os("SIMPLETASKMGR_RENDER_SELECT_FIRST").is_some() {
            view_map.selection().set_selected(0);
        }
    });

    // ---- Refresh timer ------------------------------------------------------------
    restart_timer(state, &rebuild, &graph_areas);

    window
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

    /// A `State` for a unit test: settings round-trip to a throwaway path
    /// (so tests never read or write the real `~/.config/simpletaskmgr`),
    /// the process list is **fixture data** (not a live `/proc` walk), and
    /// `metrics` starts empty (no live `/proc/stat` baseline read). This is
    /// the spawn-free, fd-light constructor, `State::with_processes` — the
    /// live-reading paths (`State::with_settings_path`, `refresh`) are
    /// exercised by the integration tests instead.
    fn test_state(tag: &str) -> State {
        let path = temp_settings_path(tag);
        let _ = std::fs::remove_file(&path);
        // A representative row so `find` / list-content tests have something
        // concrete to look up by pid.
        State::with_processes(path, vec![item(123)])
    }

    #[test]
    fn test_kill_no_selection() {
        let mut s = test_state("kill_none");
        assert!(matches!(s.kill(Signal::Sighup), KillStatus::NoSelection));
    }

    /// A signal to a pid that will not resolve to a live process reports
    /// `Failed` with the OS error (ESRCH) — never a panic and never a false
    /// `Sent`.
    #[test]
    fn test_kill_unknown_pid_reports_failure() {
        let mut s = test_state("kill_unknown");
        // A pid far beyond typical allocations that is not going to be live.
        s.selected_pid = Some(2_147_483_647);
        match s.kill(Signal::Sighup) {
            KillStatus::Failed { cause, .. } => {
                assert_eq!(
                    cause.raw_os_error(),
                    Some(libc::ESRCH),
                    "no such pid must surface ESRCH, got {cause:?}"
                );
            }
            other => panic!("signalling a dead pid must report Failed, got {other:?}"),
        }
    }

    /// The error pop-up text: EPERM (another user's process) is the user's
    /// real-world case and must be named as such; ESRCH and anything else
    /// fall back to the raw OS error.
    #[test]
    fn test_describe_failure_messages() {
        let eperm = std::io::Error::from_raw_os_error(libc::EPERM);
        let (title, detail) = describe_failure(Signal::Sigterm, 999, &eperm);
        assert!(title.contains("No permission"), "{title}");
        assert!(title.contains("SIGTERM"), "{title}");
        assert!(detail.to_lowercase().contains("another user"), "{detail}");

        let esrch = std::io::Error::from_raw_os_error(libc::ESRCH);
        let (t2, _d2) = describe_failure(Signal::Sigkill, 999, &esrch);
        assert!(t2.contains("no longer running"), "{t2}");

        let other = std::io::Error::other("boom");
        let (t3, d3) = describe_failure(Signal::Sigkill, 999, &other);
        assert!(t3.contains("SIGKILL") && t3.contains("pid 999"), "{t3}");
        assert_eq!(d3, "boom");
    }

    /// The pop-up is a self-contained modal window parented to the main
    /// window, carrying the failure title/detail, and its OK button
    /// disposes the dialog. (GTK is initialized on this test's own thread,
    /// like any other unit test in the binary — no main loop is involved.)
    #[test]
    fn test_failure_popup_is_modal_and_ok_dismisses() {
        let _ = gtk4::init();
        let parent = gtk4::Window::default();
        let title = "No permission to send Terminate to pid 1";
        let detail = "The process is owned by another user.";
        let before = gtk4::Window::list_toplevels().len();

        show_failure_popup(&parent, title, detail);

        let toplevels = gtk4::Window::list_toplevels();
        assert_eq!(
            toplevels.len(),
            before + 1,
            "pop-up must add one top-level window"
        );
        let popup = toplevels
            .iter()
            .find_map(|w| {
                w.downcast_ref::<gtk4::Window>()
                    .filter(|win| win.title().map(|t| t.to_string()) == Some(title.to_string()))
            })
            .expect("the new toplevel is the pop-up")
            .clone();
        assert!(popup.is_modal(), "the pop-up is modal");
        assert!(
            popup
                .transient_for()
                .is_some_and(|t| std::ptr::eq(t.as_ptr(), parent.as_ptr())),
            "the pop-up is parented to the main window"
        );

        let vbox = popup
            .child()
            .expect("a single body widget")
            .downcast::<gtk4::Box>()
            .expect("the body is a vertical Box");
        let detail_label = vbox
            .first_child()
            .expect("first child (icon)")
            .next_sibling()
            .expect("detail label")
            .downcast::<gtk4::Label>()
            .expect("a detail label");
        assert_eq!(
            detail_label.label().as_str(),
            detail,
            "the detail text is shown verbatim"
        );
        assert!(detail_label.is_selectable(), "the error text is copyable");

        let ok = detail_label
            .next_sibling()
            .expect("OK button")
            .downcast::<gtk4::Button>()
            .expect("an OK button");
        assert_eq!(ok.label().map(|s| s.to_string()), Some("OK".to_string()));

        // Emits the same signal GTK's release path fires on a real pointer
        // click (the headless test env never realizes the window, so
        // `activate` alone cannot reach it).
        assert!(popup.is_visible(), "the pop-up is presented");
        ok.emit_by_name::<()>("clicked", &[] as &[&dyn gtk4::glib::value::ToValue]);
        assert!(!popup.is_visible(), "clicking OK disposes the pop-up");
        assert_eq!(
            gtk4::Window::list_toplevels().len(),
            before,
            "the disposed pop-up is gone from the toplevels"
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
