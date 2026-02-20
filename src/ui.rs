use crate::process::Process;
use floem::prelude::{RwSignal, SignalGet};
use floem::unit::UnitExt;
use floem::views::{container, dyn_stack, label, scroll, v_stack, Decorators};
use floem::{IntoView, View};
use imbl::Vector;

/// Creates a clickable view for a single process item
pub fn process_item_view(process: Process, on_click: impl Fn(Process) + 'static) -> Box<dyn View> {
    let process_clone = process.clone();
    Box::new(process.into_view().on_click(move |_| {
        on_click(process_clone.clone());
        floem::event::EventPropagation::Continue
    }))
}

/// Creates a detailed view showing comprehensive information about a process
pub fn process_detail_view(process: Process) -> Box<dyn View> {
    let name = process.name.clone();
    let pid = process.pid;
    let ruid = process.ruid;
    let username = process.username.clone();
    let cpu_percent = process.cpu_percent;

    Box::new(
        container(
            scroll(
                container(
                    v_stack((
                        label(move || "=== Process Details ==="),
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
        .style(move |s| s.width(100_i32.pct())),
    )
}

/// Creates a dynamic stack view that displays the list of processes
pub fn process_list_view(processes: RwSignal<Vector<Process>>) -> impl IntoView {
    dyn_stack(
        move || processes.get(),
        |process| process.clone(),
        |process| process,
    )
    .style(|s| s.flex_col().min_size(0, 0))
    .debug_name("Process List Stack")
}
