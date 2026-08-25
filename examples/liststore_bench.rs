//! Compares the two `ListStore` refresh strategies used by the process list.
//!
//! Run it (a display is not required; one is recommended):
//!
//! ```sh
//! cargo run --release --example liststore_bench            # plain
//! xvfb-run -a cargo run --release --example liststore_bench # Xvfb + UI level
//! ```
//!
//! Tunables (env vars): `BENCH_ROWS` (list size, default 500),
//! `BENCH_ITERS` (timed iterations, default 20).
//!
//! Sections:
//!
//! 1. **Store level** — wall time and `items-changed` event count of
//!    `Strategy::apply` alone. Scenarios:
//!     * `unchanged` — identical data and order (best case for InPlace),
//!     * `real churn` — a small active set changes and reorders, the rest
//!       stays idle (typical for a real process list),
//!     * `full change` — every row's data changes and the order permutes
//!       (worst case).
//! 2. **UI level** (needs a display) — real `gtk4::ListView` with the same
//!    label factory the app uses, inside a `ScrolledWindow`, with a visible
//!    window. Each refresh is measured end-to-end (apply + one full GTK
//!    repaint: row setup/bind/allocation) and the vertical scroll position
//!    before/after is reported, demonstrating the original bug and its fix.

use std::rc::Rc;
use std::time::{Duration, Instant};

use glib::prelude::*;
use gtk4::gio::ListStore;
use gtk4::prelude::*;

use simpletaskmgr::process::ProcessItem;
use simpletaskmgr::process::TaskMgrProcess;
use simpletaskmgr::process_row::ProcessRow;
use simpletaskmgr::refresh_strategy::Strategy;

fn item(pid: i32) -> ProcessItem {
    let mut v = TaskMgrProcess::new(format!("proc-{pid}"), pid, 1000, "paul".to_string(), 0.0);
    v.disk_read_speed = None;
    v.disk_write_speed = None;
    ProcessItem::new(&v)
}

fn set_cpu(it: &mut ProcessItem, cpu: f64) {
    it.value.cpu_percent = cpu;
    it.value.disk_read_speed = Some(cpu * 4096.0);
    it.value.disk_write_speed = Some(cpu * 512.0);
}

struct Result {
    iters: usize,
    total_ns: u128,
    events: u64, // items-changed signals
    binds: u64,  // factory bind callbacks (UI level only)
}

impl Result {
    fn per_iter_us(&self) -> f64 {
        self.total_ns as f64 / self.iters as f64 / 1000.0
    }
}

fn print(name: &str, r: &Result) {
    println!(
        "  {name:<18} {:>8.2} us/op  {:>5} events/op  {:>5} binds/op",
        r.per_iter_us(),
        r.events as f64 / r.iters as f64,
        r.binds as f64 / r.iters as f64
    );
}

fn store_level() {
    let rows: usize = std::env::var("BENCH_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let iters = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    let active = rows.div_ceil(7);
    let base: Vec<ProcessItem> = (1..=rows as i32).map(item).collect();

    // A scenario derives a tick's items from the base list and the round number.
    type MakeFn = dyn Fn(usize, &Vec<ProcessItem>) -> Vec<ProcessItem>;
    struct Scenario {
        name: &'static str,
        make: Box<MakeFn>,
    }
    let scenarios: Vec<Scenario> = vec![
        Scenario {
            name: "unchanged",
            make: Box::new(move |_: usize, base: &Vec<ProcessItem>| base.clone()),
        },
        Scenario {
            name: "churn (real)",
            make: Box::new(move |round: usize, base: &Vec<ProcessItem>| {
                let mut items = base.clone();
                for i in 0..active {
                    let r = (i * 13 + round) % active;
                    set_cpu(&mut items[(rows - 1 - r) as usize], (r + 1) as f64 * 3.5);
                }
                if round % 2 == 1 {
                    items.rotate_left(2);
                }
                items
            }),
        },
        Scenario {
            name: "full change",
            make: Box::new(move |round: usize, base: &Vec<ProcessItem>| {
                let mut items = base.clone();
                for (i, it) in items.iter_mut().enumerate() {
                    set_cpu(it, ((i + round) % 97) as f64 * 0.7);
                }
                items.rotate_left(round + 1);
                items
            }),
        },
    ];

    for strat in [Strategy::RebuildAll, Strategy::InPlace] {
        let label = match strat {
            Strategy::RebuildAll => "RebuildAll",
            Strategy::InPlace => "InPlace",
        };
        println!("\n=== rows={rows}  strat={label}  (store level, {iters} iters) ===");
        for sc in &scenarios {
            let mut items = (sc.make)(0, &base);
            let store = ListStore::new::<ProcessRow>();
            strat.apply(&store, &items);
            let c = Rc::new(std::cell::Cell::new(0u64));
            let c2 = c.clone();
            let _h = store.connect_items_changed(move |_s, _pos, _r, _a| {
                c2.set(c2.get() + 1);
            });
            let start = Instant::now();
            for i in 1..=iters {
                let next = (sc.make)(i, &base);
                items = next;
                strat.apply(&store, &items);
            }
            let elapsed = start.elapsed();
            let r = Result {
                iters,
                total_ns: elapsed.as_nanos(),
                events: c.get(),
                binds: 0,
            };
            print(sc.name, &r);
            // End-state must equal the last supplied target.
            let got: Vec<i32> = (0..store.n_items())
                .map(|i| {
                    store
                        .item(i)
                        .and_then(|o| o.downcast::<ProcessRow>().ok())
                        .expect("row")
                        .item()
                        .pid
                })
                .collect();
            assert_eq!(
                got,
                items.iter().map(|i| i.pid).collect::<Vec<_>>(),
                "{} end-state",
                sc.name
            );
        }
    }
}

/// Spins the main context so pending GTK work (row setup/bind/allocate,
/// scroll adjustments) settles before the next measurement.
fn pump_frame() {
    let ctx = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(25);
    loop {
        if !ctx.pending() {
            break;
        }
        let _ = ctx.iteration(false);
        if Instant::now() >= deadline {
            break;
        }
        std::thread::yield_now();
    }
}

/// Scrolls so the top lands at `target` (absolute px), retrying until the
/// value sticks. Row layout/`upper` grows asynchronously after a model
/// change, so a single `set_value` can be clamped away.
fn pin_scroll(adj: &gtk4::Adjustment, target: f64) {
    for _ in 0..50 {
        adj.set_value(target);
        if (adj.value() - target).abs() < 1.0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
        pump_frame();
    }
}

struct UiHarness {
    _window: gtk4::Window,
    store: ListStore,
    scroll: gtk4::ScrolledWindow,
    binds: Rc<std::cell::Cell<u64>>,
}

fn ui_harness(rows: usize) -> UiHarness {
    let factory = gtk4::SignalListItemFactory::new();
    let bind_counter = Rc::new(std::cell::Cell::new(0u64));
    let bc_setup = bind_counter.clone();
    factory.connect_setup(move |_f, li| {
        let li = li.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let box_ = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        for _ in 0..6 {
            let l = gtk4::Label::new(None);
            l.set_hexpand(true);
            box_.append(&l);
        }
        li.set_child(Some(&box_));
        bc_setup.set(bc_setup.get() + 1);
    });
    let bc_bind = bind_counter.clone();
    factory.connect_bind(move |_f, li| {
        let li = li.downcast_ref::<gtk4::ListItem>().expect("a list item");
        let row = li
            .item()
            .expect("an item")
            .downcast::<ProcessRow>()
            .expect("a ProcessRow");
        let p = &row.item().value;
        let box_ = li.child().expect("a child");
        let texts = [
            p.pid.to_string(),
            p.username.clone(),
            p.name.clone(),
            p.cpu_percent_str(),
            p.disk_read_str(),
            p.disk_write_str(),
        ];
        let mut child = box_.first_child();
        for text in texts {
            let next = child.as_ref().and_then(|w| w.next_sibling());
            if let Some(w) = child {
                if let Ok(label) = w.downcast::<gtk4::Label>() {
                    label.set_label(&text);
                }
            }
            child = next;
        }
        bc_bind.set(bc_bind.get() + 1);
    });

    let store = ListStore::new::<ProcessRow>();
    for p in 1..=(rows as i32) {
        store.append(&ProcessRow::from_item(&item(p)));
    }
    let selection = gtk4::SingleSelection::new(Some(store.clone()));
    let list = gtk4::ListView::new(Some(selection), Some(factory));
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scroll.set_child(Some(&list));
    scroll.set_hexpand(true);
    scroll.set_vexpand(true);

    let window = gtk4::Window::new();
    window.set_default_size(900, 600);
    window.set_child(Some(&scroll));
    window.set_visible(true);
    pump_frame(); // let it allocate + first paint

    UiHarness {
        _window: window,
        store,
        scroll,
        binds: bind_counter,
    }
}

fn ui_level() {
    if gtk4::gdk::Display::default().is_none() {
        eprintln!("UI level skipped: no display available");
        return;
    }
    let rows: usize = std::env::var("BENCH_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let iters = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    let active = rows.div_ceil(7);
    let mk = |round: usize| -> Vec<ProcessItem> {
        let mut items: Vec<ProcessItem> = (1..=rows as i32).map(item).collect();
        for i in 0..active {
            let r = (i * 13 + round) % active;
            set_cpu(&mut items[(rows - 1 - r) as usize], (r + 1) as f64 * 3.5);
        }
        if round % 2 == 1 {
            items.rotate_left(2);
        }
        items
    };

    // (label, strat, restore scroll like the app does)
    let cases: Vec<(&str, Strategy, bool)> = vec![
        ("RebuildAll (buggy)", Strategy::RebuildAll, false),
        ("RebuildAll (+restore)", Strategy::RebuildAll, true),
        ("InPlace (no restore)", Strategy::InPlace, false),
    ];

    for (label, strat, restore) in cases {
        let h = ui_harness(rows);
        let adj = h.scroll.vadjustment();
        let mut waited = 0u32;
        while adj.upper() <= 0.0 && waited < 200 {
            pump_frame();
            std::thread::sleep(Duration::from_millis(2));
            waited += 1;
        }
        if adj.upper() <= 0.0 {
            eprintln!("    [diag] list never laid out (upper=0) — skipping case");
            continue;
        }
        let saved = adj.upper() as f64 * 0.5;
        pin_scroll(&adj, saved); // user has scrolled to the middle
        eprintln!(
            "    [setup] upper={:.0} saved={saved:.0} value_after_pin={:.0}",
            adj.upper(),
            adj.value()
        );

        let mut items = mk(0);
        let mut total = std::time::Duration::ZERO;
        let bind0 = h.binds.get();
        for i in 1..=iters {
            let next = mk(i);
            items = next;
            let t = Instant::now();
            strat.apply(&h.store, &items);
            pump_frame();
            if restore {
                pin_scroll(&adj, saved);
            }
            total += t.elapsed();
        }
        let scroll_end = adj.value();
        let binds = (h.binds.get() - bind0) as f64 / iters as f64;
        // End-state sanity: first row matches.
        let got = h
            .store
            .item(0)
            .and_then(|o| o.downcast::<ProcessRow>().ok())
            .expect("row0")
            .item()
            .pid;
        assert_eq!(got, items.first().expect("first").pid, "{label} end-state");
        println!(
            "\n=== rows={rows}  {label}  (UI level: apply + repaint, {iters} iters) ===\n\
             \u{2003} {:>8.2} us/op   binds/op {:>5.1}   scroll_end={:.0}",
            total.as_nanos() as f64 / iters as f64 / 1000.0,
            binds,
            scroll_end,
        );
    }
}

fn main() {
    // The store-level section does not require a display; the UI level uses
    // it when one is available. gtk4::init is harmless without a display and
    // is only needed for the UI section.
    let init = gtk4::init();
    if init.is_err() {
        eprintln!("gtk4::init failed (display unavailable) — store level only");
    }
    store_level();
    ui_level();
}
