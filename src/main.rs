use std::time::Duration;

use floem::{
    action::exec_after,
    prelude::create_rw_signal,
    reactive::{create_effect, SignalGet, SignalTrack, SignalUpdate},
    unit::UnitExt,
    views::{
        container, scroll, virtual_list, Decorators, VirtualDirection,
        VirtualItemSize,
    },
    IntoView,
};

fn app_view() -> impl IntoView {
    let process_name_list = simpletaskmgr::process_names();
    let process_name_list = create_rw_signal(process_name_list);
    let tick = create_rw_signal(());

    create_effect(move |_| {
        tick.track();

        exec_after(Duration::from_millis(1000), move |_| {
            process_name_list.update(|l| *l = simpletaskmgr::process_names());
            tick.set(());
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