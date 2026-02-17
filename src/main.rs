use std::cell::RefCell;
use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::{create_rw_signal, SignalUpdate, dyn_stack};
use floem::reactive::{create_effect, SignalGet};
use floem::views::{container, scroll, text, Decorators};
use floem::View;

use simpletaskmgr::{cpu_tracker::CpuTracker, UserFilter};

fn process_item_view(pid: i64, name: String) -> Box<dyn View> {
    Box::new(container(
        text(format!("PID: {}  {}", pid, name))
    )
    .style(|s| s.height(20.0).padding_left(10.0).padding_right(10.0))
    .on_click(move |_| {
        // Show process detail dialog
        simpletaskmgr::show_process_detail(pid as i32);
        floem::event::EventPropagation::Continue
    }))
}

fn process_detail_view(_pid: i32) -> Box<dyn View> {
    Box::new(container(
        scroll(
            container(
                text("Process Details")
            )
        )
        .style(|s| s.size_full())
    )
    .style(|s| s.size_full().flex_col().items_center().padding_vert(40.0))
    .on_click(|_| {
        // Close detail view
        simpletaskmgr::close_process_detail();
        floem::event::EventPropagation::Continue
    }))
}

fn app_view() -> Box<dyn View> {
    let process_list_signal = create_rw_signal(vec![]);
    let cpu_tracker = RefCell::new(CpuTracker::new());
    let show_details = create_rw_signal(false);
    let selected_pid = create_rw_signal(0);

    create_effect(move |_| {
        let cpu_tracker = cpu_tracker.clone();
        let mut processes = process_list_signal.get();
        exec_after(Duration::from_millis(1000), move |_| {
            // Get process list using process_names() from lib.rs
            let _processes = simpletaskmgr::process_names(UserFilter::Current);

            // Update CPU usage for each process
            let mut process_map: std::collections::HashMap<i32, simpletaskmgr::Process> =
                processes.iter().map(|p: &simpletaskmgr::Process| (p.pid, p.clone())).collect();
            cpu_tracker.borrow_mut().update_process_cpu_usage(&mut process_map);

            // Convert back to vector
            processes = process_map.values().cloned().collect();

            // Sort by CPU usage (highest first)
            processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

            process_list_signal.set(processes.iter().cloned().collect());
        });
    });

    // Conditionally render based on show_details
    if show_details.get() {
        Box::new(process_detail_view(selected_pid.get()))
    } else {
        Box::new(container(
            scroll(
                dyn_stack(
                    move || process_list_signal.get(),
                    move |p: &simpletaskmgr::Process| p.pid,
                    move |p| process_item_view(p.pid as i64, p.name.clone())
                )
            )
            .style(|s| s.size_full())
        )
        .style(|s| s.size_full().height(600.0).flex_col()))
    }
}

fn main() {
    floem::launch(app_view);
}