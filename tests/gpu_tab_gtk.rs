//! The GPU tab must appear in an **already open** window when a GPU becomes
//! readable after startup.
//!
//! The case this pins down: a hybrid laptop whose discrete card is
//! runtime-PM-suspended, unbound, or passed through to a VM at launch. The
//! startup probe then finds no readable card, `gpu_available` starts
//! `false`, and the tab list is built without a GPU page. `State::refresh_gpu`
//! re-probes on a cadence, and the refresh closure inserts the page into the
//! live `GtkStack` the tick a card answers (`ui::ensure_gpu_tab`) — no
//! restart, and no second tab on the ticks after.
//!
//! Same single-threaded GTK harness as `tests/process_view_gtk.rs`: GTK is
//! initialized once, on one long-lived worker thread, and every widget is
//! created, driven and asserted inside one `run_gtk` closure.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, mpsc::Sender, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use simpletaskmgr::process::{ProcessItem, TaskMgrProcess};
use simpletaskmgr::ui::{build_window, ensure_gpu_tab, State};

/// A job for the GTK worker: runs and yields the result or the panic payload.
#[allow(clippy::type_complexity)]
type Job = (
    Box<dyn FnOnce() -> Result<Box<dyn Any + Send>, Box<dyn Any + Send>> + Send>,
    Sender<Result<Box<dyn Any + Send>, Box<dyn Any + Send>>>,
);

static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

/// Run `f` on the process's dedicated GTK worker thread (initialized once, on
/// first call) and return its result. A panic inside `f` is re-thrown at the
/// call site.
fn run_gtk<R>(f: impl FnOnce() -> R + std::marker::Send + 'static) -> R
where
    R: std::marker::Send + 'static,
{
    let worker_tx = {
        let mut guard = WORKER.lock().unwrap();
        match guard.as_ref() {
            Some(tx) => tx.clone(),
            None => {
                let (tx, rx) = mpsc::channel::<Job>();
                thread::Builder::new()
                    .name("gpu-tab-gtk".into())
                    .spawn(move || {
                        gtk4::init().expect("GTK available in the test env");
                        for (work, reply) in rx {
                            let _ = reply.send(work());
                        }
                    })
                    .expect("spawning the GTK worker thread");
                let _ = guard.insert(tx.clone());
                tx
            }
        }
    };
    let (reply_tx, reply_rx) = mpsc::channel();
    worker_tx
        .send((
            Box::new(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                    .map(|r| Box::new(r) as Box<dyn Any + Send>)
                    .map_err(|payload| payload as Box<dyn Any + Send>)
            }),
            reply_tx,
        ))
        .expect("the GTK worker is alive");
    match reply_rx.recv() {
        Ok(Ok(r)) => *r.downcast::<R>().expect("the result type matches"),
        Ok(Err(payload)) => std::panic::resume_unwind(payload),
        Err(_) => panic!("the GTK worker died"),
    }
}

/// A minimal process row for seeding a fixture state.
fn item(pid: i32) -> ProcessItem {
    let p = TaskMgrProcess::new(format!("proc-{pid}"), pid, 1000, "paul".to_string(), 0.5);
    ProcessItem::new(&p)
}

/// A throwaway settings path, so a test never touches `~/.config`.
fn temp_settings(tag: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("stgm_gpu_tab_{}_{}.toml", std::process::id(), tag));
    let _ = std::fs::remove_file(&path);
    path
}

/// The tab titles currently in `stack`, in insertion order.
fn page_titles(stack: &gtk4::Stack) -> Vec<String> {
    let model = stack.pages();
    (0..model.n_items())
        .filter_map(|i| model.item(i))
        .filter_map(|i| i.downcast::<gtk4::StackPage>().ok())
        .map(|p| p.title().map(|t| t.to_string()).unwrap_or_default())
        .collect()
}

/// Depth-first collection of all descendants (incl. self) downcast to `W`
/// (gtk4-rs 0.11.x exposes children through `observe_children()`).
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

/// The tab-insertion step itself: nothing before a GPU is readable, exactly
/// one page when it becomes readable, and idempotence on every tick after —
/// a card that keeps answering must not stack up GPU tabs. The pane's two
/// drawing areas must join the tick's redraw list, and the user's visible tab
/// must be left alone.
#[test]
fn test_ensure_gpu_tab_inserts_once_when_a_gpu_appears() {
    let (before, after, again, graphs_len, visible) = run_gtk(|| {
        let path = temp_settings("insert_once");
        let state = Rc::new(RefCell::new(State::with_processes(
            path.clone(),
            vec![item(10)],
        )));
        assert!(
            !state.borrow().gpu_available,
            "the fixture state starts with no readable GPU"
        );

        let stack = gtk4::Stack::new();
        let cpu_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        stack.add_titled(&cpu_row, Some("cpu"), "CPU / Mem");
        stack.set_visible_child_name("cpu");
        let graphs = Rc::new(RefCell::new(Vec::<gtk4::DrawingArea>::new()));
        let added = Cell::new(false);

        // No GPU readable: the tab list stays as it was built.
        assert!(
            !ensure_gpu_tab(&state, &stack, &graphs, &added),
            "nothing is added while no GPU is readable"
        );
        let before = page_titles(&stack);

        // The throttled re-probe hits: `State::refresh_gpu` flips the flag.
        state.borrow_mut().gpu_available = true;
        assert!(
            ensure_gpu_tab(&state, &stack, &graphs, &added),
            "the tab appears the tick a GPU becomes readable"
        );
        let after = page_titles(&stack);
        let graphs_len = graphs.borrow().len();
        let visible = stack.visible_child_name().map(|s| s.to_string());

        // Every later tick: idempotent.
        assert!(
            !ensure_gpu_tab(&state, &stack, &graphs, &added),
            "a card that keeps answering adds no second tab"
        );
        let again = page_titles(&stack);

        let _ = std::fs::remove_file(&path);
        (before, after, again, graphs_len, visible)
    });

    assert_eq!(before, vec!["CPU / Mem"], "no GPU, no GPU tab");
    assert_eq!(after, vec!["CPU / Mem", "GPU"], "the GPU tab joined");
    assert_eq!(again, vec!["CPU / Mem", "GPU"], "idempotent after");
    assert_eq!(
        graphs_len, 2,
        "both GPU panes joined the tick's redraw list"
    );
    assert_eq!(
        visible.as_deref(),
        Some("cpu"),
        "the user keeps the tab they were looking at"
    );
}

/// The production wiring, driven by the real timer: a window built with no
/// readable GPU grows its GPU tab on a later tick, with no restart and no
/// window rebuild. The ticks are real `glib` timeouts firing the real
/// `State::refresh` (live `/proc` walk included), exactly as the running app
/// does.
#[test]
fn test_gpu_tab_appears_in_a_live_window_on_a_later_tick() {
    let (before, after, ticks) = run_gtk(|| {
        let app = gtk4::Application::builder().build();
        let path = temp_settings("live_window");
        let state = Rc::new(RefCell::new(State::with_processes(
            path.clone(),
            vec![item(10), item(20)],
        )));
        let (window, _settings) = build_window(&app, &state);

        let stack = collect::<gtk4::Stack>(window.upcast_ref())
            .into_iter()
            .next()
            .expect("the built window has the graph stack");
        let before = page_titles(&stack);

        // The card wakes up: the state flips on the cadence tick in the real
        // app; here the flip is the same field write, and the running timer
        // does the insertion.
        state.borrow_mut().gpu_available = true;
        let start = state.borrow().refresh_count;
        // The worker thread blocks on its job queue, so spin the main loop
        // here and let the 1.5 s timer source fire (two ticks in 3.2 s).
        let deadline = Instant::now() + Duration::from_millis(3_200);
        while Instant::now() < deadline {
            let _ = glib::MainContext::default().iteration(false);
            std::thread::sleep(Duration::from_millis(10));
        }

        let after = page_titles(&stack);
        let ticks = state.borrow().refresh_count - start;
        drop(window);
        let _ = std::fs::remove_file(&path);
        (before, after, ticks)
    });

    assert_eq!(
        before,
        vec!["CPU / Mem", "Disk I/O"],
        "a window built with no readable GPU has no GPU tab"
    );
    assert_eq!(
        after,
        vec!["CPU / Mem", "Disk I/O", "GPU"],
        "the GPU tab appeared in the open window, with no restart"
    );
    assert!(
        ticks >= 1,
        "a real refresh tick (not a rebuild) inserted it; saw {ticks}"
    );
}
