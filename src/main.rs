use floem::IntoView;

use simplelog::*;
use simpletaskmgr::process_list::ProcessList;
use simpletaskmgr::ui::{process_detail_view, process_list_view};
use std::rc::Rc;

use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::{create_rw_signal, SignalGet, SignalTrack, SignalUpdate};
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::{container, h_stack, scroll, Decorators, ScrollExt};

fn app_view() -> impl IntoView {
    let selected_process = create_rw_signal(None);
    let tick = create_rw_signal(());
    let process_list = Rc::new(ProcessList::new());
    let process_list_for_view = Rc::clone(&process_list);
    let tick_for_effect = tick;

    create_effect(move |_| {
        tick_for_effect.track();
        let process_list_for_effect = Rc::clone(&process_list);
        exec_after(Duration::from_millis(1000), move |_| {
            process_list_for_effect.update_process_list();
            tick_for_effect.set(());
        });
    });

    let process_scroll = process_list_view(process_list_for_view.processes)
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
