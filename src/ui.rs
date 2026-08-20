use std::rc::Rc;

use crate::process::ProcessItem;
use crate::SortColumn;
use crate::SortDirection;
use floem::prelude::{h_stack, RwSignal, SignalGet, SignalUpdate};
use floem::taffy::style_helpers::{auto, fr};
use floem::unit::UnitExt;
use floem::views::{container, dyn_stack, dyn_view, label, scroll, text, v_stack, Decorators};
use floem::{IntoView, View};
use imbl::Vector;
use log::debug;

/// Creates a clickable view for a single process row.
///
/// Every label reads the row's `value` signal reactively, so when the data
/// layer swaps in a new snapshot for this `pid` the row's text updates in
/// place instead of the row being torn down and rebuilt.
pub fn process_item_view(
    item: ProcessItem,
    selected_pid: RwSignal<Option<i32>>,
    on_click: Rc<dyn Fn(i32)>,
) -> Box<dyn View> {
    let pid = item.pid;
    let value = item.value;

    Box::new(
        h_stack((
            label(move || value.get().pid.to_string()),
            label(move || value.get().username.clone()),
            label(move || value.get().cpu_percent_str()),
            label(move || value.get().name.clone()),
        ))
        .style(move |s| {
            s.width_full()
                .items_center()
                .gap(6)
                .grid()
                .grid_template_columns(vec![auto(), auto(), fr(1.), fr(1.)])
                .padding_vert(4)
        })
        .on_click(move |_| {
            debug!("Clicked process pid {:?}", pid);
            selected_pid.set(Some(pid));
            on_click(pid);
            floem::event::EventPropagation::Continue
        }),
    )
}

/// Creates a detailed view showing comprehensive information about one process.
///
/// Wraps the content in `dyn_view` so it re-runs reactively when the selected
/// pid or the list changes. Each field label also reads the row's `value`
/// signal so the numbers update in place when the data layer swaps it.
pub fn process_detail_view(
    selected_pid: RwSignal<Option<i32>>,
    processes: RwSignal<Vector<ProcessItem>>,
) -> Box<dyn View> {
    Box::new(dyn_view(move || {
        match (selected_pid.get(), processes.get()) {
            (None, _) => container(text("No process selected")).into_any(),
            (Some(pid), processes) => {
                match processes.iter().find(|item| item.pid == pid).cloned() {
                    None => {
                        debug!("Process not found for pid: {pid}");
                        container(text(format!("Process not found for pid: {pid}"))).into_any()
                    }
                    Some(item) => {
                        let value = item.value;
                        let pid = item.pid;
                        container(
                            scroll(
                                container(
                                    v_stack((
                                        label(move || "Process Details")
                                            .style(move |s| s.font_bold().font_size(18.0)),
                                        label(move || format!("PID: {}", pid)),
                                        label(move || format!("Name: {}", value.get().name)),
                                        label(move || format!("UID: {}", value.get().ruid)),
                                        label(move || {
                                            format!("Username: {}", value.get().username)
                                        }),
                                        label(move || {
                                            format!("CPU Usage: {}", value.get().cpu_percent_str())
                                        }),
                                    ))
                                    .style(move |s: floem::style::Style| s.flex_col().gap(8)),
                                )
                                .style(move |s| s.padding(20.0)),
                            )
                            .style(move |s| s.width(100_i32.pct())),
                        )
                        .style(move |s| s.width(100_i32.pct()))
                        .into_any()
                    }
                }
            }
        }
    }))
}

/// Creates a dynamic stack view that displays the list of processes, keyed on
/// the stable `pid` so each row is updated in place rather than rebuilt.
pub fn process_list_view(
    processes: RwSignal<Vector<ProcessItem>>,
    selected_pid: RwSignal<Option<i32>>,
    on_click: impl Fn(i32) + 'static,
) -> impl IntoView {
    let on_click = Rc::new(on_click);
    dyn_stack(
        move || processes.get(),
        |item| item.pid,
        move |item| process_item_view(item, selected_pid, on_click.clone()),
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
