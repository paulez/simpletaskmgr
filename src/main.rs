use std::cell::RefCell;
use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::{create_rw_signal, SignalGet, SignalUpdate};
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::{
    container, h_stack, label, scroll, v_stack, virtual_list, Decorators, VirtualDirection,
    VirtualItemSize,
};
use floem::{IntoView, View};
use im::Vector;
use simpletaskmgr::{cpu_tracker::CpuTracker, Process, UserFilter};

fn process_item_view(process: Process, on_click: impl Fn(Process) + 'static) -> Box<dyn View> {
    let process_clone = process.clone();
    Box::new(process.into_view().on_click(move |_| {
        on_click(process_clone.clone());
        floem::event::EventPropagation::Continue
    }))
}

fn process_detail_view(process: Process) -> Box<dyn View> {
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

fn app_view() -> Box<dyn View> {
    let process_list_signal = create_rw_signal(Vector::new());
    let selected_process = create_rw_signal(None);
    let cpu_tracker = RefCell::new(CpuTracker::new());

    create_effect(move |_| {
        let cpu_tracker = cpu_tracker.clone();
        exec_after(Duration::from_millis(1000), move |_| {
            // Get process list using process_names() from lib.rs
            let processes = simpletaskmgr::process_names(UserFilter::Current);

            // Update CPU usage for each process
            let mut process_map: std::collections::HashMap<i32, simpletaskmgr::Process> = processes
                .iter()
                .map(|p: &simpletaskmgr::Process| (p.pid, p.clone()))
                .collect();
            cpu_tracker
                .borrow_mut()
                .update_process_cpu_usage(&mut process_map);

            // Convert back to vector
            let mut processes: Vec<simpletaskmgr::Process> =
                process_map.values().cloned().collect();

            // Sort by CPU usage (highest first)
            processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

            process_list_signal.set(processes.into_iter().collect());
        });
    });

    let main_view = container(match selected_process.get() {
        Some(process) => container(
            h_stack((
                scroll(
                    virtual_list(
                        VirtualDirection::Vertical,
                        VirtualItemSize::Fixed(Box::new(|| 30.0)),
                        move || process_list_signal.get(),
                        move |item| item.clone(),
                        move |item| process_item_view(item, move |p| selected_process.set(Some(p))),
                    )
                    .style(|s| s.width(50_i32.pct()).height(100_i32.pct())),
                )
                .style(|s| s.width(50_i32.pct()).height(100_i32.pct())),
                scroll(process_detail_view(process))
                    .style(|s| s.width(50_i32.pct()).height(100_i32.pct())),
            ))
            .style(|s| s.size(100_i32.pct(), 100_i32.pct())),
        ),

        None => container(
            scroll(
                virtual_list(
                    VirtualDirection::Vertical,
                    VirtualItemSize::Fixed(Box::new(|| 30.0)),
                    move || process_list_signal.get(),
                    move |item| item.clone(),
                    move |item| process_item_view(item, move |p| selected_process.set(Some(p))),
                )
                .style(|s| s.width(100_i32.pct()).height(100_i32.pct())),
            )
            .style(|s| s.width(100_i32.pct()).height(100_i32.pct())),
        ),
    });

    Box::new(main_view.style(|s| {
        s.size(100_i32.pct(), 100_i32.pct())
            .padding_vert(20.0)
            .flex_col()
            .items_center()
    }))
}

fn main() {
    floem::launch(app_view);
}
