use std::cell::RefCell;
use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::container;
use floem::prelude::create_rw_signal;
use floem::prelude::scroll;
use floem::prelude::virtual_list;
use floem::prelude::SignalGet;
use floem::prelude::SignalUpdate;
use floem::prelude::VirtualDirection;
use floem::prelude::VirtualItemSize;
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::Decorators;
use floem::IntoView;

use simpletaskmgr::{cpu_tracker::CpuTracker, UserFilter};

fn app_view() -> impl IntoView {
    let process_list_signal = create_rw_signal(im::vector![]);
    let cpu_tracker = RefCell::new(CpuTracker::new());

    create_effect(move |_| {
        let cpu_tracker = cpu_tracker.clone();
        exec_after(Duration::from_millis(1000), move |_| {
            // Get process list using process_names() from lib.rs
            let mut processes = simpletaskmgr::process_names(UserFilter::Current);

            // Update CPU usage for each process
            let mut process_map: std::collections::HashMap<i32, simpletaskmgr::Process> =
                processes.iter().map(|p| (p.pid, p.clone())).collect();
            cpu_tracker.borrow_mut().update_process_cpu_usage(&mut process_map);

            // Convert back to vector
            processes = process_map.values().cloned().collect();

            // Sort by CPU usage (highest first)
            processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

            process_list_signal.set(processes);
        });
    });

    container(
        scroll(
            container(
                virtual_list(
                    VirtualDirection::Vertical,
                    VirtualItemSize::Fixed(Box::new(|| 20.0)),
                    move || process_list_signal.get(),
                    move |item: &simpletaskmgr::Process| item.pid as i64,
                    move |item: simpletaskmgr::Process| item.into_view(),
                )
                .style(|s| s.width_full()),
            )
            .style(|s| s.width(800_i32.pct()).height(600_i32.pct())),
        )
        .style(|s| s.size(100_i32.pct(), 100_i32.pct())),
    )
}

fn main() {
    floem::launch(app_view);
}
