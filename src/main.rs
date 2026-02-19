use simplelog::*;
use simpletaskmgr::process_list::ProcessList;
use std::rc::Rc;

use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::{create_rw_signal, RwSignal, SignalGet, SignalTrack, SignalUpdate};
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators, ScrollExt};
use floem::{IntoView, View};
use imbl::Vector;

use simpletaskmgr::process::Process;

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

fn process_list_view(processes: RwSignal<Vector<Process>>) -> impl IntoView {
    dyn_stack(
        move || processes.get(),
        |process| process.clone(),
        |process| process,
    )
    .style(|s| s.flex_col().min_size(0, 0))
    .debug_name("Process List Stack")
}

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
