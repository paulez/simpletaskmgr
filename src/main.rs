use floem::IntoView;
use log::info;
use simplelog::*;
use simpletaskmgr::config::Config;
use simpletaskmgr::metrics::SystemMetrics;
use simpletaskmgr::process_list::ProcessList;
use simpletaskmgr::ui::{process_detail_view, process_list_header, process_list_view};
use simpletaskmgr::usage_graph::usage_graph_view;
use simpletaskmgr::{SortColumn, SortDirection};
use std::cell::RefCell;
use std::rc::Rc;

use floem::action::exec_after;
use floem::prelude::*;
use floem::prelude::{create_rw_signal, SignalGet, SignalTrack, SignalUpdate};
use floem::reactive::create_effect;
use floem::unit::UnitExt;
use floem::views::{container, h_stack, scroll, Decorators, ScrollExt};

fn app_view() -> impl IntoView {
    let selected_process_id = create_rw_signal(None);
    let tick = create_rw_signal(());
    let sort_column = create_rw_signal(crate::SortColumn::CpuPercent);
    let sort_direction = create_rw_signal(crate::SortDirection::Descending);
    let process_list = Rc::new(ProcessList::init());
    let process_list_for_view = Rc::clone(&process_list);
    let tick_for_effect = tick;
    // System CPU/memory history shared between the refresh loop (which appends
    // samples) and the graph view (which reads them reactively).
    let metrics = Rc::new(RefCell::new(SystemMetrics::new()));

    let process_list_for_effect = Rc::clone(&process_list);
    let metrics_for_effect = Rc::clone(&metrics);
    create_effect(move |_| {
        tick_for_effect.track();
        let process_list_for_effect = Rc::clone(&process_list_for_effect);
        let metrics_for_effect = Rc::clone(&metrics_for_effect);
        // Read the sort column/direction at fire-time so a user's sort change is
        // applied to the freshly loaded data, not the values from the previous tick.
        exec_after(Config::refresh_interval(), move |_| {
            let column = sort_column.get();
            let direction = sort_direction.get();
            process_list_for_effect.update_process_list();
            process_list_for_effect.sort_processes(column, direction);
            // Take a system-wide CPU/memory sample each refresh so the graph
            // scrolls at the same cadence as the list.
            metrics_for_effect.borrow_mut().push_sample();
            tick_for_effect.set(());
        });
    });

    let process_list_for_sort = Rc::clone(&process_list);
    create_effect(move |_| {
        sort_column.track();
        sort_direction.track();
        let column = sort_column.get();
        let direction = sort_direction.get();
        let process_list_for_sort = Rc::clone(&process_list_for_sort);
        process_list_for_sort.sort_processes(column, direction);
    });

    let metrics_for_view = Rc::clone(&metrics);
    let history_signal = metrics_for_view.borrow().history_signal;
    let main_view = dyn_container(
        move || selected_process_id.get(),
        move |selected_process_id_item| {
            let usage_graph = usage_graph_view(history_signal);
            let header = process_list_header(sort_column, sort_direction, move |column| {
                let current_column = sort_column.get();
                let current_direction = sort_direction.get();
                let new_direction = if current_column == column {
                    match current_direction {
                        SortDirection::Ascending => SortDirection::Descending,
                        SortDirection::Descending => SortDirection::Ascending,
                    }
                } else {
                    SortDirection::Ascending
                };
                if current_column != column {
                    sort_column.set(column);
                }
                sort_direction.set(new_direction);
            });

            let process_scroll = process_list_view(
                process_list_for_view.processes,
                selected_process_id,
                move |pid: i32| {
                    log::info!("Selected process pid {pid}");
                },
            )
            .style(|s| s.max_width_full().width_full())
            .scroll()
            .style(|s| s.padding(10).padding_right(14))
            .scroll_style(|s| s.shrink_to_fit().handle_thickness(8));

            let main_container = match selected_process_id_item {
                Some(_pid) => container(
                    h_stack((
                        process_scroll,
                        scroll(process_detail_view(
                            selected_process_id,
                            process_list_for_view.processes,
                        ))
                        .style(|s| s.width(50_i32.pct()).height_full()),
                    ))
                    .style(|s| s.width_full().height_full()),
                ),
                None => container(process_scroll),
            };
            container(v_stack((usage_graph, header, main_container)))
                .style(|s| s.width_full().height_full().border(1.0))
        },
    );

    main_view.style(|s| {
        s.size(100_i32.pct(), 100_i32.pct())
            .padding_vert(20.0)
            .flex_col()
            .items_center()
    })
}

fn main() {
    let log_config = ConfigBuilder::new()
        .add_filter_allow_str("simpletaskmgr")
        .build();
    let _ = SimpleLogger::init(LevelFilter::Debug, log_config);
    info!("Starting simpletaskmgr");
    if let Err(e) = run_app() {
        log::error!("Application error: {}", e);
        std::process::exit(1);
    }
}

fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    floem::launch(app_view);
    Ok(())
}
