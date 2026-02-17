use std::cell::RefCell;
use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::{create_rw_signal, SignalGet, SignalUpdate};
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::{container, h_stack, label, scroll, virtual_list, Decorators, VirtualDirection, VirtualItemSize};
use floem::View;
use im::Vector;

use simpletaskmgr::{cpu_tracker::CpuTracker, UserFilter};

fn process_item_view(pid: i32, ruid: u32, username: String, cpu_percent: f64, name: String) -> Box<dyn View> {
    Box::new(container(
        h_stack((
            label(move || cpu_percent.to_string()),
            label(move || pid.to_string()),
            label(move || ruid.to_string()),
            label(move || username.clone()),
            label(move || name.clone()),
        ))
        .style(|s| s.height(20.0).gap(10).items_center())
    )
    .on_click(move |_| {
        // Show process detail dialog
        simpletaskmgr::show_process_detail(pid);
        floem::event::EventPropagation::Continue
    }))
}

fn app_view() -> Box<dyn View> {
    let process_list_signal = create_rw_signal(Vector::new());
    let cpu_tracker = RefCell::new(CpuTracker::new());

    create_effect(move |_| {
        let cpu_tracker = cpu_tracker.clone();
        exec_after(Duration::from_millis(1000), move |_| {
            // Get process list using process_names() from lib.rs
            let processes = simpletaskmgr::process_names(UserFilter::Current);

            // Update CPU usage for each process
            let mut process_map: std::collections::HashMap<i32, simpletaskmgr::Process> =
                processes.iter().map(|p: &simpletaskmgr::Process| (p.pid, p.clone())).collect();
            cpu_tracker.borrow_mut().update_process_cpu_usage(&mut process_map);

            // Convert back to vector
            let mut processes: Vec<simpletaskmgr::Process> = process_map.values().cloned().collect();

            // Sort by CPU usage (highest first)
            processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

            process_list_signal.set(processes.into_iter().collect());
        });
    });

    Box::new(container(
        scroll(
            virtual_list(
                VirtualDirection::Vertical,
                VirtualItemSize::Fixed(Box::new(|| 20.0)),
                move || process_list_signal.get(),
                move |item| item.clone(),
                move |item| {
                    process_item_view(item.pid, item.ruid, item.username.clone(), item.cpu_percent, item.name.clone())
                }
            )
            .style(|s| s.flex_col().width_full())
        )
        .style(|s| s.width(100_i32.pct()).height(100_i32.pct()).border(1.0))
    )
    .style(|s| {
        s.size(100_i32.pct(), 100_i32.pct())
            .padding_vert(20.0)
            .flex_col()
            .items_center()
    }))
}

fn main() {
    floem::launch(app_view);
}