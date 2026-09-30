//! End-to-end gate for the `Escape` deselect feature (spec D7): drives the
//! real main window built by [`ui::build_window`] with a **real Escape key
//! event** (`xdotool` key injection on a virtual X display) and asserts the
//! row selection is cleared and the detail pane hides.
//!
//! This exercises the exact live wiring — window-level `EventControllerKey`,
//! event propagation, selection -> detail callback chain — without guessing
//! at it. It skips transparently when no `DISPLAY`/`xdotool` is present, so
//! plain `cargo test` stays green anywhere. To run it for real:
//!
//! ```sh
//! Xvfb :99 -screen 0 1280x800x24 &
//! DISPLAY=:99 cargo test --test escape_e2e -- --nocapture
//! ```

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use gtk4::prelude::*;
use simpletaskmgr::process::{ProcessItem, TaskMgrProcess};
use simpletaskmgr::process_view::ViewRow;
use simpletaskmgr::ui::{build_window, State};

/// A minimal process row for seeding the list.
fn item(pid: i32) -> ProcessItem {
    let p = TaskMgrProcess::new(format!("proc-{pid}"), pid, 1000, "paul".to_string(), 0.5);
    ProcessItem::new(&p)
}

/// Depth-first collection of all descendants (incl. self) downcast to `W`.
///
/// gtk4-rs 0.11.x exposes widget children through `observe_children()` (a
/// `gio::ListModel` of `Widget` objects), not a `children()` iterator.
fn collect<W: IsA<gtk4::Widget>>(w: &gtk4::Widget) -> Vec<W> {
    let mut out = Vec::new();
    if let Some(t) = w.downcast_ref::<W>() {
        out.push(t.clone());
    }
    let children = w.observe_children();
    for i in 0..children.n_items() {
        if let Some(child) = children.item(i) {
            if let Ok(ow) = child.downcast::<gtk4::Widget>() {
                out.extend(collect::<W>(&ow));
            }
        }
    }
    out
}

/// First descendant (depth-first) downcast to `W`, if any.
fn find_downcast<W: IsA<gtk4::Widget>>(w: &gtk4::Widget) -> Option<W> {
    collect::<W>(w).into_iter().next()
}

type Job = (
    Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>,
    mpsc::Sender<Box<dyn Any + Send>>,
);

static WORKER: std::sync::Mutex<Option<mpsc::Sender<Job>>> = std::sync::Mutex::new(None);

/// Run `f` on the process's single GTK worker thread.
fn run_gtk<R>(f: impl FnOnce() -> R + Send + 'static) -> R
where
    R: Send + 'static,
{
    let tx = {
        let mut guard = WORKER.lock().unwrap();
        match guard.as_ref() {
            Some(tx) => tx.clone(),
            None => {
                let (tx, rx) = mpsc::channel::<Job>();
                thread::Builder::new()
                    .name("escape-e2e-gtk".into())
                    .spawn(move || {
                        gtk4::init().expect("GTK available in the test env");
                        for (work, reply) in rx {
                            reply.send(work()).expect("recv side alive");
                        }
                    })
                    .expect("spawning the GTK worker thread");
                let _ = guard.insert(tx.clone());
                tx
            }
        }
    };
    let (reply_tx, reply_rx) = mpsc::channel::<Box<dyn Any + Send>>();
    tx.send((Box::new(move || Box::new(f())), reply_tx))
        .expect("GTK worker alive");
    *reply_rx
        .recv()
        .expect("GTK worker replies")
        .downcast::<R>()
        .expect("job returns R")
}

#[test]
fn test_escape_key_deselects_in_live_window() {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let have_xdotool = std::process::Command::new("xdotool")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if display.is_empty() || !have_xdotool {
        eprintln!("skip: needs a DISPLAY and xdotool (run under Xvfb)");
        return;
    }

    // Force X11 *before* any GTK init in this process, so the test window
    // lands on the virtual DISPLAY (the default session here is Wayland,
    // where xdotool cannot see or touch the window).
    std::env::set_var("GDK_BACKEND", "x11");

    let report = run_gtk(|| {
        let app = gtk4::Application::new(
            Some("org.simpletaskmgr.esc-e2e"),
            gtk4::gio::ApplicationFlags::default(),
        );
        let report = Rc::new(RefCell::new(String::new()));
        let rep_closure = report.clone();
        let done = Rc::new(Cell::new(false));
        let done_activate = done.clone();
        let (xdotool_tx, xdotool_rx) = mpsc::channel::<String>();
        let xdotool_spawn_tx = xdotool_tx.clone();

        {
            // scope: window + view live for the whole pump below
            let state = Rc::new(RefCell::new(State::new()));
            let (window, _sw) = build_window(&app, &state);
            let root: gtk4::Widget = window.clone().upcast();
            let Some(col_view) = find_downcast::<gtk4::ColumnView>(&root) else {
                *rep_closure.borrow_mut() = "no ColumnView found".into();
                done_activate.set(true);
                return String::new();
            };
            // Model chain: ListStore -> SortListModel -> SingleSelection -> ColumnView.
            let sel: gtk4::SingleSelection = col_view
                .model()
                .expect("the view has a selection model")
                .downcast()
                .expect("the view's model is the SingleSelection");
            let sorted: gtk4::SortListModel = sel
                .model()
                .expect("the selection wraps a model")
                .downcast()
                .expect("the selection's model is the SortListModel");
            let store: gtk4::gio::ListStore = sorted
                .model()
                .expect("the sort model wraps a model")
                .downcast()
                .expect("the sort model's model is the ListStore");
            let panes = Rc::new(collect::<gtk4::ScrolledWindow>(&root));

            window.present();

            let state2 = state.clone();
            let sel2 = sel.clone();
            let store2 = store.clone();
            let win2 = window.clone();
            let report2 = rep_closure.clone();
            let panes2 = panes.clone();
            let pane = Rc::new(RefCell::new(Option::<gtk4::ScrolledWindow>::None));
            let step = Rc::new(Cell::new(0u32));
            let seeded = Rc::new(Cell::new(Option::<i32>::None));
            let done_timeout = done_activate.clone();

            let _id = glib::timeout_add_local(Duration::from_millis(250), move || {
                let n = step.get();
                if n == 0 {
                    // Seed the list store directly and select a row.
                    for pid in [1101, 2202, 3303] {
                        store2.append(&ViewRow::from_item(&item(pid)));
                    }
                    let count = store2.n_items();
                    if count > 0 {
                        sel2.set_selected(1);
                    }
                    seeded.set(state2.borrow().selected_pid);
                    // The detail pane is the ScrolledWindow that does *not*
                    // host the ColumnView (the other one is the always-visible
                    // list pane).
                    let mut pane_set = false;
                    for sw in panes2.iter() {
                        let w: &gtk4::Widget = sw.upcast_ref();
                        if find_downcast::<gtk4::ColumnView>(w).is_none() {
                            *pane.borrow_mut() = Some(sw.clone());
                            pane_set = true;
                            break;
                        }
                    }
                    if !pane_set {
                        *pane.borrow_mut() = panes2.first().cloned();
                    }
                    // Fire a *real* Escape key at the window, sending the
                    // xdotool diagnostics back over a channel so a silent miss
                    // is visible in the report.
                    let xdotool_spawn_tx2 = xdotool_spawn_tx.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(1500));
                        fn run(name: &mut String, label: &str, args: &[&str]) {
                            let out = std::process::Command::new("xdotool").args(args).output();
                            match out {
                                Ok(o) => name.push_str(&format!(
                                    "[{}] exit={:?} stderr={}\n",
                                    label,
                                    o.status.code(),
                                    String::from_utf8_lossy(&o.stderr).trim()
                                )),
                                Err(e) => name.push_str(&format!("[{}] error {e:?}\n", label)),
                            }
                        }
                        let mut d = String::new();
                        let search = std::process::Command::new("xdotool")
                            .args(["search", "--name", "Simple Task Manager"])
                            .output()
                            .ok();
                        let wid = String::from_utf8_lossy(
                            &search
                                .as_ref()
                                .map(|o| o.stdout.clone())
                                .unwrap_or_default(),
                        )
                        .trim()
                        .to_string();
                        d.push_str(&format!(
                            "[search] exit={:?} windows={:?}\n",
                            search.as_ref().map(|o| o.status.code()),
                            wid
                        ));
                        run(
                            &mut d,
                            "windowfocus-chain",
                            &[
                                "search",
                                "--name",
                                "Simple Task Manager",
                                "windowfocus",
                                "--sync",
                            ],
                        );
                        thread::sleep(Duration::from_millis(200));
                        run(
                            &mut d,
                            "key-chain",
                            &[
                                "search",
                                "--name",
                                "Simple Task Manager",
                                "key",
                                "--clearmodifiers",
                                "Escape",
                            ],
                        );
                        if !wid.is_empty() {
                            run(
                                &mut d,
                                "key---window",
                                &["key", "--window", &wid, "--clearmodifiers", "Escape"],
                            );
                        }
                        let _ = xdotool_spawn_tx2.send(d);
                    });
                    step.set(1);
                    return glib::ControlFlow::Continue;
                }
                // Poll for the deselect to land.
                let done = state2.borrow().selected_pid.is_none() && sel2.selected_item().is_none();
                if (done && n >= 2) || n >= 18 {
                    // detail_pane_hidden: the detail ScrolledWindow must be
                    // invisible after the Escape deselect.
                    let detail_pane_hidden = pane
                        .borrow()
                        .as_ref()
                        .map(|p| !p.is_visible())
                        .unwrap_or(false);
                    let title = win2.title().unwrap_or_default();
                    *report2.borrow_mut() = format!(
                        "seeded={:?} deselected={done} detail_pane_hidden={detail_pane_hidden} dbg_title={title}",
                        seeded.get()
                    );
                    done_timeout.set(true);
                    return glib::ControlFlow::Break;
                }
                step.set(n + 1);
                glib::ControlFlow::Continue
            });
        }

        // `Application::run` would parse the *test* harness's os::args (e.g.
        // `--nocapture`) as GTK application CLI options and abort, so skip
        // the app's run loop entirely: pump the main context ourselves
        // until the scenario reports completion. `app` is kept alive by the
        // enclosing scope for the duration of the pump.
        let ctx = glib::MainContext::default();
        let d = done.clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !d.get() && std::time::Instant::now() < deadline {
            ctx.iteration(false);
            thread::sleep(Duration::from_millis(10));
        }
        let x = {
            let mut r = report.borrow().clone();
            if let Ok(d) = xdotool_rx.recv_timeout(Duration::from_secs(2)) {
                r.push_str(&d);
            }
            r
        };
        x
    });

    let seeded_ok = report.contains("seeded=Some");
    let deselected_ok = report.contains("deselected=true");
    let pane_ok = report.contains("detail_pane_hidden=true");
    assert!(
        seeded_ok,
        "row was selected before the Escape key (setup failed): {report}"
    );
    assert!(
        deselected_ok && pane_ok,
        "Escape did not deselect / hide the pane: {report}"
    );
    eprintln!("escape-e2e: {report}");
}
