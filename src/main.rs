use std::time::Duration;

use floem::{
    action::exec_after,
    prelude::create_rw_signal,
    reactive::{create_effect, SignalGet, SignalTrack, SignalUpdate},
    taffy::style_helpers::{auto, fr},
    unit::UnitExt,
    views::{
        container, h_stack, label, scroll, virtual_list, Decorators, Stack, VirtualDirection,
        VirtualItemSize,
    },
    IntoView,
};
use users::{Users, UsersCache};
use procfs::process;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Process {
    name: String,
    pid: i32,
    ruid: u32,
    username: String,
}

impl IntoView for Process {
    type V = Stack;

    fn into_view(self) -> Self::V {
        h_stack((
            label(move || self.pid.to_string()),
            label(move || self.ruid.to_string()),
            label(move || self.username.clone()),
            label(move || self.name.to_string()),
        ))
        .style(move |s| {
            s.items_center()
                .gap(6)
                .grid()
                .grid_template_columns(vec![auto(), auto(), fr(1.), auto()])
        })
    }
}

fn process_names() -> im::Vector<Process> {
    let cache = UsersCache::new();
    process::all_processes()
        .expect("Can't read /proc")
        .filter_map(|p| match p {
            Ok(p) => Some(p),
            Err(e) => match e {
                procfs::ProcError::NotFound(_) => None,
                procfs::ProcError::Io(_e, _path) => None,
                x => {
                    println!("Can't read process due to error {x:?}");
                    None
                }
            },
        })
        .filter_map(|proc| match proc.status() {
            Ok(status) => {
                let username = match cache.get_user_by_uid(status.ruid) {
                    Some(user) => {
                        let user_name: &std::ffi::OsStr = user.name();
                        user_name.to_string_lossy().to_string()
                    },
                    None => "unknown".to_string(),
                };
                Some(Process {
                    name: status.name,
                    pid: status.pid,
                    ruid: status.ruid,
                    username,
                })
            }
            Err(e) => {
                println!("Can't get process status due to error {e:?}");
                None
            }
        })
        .collect()
}

fn app_view() -> impl IntoView {
    let process_name_list = process_names();
    let process_name_list = create_rw_signal(process_name_list);
    let tick = create_rw_signal(());
    create_effect(move |_| {
        tick.track();

        exec_after(Duration::from_millis(1000), move |_| {
            process_name_list.update(|l| *l = process_names());
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
