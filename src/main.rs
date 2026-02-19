use log::debug;
use simplelog::*;
use simpletaskmgr::process_list::PROCESS_LIST;
use std::cell::RefCell;
use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::{create_rw_signal, SignalGet, SignalUpdate};
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::{
    container, dyn_stack, h_stack, label, scroll, v_stack, virtual_list, Decorators, ScrollExt,
    VirtualDirection, VirtualItemSize,
};
use floem::{IntoView, View};
use im::Vector;
use simpletaskmgr::{
    cpu_tracker::CpuTracker, process::process_names, process::Process, process::UserFilter,
};

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

fn process_list() -> impl IntoView {
    let process_list = PROCESS_LIST.with(|s| s.processes);
    dyn_stack(
        move || process_list.get(),
        |process| process.clone(),
        |process| process,
    )
    .style(|s| s.flex_col().min_size(0, 0))
    .debug_name("Process List Stack")
}

fn app_view() -> impl IntoView {
    let process_list_signal = create_rw_signal(Vector::new());
    let selected_process = create_rw_signal(None);
    let cpu_tracker = RefCell::new(CpuTracker::new());
    let tick = create_rw_signal(());

    create_effect(move |_| {
        let cpu_tracker = cpu_tracker.clone();
        exec_after(Duration::from_millis(1000), move |_| {
            debug!("Refreshing process list");
            // Get process list using process_names() from lib.rs
            let processes = process_names(UserFilter::Current);

            // Update CPU usage for each process
            let mut process_map: std::collections::HashMap<i32, Process> = processes
                .iter()
                .map(|p: &Process| (p.pid, p.clone()))
                .collect();
            cpu_tracker
                .borrow_mut()
                .update_process_cpu_usage(&mut process_map);

            // Convert back to vector
            let mut processes: Vec<Process> = process_map.values().cloned().collect();

            // Sort by CPU usage (highest first)
            processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

            process_list_signal.update(|l| *l = processes.into_iter().collect());
            tick.set(());
        });
    });

    let process_scroll = process_list()
        .style(|s| s.max_width_full().width_full())
        .scroll()
        .style(|s| s.padding(10).padding_right(14))
        .scroll_style(|s| s.shrink_to_fit().handle_thickness(8));

    let main_view = match selected_process.get() {
        Some(process) => container(
            h_stack((
                process_scroll,
                scroll(process_detail_view(process)).style(|s| s.width(50_i32.pct()).height_full()),
            ))
            .style(|s| s.width_full().height_full()),
        ),
        None => container(process_scroll).style(|s| s.width_full().height_full().border(1.0)),
    };

    main_view.style(|s| {
        s.size(100_i32.pct(), 100_i32.pct())
            .padding_vert(20.0)
            .flex_col()
            .items_center()
    })
}

fn main() {
    let _ = SimpleLogger::init(LevelFilter::Debug, Config::default());
    floem::launch(app_view);
}
