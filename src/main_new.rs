tmp - use std::collections::HashMap;
tmp - use std::time::Duration;
tmp - 
tmp - use floem::{
tmp -     action::exec_after,
tmp -     prelude::{create_rw_signal, text},
tmp -     reactive::{create_effect, SignalGet, SignalTrack},
tmp -     unit::UnitExt,
tmp -     views::{container, scroll, virtual_list, Decorators, VirtualDirection, VirtualItemSize},
tmp -     IntoView,
tmp - };
tmp - 
tmp - use procfs::process;
tmp - 
tmp - fn app_view() -> impl IntoView {
tmp -     let process_list_signal = create_rw_signal(im::vector![]);
tmp - 
tmp -     create_effect(move |_| {
tmp -         process_list_signal.get().get(0);
tmp - 
tmp -         exec_after(Duration::from_millis(1000), move |_| {
tmp -             let mut processes = vec![];
tmp - 
tmp -             if let Ok(proc_results) = process::all_processes() {
tmp -                 for proc_result in proc_results {
tmp -                     if let Ok(proc) = proc_result {
tmp -                         if let Ok(stat) = proc.stat() {
tmp -                             let name = if let Ok(vec) = proc.cmdline() {
tmp -                                 vec.first()
tmp -                                     .map(|s| s.trim_matches(char::from(0)))
tmp -                                     .unwrap_or("-")
tmp -                             } else {
tmp -                                 "-"
tmp -                             };
tmp - 
tmp -                             processes.push(simpletaskmgr::Process {
tmp -                                 name: name.to_string(),
tmp -                                 pid: stat.pid,
tmp -                                 ruid: 0,
tmp -                                 username: "unknown".to_string(),
tmp -                                 cpu_percent: 0.0,
tmp -                             });
tmp -                         }
tmp -                     }
tmp -                 }
tmp -             }
tmp - 
tmp -             process_list_signal.set(processes);
tmp -         });
tmp -     });
tmp - 
tmp -     container(
tmp -         scroll(
tmp -             virtual_list(
tmp -                 VirtualDirection::Vertical,
tmp -                 VirtualItemSize::Fixed(Box::new(|| 20.0)),
tmp -                 move || process_list_signal.get(),
tmp -                 move |item: &simpletaskmgr::Process| item.pid as i64,
tmp -                 move |item: simpletaskmgr::Process| {
tmp -                     text(format!("PID: {}, Name: {}", item.pid, item.name))
tmp -                         .into_view()
tmp -                         .style(|s| s.height(20.0))
tmp -                 },
tmp -             )
tmp -             .style(|s| s.flex_col().width_full()),
tmp -         )
tmp -         .style(|s| s.width(100.pct()).height(100.pct()).border(1.0)),
tmp -     )
tmp -     .style(|s| {
tmp -         s.size(100.pct(), 100.pct())
tmp -             .padding_vert(20.0)
tmp -             .flex_col()
tmp -             .items_center()
tmp -     })
tmp - }
tmp - 
tmp - fn main() {
tmp -     floem::launch(app_view);
tmp - }
