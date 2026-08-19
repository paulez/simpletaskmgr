use crate::process::TaskMgrProcess;
use crate::SortColumn;
use crate::SortDirection;
use floem::prelude::{h_stack, RwSignal, SignalGet};

use floem::taffy::style_helpers::{auto, fr};
use floem::unit::UnitExt;
use floem::views::{container, dyn_container, dyn_stack, label, scroll, text, v_stack, Decorators};
use floem::{IntoView, View};
use imbl::Vector;
use log::debug;
use std::rc::Rc;

/// Creates a clickable view for a single process item
pub fn process_item_view(
    process: TaskMgrProcess,
    on_click: Rc<dyn Fn(TaskMgrProcess)>,
) -> Box<dyn View> {
    let process_clone = process.clone();
    Box::new(process.into_view().on_click(move |_| {
        debug!("That's a click! Clicked process is {:?}", process_clone);
        on_click(process_clone.clone());
        floem::event::EventPropagation::Continue
    }))
}

/// Creates a detailed view showing comprehensive information about a process
pub fn process_detail_view(
    pid_signal: RwSignal<Option<i32>>,
    processes: RwSignal<Vector<TaskMgrProcess>>,
) -> Box<dyn View> {
    let pid = pid_signal.get();
    match pid {
        None => Box::new(container(text("No process selected"))),
        Some(pid) => Box::new(dyn_container(
            move || processes.get(),
            move |processes| {
                let process = processes.iter().find(|p| p.pid == pid).cloned();
                match process {
                    None => {
                        // A selected process exiting is expected, not an error; keep it quiet.
                        debug!("Process not found for pid: {pid}");
                        container(text(format!("Process not found for pid: {pid}")))
                    }
                    Some(process) => {
                        let name = process.name.clone();
                        let pid = process.pid;
                        let ruid = process.ruid;
                        let username = process.username.clone();
                        let cpu_percent = process.cpu_percent;
                        container(
                            scroll(
                                container(
                                    v_stack((
                                        label(move || "Process Details")
                                            .style(move |s| s.font_bold().font_size(18.0)),
                                        label(move || format!("PID: {}", pid)),
                                        label(move || format!("Name: {}", name)),
                                        label(move || format!("UID: {}", ruid)),
                                        label(move || format!("Username: {}", username)),
                                        label(move || format!("CPU Usage: {:.1}%", cpu_percent)),
                                    ))
                                    .style(move |s: floem::style::Style| s.flex_col().gap(8)),
                                )
                                .style(move |s| s.padding(20.0)),
                            )
                            .style(move |s| s.width(100_i32.pct())),
                        )
                        .style(move |s| s.width(100_i32.pct()))
                    }
                }
            },
        )),
    }
}

/// Creates a dynamic stack view that displays the list of processes
pub fn process_list_view(
    processes: RwSignal<Vector<TaskMgrProcess>>,
    on_click: impl Fn(TaskMgrProcess) + 'static,
) -> impl IntoView {
    let on_click = Rc::new(on_click);
    dyn_stack(
        move || processes.get(),
        |process: &TaskMgrProcess| process.clone(),
        move |process| process_item_view(process, on_click.clone()),
    )
    .style(|s| s.flex_col().min_size(0, 0))
    .debug_name("Process List Stack")
}

/// Creates a header row for the process list with clickable columns
/// that allow sorting by different fields
pub fn process_list_header(
    sort_column: RwSignal<crate::SortColumn>,
    sort_direction: RwSignal<crate::SortDirection>,
    on_sort: impl Fn(crate::SortColumn) + 'static,
) -> impl IntoView {
    let on_sort = Rc::new(on_sort);

    h_stack((
        // PID Header
        label(move || {
            let mut pid_header = "PID".to_string();
            if sort_column.get() == SortColumn::Pid {
                pid_header.push_str(match sort_direction.get() {
                    SortDirection::Ascending => " ↑",
                    SortDirection::Descending => " ↓",
                });
            }
            pid_header
        })
        .on_click({
            let on_sort_clone = on_sort.clone();
            move |_| {
                on_sort_clone(SortColumn::Pid);
                floem::event::EventPropagation::Continue
            }
        })
        .style(move |s| s.padding_vert(4)),
        // Username Header
        label(move || {
            let mut username_header = "Username".to_string();
            if sort_column.get() == SortColumn::Username {
                username_header.push_str(match sort_direction.get() {
                    SortDirection::Ascending => " ↑",
                    SortDirection::Descending => " ↓",
                });
            }
            username_header
        })
        .on_click({
            let on_sort_clone = on_sort.clone();
            move |_| {
                on_sort_clone(SortColumn::Username);
                floem::event::EventPropagation::Continue
            }
        })
        .style(move |s| s.padding_vert(4)),
        // CPU Header
        label(move || {
            let mut cpu_header = "CPU%".to_string();
            if sort_column.get() == SortColumn::CpuPercent {
                cpu_header.push_str(match sort_direction.get() {
                    SortDirection::Ascending => " ↑",
                    SortDirection::Descending => " ↓",
                });
            }
            cpu_header
        })
        .on_click({
            let on_sort_clone = on_sort.clone();
            move |_| {
                on_sort_clone(SortColumn::CpuPercent);
                floem::event::EventPropagation::Continue
            }
        })
        .style(move |s| s.padding_vert(4)),
        // Name Header
        label(move || {
            let mut name_header = "Name".to_string();
            if sort_column.get() == SortColumn::Name {
                name_header.push_str(match sort_direction.get() {
                    SortDirection::Ascending => " ↑",
                    SortDirection::Descending => " ↓",
                });
            }
            name_header
        })
        .on_click({
            let on_sort_clone = on_sort.clone();
            move |_| {
                on_sort_clone(SortColumn::Name);
                floem::event::EventPropagation::Continue
            }
        })
        .style(move |s| s.padding_vert(4)),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .gap(6)
            .grid()
            .grid_template_columns(vec![auto(), auto(), auto(), fr(1.)])
            .padding_vert(4)
            .background(floem::prelude::Color::LIGHT_GRAY)
    })
}
