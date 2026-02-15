use std::time::Duration;

use floem::action::exec_after;
use floem::prelude::container;
use floem::prelude::create_rw_signal;
use floem::prelude::label;
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

use procfs::process;

fn app_view() -> impl IntoView {
    let process_list_signal = create_rw_signal(im::vector![]);

    create_effect(move |_| {
        exec_after(Duration::from_millis(1000), move |_| {
            let mut processes = im::vector![];

            if let Ok(proc_results) = process::all_processes() {
                for proc_result in proc_results {
                    if let Ok(proc) = proc_result {
                        if let Ok(stat) = proc.stat() {
                            let cmdline_vec = if let Ok(cmdline) = proc.cmdline() {
                                cmdline
                            } else {
                                vec![]
                            };
                            let name = cmdline_vec
                                .first()
                                .map(|s| s.trim_matches(char::from(0)))
                                .unwrap_or("-");

                            let process = simpletaskmgr::Process {
                                name: stat.comm.to_string(),
                                pid: stat.pid,
                                ruid: 0,
                                username: "unknown".to_string(),
                                cpu_percent: 0.0,
                            };

                            processes.push_back(process);
                        }
                    }
                }
            }

            process_list_signal.set(processes);
        });
    });

    container(
        scroll(
            virtual_list(
                VirtualDirection::Vertical,
                VirtualItemSize::Fixed(Box::new(|| 20.0)),
                move || process_list_signal.get(),
                move |item: &simpletaskmgr::Process| item.pid as i64,
                move |item: simpletaskmgr::Process| item.into_view(),
            )
            .style(|s| s.flex_col().width_full()),
        )
        .style(|s| s.width(100_i32.pct()).height(100_i32.pct()).border(1.0)),
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
