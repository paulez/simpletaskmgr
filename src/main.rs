use std::cell::RefCell;
use std::time::Duration;

use floem::{
    action::exec_after,
    prelude::create_rw_signal,
    reactive::{create_effect, SignalGet, SignalTrack, SignalUpdate},
    unit::UnitExt,
    views::{container, scroll, virtual_list, Decorators, VirtualDirection, VirtualItemSize},
    IntoView,
};

use simpletaskmgr::{cpu_tracker::CpuTracker, UserFilter};

fn app_view() -> impl IntoView {
    let process_name_list = create_rw_signal(simpletaskmgr::process_names(UserFilter::Current));
    let tick = create_rw_signal(());
    let cpu_tracker = RefCell::new(CpuTracker::new());

    create_effect(move |_| {
        tick.track();

        // Update inside the effect to avoid moving issues
        let cpu_tracker_clone = cpu_tracker.clone();
        let process_name_list_clone = process_name_list.clone();

        exec_after(Duration::from_millis(1000), move |_| {
            // Update process list
            let mut processes = simpletaskmgr::process_names(UserFilter::Current);
            cpu_tracker_clone.borrow_mut().update(&mut processes);

            // Sort by CPU usage (highest first)
            processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

            process_name_list_clone.update(|l| *l = processes);
        })
    });

    container(
        scroll(
            virtual_list(
                VirtualDirection::Vertical,
                VirtualItemSize::Fixed(Box::new(|| 20.0)),
                move || process_name_list.get(),
                move |item| item.clone(),
                move |item| item.into_view().style(|s| s.height(20.0)),
            )
            .style(|s| s.flex_col().width_full()),
        )
        .style(|s| s.width(100.pct()).height(100.pct()).border(1.0)),
    )
    .style(|s| {
        s.size(100.pct(), 100.pct())
            .padding_vert(20.0)
            .flex_col()
            .items_center()
    })
}

fn main() {
    floem::launch(app_view);
}
