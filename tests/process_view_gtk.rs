//! Widget-level tests for the process view (`src/process_view.rs`).
//!
//! GTK must be initialized exactly once per process, on one thread, and its
//! main context stays owned by that thread for the process lifetime. libtest
//! runs each test on a fresh thread, so **all** GTK work in this binary is
//! funneled through one long-lived worker thread (`run_gtk` below): each test
//! closure creates everything it needs, asserts locally, and returns plain
//! data — the GTK objects never cross threads. Keep each test self-contained
//! for that reason.
//!
//! This is its own test binary, so its GTK state is isolated from the lib's
//! unit tests. Run with the usual `cargo test`.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, mpsc::Sender, Mutex};
use std::thread;

use gtk4::prelude::*;
use simpletaskmgr::process::{ProcessItem, TaskMgrProcess};
use simpletaskmgr::process_view::{ProcessView, ViewRow};
use simpletaskmgr::signal::Signal;
use simpletaskmgr::SortColumn;

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
                    .name("process-view-gtk".into())
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

/// A `ProcessItem` whose CPU percent renders at the tracker's scale
/// (`cpu_ticks / 100.0`), e.g. 10 ticks -> "0.1%", 11 ticks -> "0.1%"
/// (same displayed text).
fn item(pid: i32, cpu_ticks: u64) -> ProcessItem {
    itemp(pid, cpu_ticks as f64 / 100.0, cpu_ticks)
}

/// A `ProcessItem` with an explicit CPU percent and ticks.
fn itemp(pid: i32, cpu_percent: f64, cpu_ticks: u64) -> ProcessItem {
    let mut p = TaskMgrProcess::new(
        "procname".to_string(),
        pid,
        1000,
        "paul".to_string(),
        cpu_percent,
    );
    p.cpu_ticks = cpu_ticks;
    ProcessItem::new(&p)
}

#[test]
fn test_row_roundtrip_and_identity() {
    run_gtk(|| {
        let it = item(123, 42);
        let r = ViewRow::from_item(&it);
        assert_eq!(r.item(), it);
        assert_eq!(r.pid(), 123);
        assert!(r.has_value(&it));
        let other = item(999, 42);
        assert!(!r.has_value(&other));
    });
}

#[test]
fn test_row_properties_mirror_data() {
    run_gtk(|| {
        let r = ViewRow::from_item(&itemp(123, 10.0, 4_294_967));
        let pid: String = r.property("pid");
        let name: String = r.property("name");
        let cpu: String = r.property("cpu");
        let mem: String = r.property("mem");
        let rd: String = r.property("disk-read");
        let wr: String = r.property("disk-write");
        assert_eq!(pid, "123");
        assert_eq!(name, "procname");
        assert_eq!(cpu, "10.0%");
        assert_eq!(mem, "", "unknown MEM% shows blank");
        assert_eq!(rd, "", "unknown disk rate shows blank");
        assert_eq!(wr, "", "unknown disk rate shows blank");
    });
}

#[test]
fn test_cell_label_truncates_long_names() {
    // A multi-hundred-character `argv[0]` (e.g. a Slack sandbox process)
    // used to stretch the expandable Name column — and hence the whole
    // window — to its full text width. The cell label must ellipsize and
    // cap its natural width request so the view truncates long names
    // instead of growing to fit them (the column stays resizable, and the
    // detail pane shows the full command line).
    run_gtk(|| {
        let label = gtk4::Label::new(Some(&"x".repeat(600)));
        simpletaskmgr::process_view::style_cell_label(&label);
        assert_eq!(
            label.ellipsize(),
            gtk4::pango::EllipsizeMode::End,
            "long names must be truncated in the display"
        );
        assert!(
            label.max_width_chars() > 0,
            "the natural width request must be capped so the window stays a reasonable size"
        );
    });
}

#[test]
fn test_row_notify_only_changed() {
    run_gtk(|| {
        let r = ViewRow::from_item(&item(1, 10));
        let fired = Rc::new(RefCell::new(Vec::<i32>::new()));
        let f = fired.clone();
        r.connect_notify_local(Some("cpu"), move |_row, _| {
            f.borrow_mut().push(1);
        });

        // Different ticks, same displayed percentage -> no notify at all.
        let same_display = item(1, 11); // still "1.0%" at 100 ms scale
        assert_ne!(r.item(), same_display);
        r.set_item(&same_display);
        assert!(fired.borrow().is_empty(), "same text must not notify");

        let other = itemp(1, 90.0, 90_000);
        r.set_item(&other);
        assert!(!fired.borrow().is_empty(), "changed text must notify");
        let n = fired.borrow().len();

        // Re-setting the same value -> no extra notify.
        r.set_item(&other);
        assert_eq!(fired.borrow().len(), n);
    });
}

/// A `ProcessItem` with a disk read rate (the fixture's default is `None`).
fn with_disk_read(mut it: ProcessItem, speed: f64) -> ProcessItem {
    it.value.disk_read_speed = Some(speed);
    it
}
/// A `ProcessItem` with a disk write rate (the fixture's default is `None`).
fn with_disk_write(mut it: ProcessItem, speed: f64) -> ProcessItem {
    it.value.disk_write_speed = Some(speed);
    it
}

/// Blank (missing) disk rates stay at the *bottom* of the list in **both**
/// sort directions (spec S3) — the header click negates the comparator, so
/// the comparator itself must un-negate the missing-value ranking.
#[test]
fn test_blank_disk_rows_last_both_directions() {
    run_gtk(|| {
        let pv = ProcessView::new();
        let blank = item(1, 0);
        let read10 = with_disk_read(item(2, 0), 10_000.0);
        let read20 = with_disk_read(item(3, 0), 20_000.0);
        let w1 = with_disk_write(item(2, 0), 5_000.0);
        let w2 = with_disk_write(item(3, 0), 9_000.0);
        pv.update(&[blank.clone(), read10.clone(), read20.clone()]);

        pv.sort_like_click(SortColumn::DiskRead, gtk4::SortType::Ascending);
        assert_eq!(pv.display_order(), vec![2, 3, 1], "blank last (Disk R asc)");

        pv.sort_like_click(SortColumn::DiskRead, gtk4::SortType::Descending);
        assert_eq!(
            pv.display_order(),
            vec![3, 2, 1],
            "blank last (Disk R desc)"
        );

        // Same contract for the write column.
        pv.update(&[blank, w1, w2]);
        pv.sort_like_click(SortColumn::DiskWrite, gtk4::SortType::Ascending);
        assert_eq!(pv.display_order(), vec![2, 3, 1], "blank last (Disk W asc)");

        pv.sort_like_click(SortColumn::DiskWrite, gtk4::SortType::Descending);
        assert_eq!(
            pv.display_order(),
            vec![3, 2, 1],
            "blank last (Disk W desc)"
        );
    });
}

/// Full lifecycle of the GTK-side algorithm (spec D1, S7, V, R4, D2):
/// default CPU% descending, header re-click is a no-op commit, in-place
/// value refresh (visually free, row objects stable), reorder refresh (one
/// commit, rows survive), selection follow-by-PID with reentrant callbacks,
/// and drop-selection on removal.
#[test]
fn test_process_view_refresh_selection_detail() {
    run_gtk(|| {
        let pv = ProcessView::new();

        // Selection events, as the app would observe them. These may fire
        // reentrantly from within `update` (spec B2); pushing into an
        // `Rc<RefCell>` shared with the closure is exactly that.
        let events = Rc::new(RefCell::new(Vec::<Option<i32>>::new()));
        let ev = events.clone();
        pv.connect_selection_changed(move |sel| {
            ev.borrow_mut().push(sel);
        });

        // S7: the view opens sorted by CPU% descending (top-style), no
        // initial selection, pane hidden. (Distinct CPU ticks keep the
        // expected order deterministic.)
        let a = item(100, 60);
        let b = item(200, 30);
        let c = item(300, 50);
        pv.update(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!(
            pv.display_order(),
            vec![100, 300, 200],
            "default sort is CPU% descending (S7)"
        );
        assert!(
            pv.row_of(100).is_some() && pv.row_of(200).is_some() && pv.row_of(300).is_some(),
            "all rows present in the store"
        );
        assert!(pv.selected_pid().is_none(), "no selection at startup (D1)");
        assert!(!pv.detail_labels().pane.is_visible());
        assert!(events.borrow().is_empty(), "no selection callback yet");

        // S2 (header click): clicking the active CPU header again re-sets
        // the same state; GTK commits nothing when the order is unchanged
        // (spec T3) — so the refresh ticks below start from zero commits.
        let commits = Rc::new(Cell::new(0u32));
        let c2 = commits.clone();
        pv.sort_model()
            .connect_items_changed(move |_m, _pos, _rem, _add| {
                c2.set(c2.get() + 1);
            });
        pv.sort_like_click(SortColumn::CpuPercent, gtk4::SortType::Descending);
        assert_eq!(pv.display_order(), vec![100, 300, 200]);
        assert_eq!(
            commits.get(),
            0,
            "redundant click on the active sort: no commit"
        );

        // V: value-only refresh (values change, order does not) is visually
        // free and keeps the row objects.
        let b_same = item(200, 29);
        pv.update(&[a.clone(), b_same.clone(), c.clone()]);
        assert_eq!(pv.display_order(), vec![100, 300, 200]);
        assert_eq!(commits.get(), 0, "value-only refresh commits nothing");

        // R: a refresh that changes order commits exactly once, and each
        // surviving process keeps its row object (the display is the same set
        // of rows reordered, not a rebuild).
        let b2 = item(200, 70);
        let before_b = pv.row_of(200).expect("row 200");
        pv.update(&[a.clone(), b2.clone(), c.clone()]);
        assert_eq!(pv.display_order(), vec![200, 100, 300]);
        assert_eq!(commits.get(), 1, "one commit for the reorder");
        let after_b = pv.row_of(200).expect("row 200");
        assert!(
            before_b == after_b,
            "surviving rows keep their object identity"
        );

        // R4/D2: selection follows its PID through refreshes and reorders,
        // with the callback firing on every real change.
        pv.selection().set_selected(0); // now pid 200 (top row)
        assert_eq!(pv.selected_pid(), Some(200));
        assert!(
            *events.borrow().last().expect("select fired the callback") == Some(200),
            "select fires the callback"
        );
        assert!(pv.detail_labels().pane.is_visible());

        // Re-pin: pid 200 drops out of the top after a refresh — the
        // selection rides the process, not the slot (spec R4).
        pv.selection().set_selected(
            pv.display_order()
                .iter()
                .position(|p| *p == 200)
                .expect("row 200 is visible") as u32,
        );
        let a2 = item(100, 80);
        let c2item = item(300, 90);
        events.borrow_mut().clear();
        pv.update(&[a2.clone(), b2.clone(), c2item.clone()]);
        assert_eq!(pv.display_order(), vec![300, 100, 200]);
        assert_eq!(
            pv.selected_pid(),
            Some(200),
            "selection must follow its PID"
        );
        assert!(
            *events
                .borrow()
                .last()
                .expect("the re-pin fired the callback")
                == Some(200),
            "re-pinned selection fires the callback"
        );

        // D2/D-fall: deselect via the sentinel; then the selected PID
        // leaves the list — the selection clears, the pane hides, and a
        // `None` event fires (spec B2 reentrancy during `update`).
        pv.selection().set_selected(u32::MAX);
        assert!(pv.selected_pid().is_none(), "sentinel clears the selection");
        assert!(
            events
                .borrow()
                .last()
                .expect("the deselect fired the callback")
                .is_none(),
            "deselect fires the callback"
        );
        assert!(!pv.detail_labels().pane.is_visible());

        // Select, then have that very PID removed.
        pv.selection().set_selected(0); // pid 300
        assert_eq!(pv.selected_pid(), Some(300));
        events.borrow_mut().clear();
        pv.update(&[a2.clone(), b2.clone()]);
        assert_eq!(pv.display_order(), vec![100, 200]);
        assert!(pv.selected_pid().is_none(), "gone PID deselects");
        assert!(
            events
                .borrow()
                .last()
                .expect("the deselect fired the callback")
                .is_none(),
            "deselect fires the callback (B2 reentrancy during update)"
        );
        assert!(!pv.detail_labels().pane.is_visible());
    });
}

/// Refresh cadence (guards a past double-refresh regression): with the real
/// production wiring — `ui::build_window`, including the app's own 1.5 s
/// `glib::timeout` — the initial build performs exactly one
/// `State::refresh`, and a 3.2 s window sees exactly the two scheduled
/// ticks. A second refresh per tick (timer pre-refresh + rebuild refresh)
/// shrinks the CPU-delta window to the first refresh's own duration, which
/// inflates every reported %CPU (the app measures itself 4x its real usage
/// and idle processes read 0%).
#[test]
fn test_refresh_cadence_one_per_tick() {
    let n = run_gtk(|| {
        let app = gtk4::Application::builder().build();
        let path = std::env::temp_dir().join(format!("stgm_cadence_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let state = Rc::new(RefCell::new(simpletaskmgr::ui::State::with_settings_path(
            path.clone(),
        )));
        let window = simpletaskmgr::ui::build_window(&app, &state);
        assert_eq!(
            state.borrow().refresh_count,
            1,
            "the initial build performs exactly one refresh"
        );
        // The worker loop blocks on its job queue, so spin the main loop here
        // until 3.2 s elapse, letting the 1.5 s timer source fire (two ticks).
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(3_200);
        while std::time::Instant::now() < deadline {
            let _ = glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let n = state.borrow().refresh_count - 1;
        drop(window);
        let _ = std::fs::remove_file(&path);
        n
    });
    assert!(
        (1..=2).contains(&n),
        "3.2 s holds two 1.5 s ticks; saw {n} refreshes (a double refresh per tick reads 4+)",
    );
}

/// S8: a header click (new column, direction flip, or even a redundant re-click)
/// settles the sort through the `ColumnViewSorter`'s `primary-sort-column` /
/// `primary-sort-order` properties, and the view scrolls to the top on each —
/// so the new first row is immediately visible. The one thing that must NOT
/// trigger that scroll is a refresh tick: an update's `Sorter::changed`
/// invalidation fires neither property, so the user's scroll position survives
/// (R3). The observables are the S8 counters: `sort_change_count` bumps when
/// a real sort change *schedules* the top-scroll, and `scroll_done_count`
/// when the deferred scroll actually *runs* (its idle fires — after the
/// re-sort commit) — a tick must move neither.
#[test]
fn test_sort_change_scrolls_to_top_but_ticks_do_not() {
    run_gtk(|| {
        let pv = ProcessView::new();
        let a = item(1, 100);
        let b = item(2, 200);
        let c = item(3, 300);
        pv.update(&[a, b.clone(), c.clone()]);
        assert_eq!(pv.display_order(), vec![3, 2, 1], "CPU% desc at startup");

        // Switching the active column is a genuine sort change -> schedule.
        let sched = pv.sort_change_count();
        let done = pv.scroll_done_count();
        pv.sort_like_click(SortColumn::MemPercent, gtk4::SortType::Descending);
        assert!(
            pv.sort_change_count() > sched,
            "a header click on a new column must schedule the S8 scroll",
        );
        assert_eq!(
            pv.scroll_done_count(),
            done,
            "the scroll is deferred — still not executed while the click settles",
        );

        // Flipping the direction of the active column is a genuine change too.
        let sched = pv.sort_change_count();
        pv.sort_like_click(SortColumn::MemPercent, gtk4::SortType::Ascending);
        assert!(
            pv.sort_change_count() > sched,
            "a direction flip must schedule the S8 scroll",
        );

        // Run the queued idles: the scheduled scrolls now execute (post-commit)
        // and — and only then — the done counter moves.
        let done = pv.scroll_done_count();
        while glib::MainContext::default().iteration(false) {}
        assert!(
            pv.scroll_done_count() > done,
            "the deferred scrolls run once their idles fire",
        );

        // The critical negative: a refresh tick must NOT schedule (or run)
        // a scroll, or every refresh would yank the user back to the top
        // (R3) — even when the tick genuinely reorders the list. Sort by
        // CPU% again first, so the reorder is observable (the active sort is
        // MEM% asc, where all three rows tie).
        pv.sort_like_click(SortColumn::CpuPercent, gtk4::SortType::Descending);
        while glib::MainContext::default().iteration(false) {}
        let sched = pv.sort_change_count();
        let done = pv.scroll_done_count();
        pv.update(&[item(1, 999), b.clone(), c.clone()]);
        assert_eq!(
            pv.display_order(),
            vec![1, 3, 2],
            "pid 1 strictly heaviest: real reorder committed"
        );
        pv.update(&[item(1, 0), b, c]); // …and back to [3, 2, 1]: another real reorder
        assert_eq!(pv.display_order(), vec![3, 2, 1]);
        assert_eq!(
            pv.sort_change_count(),
            sched,
            "a refresh tick must not schedule the S8 scroll",
        );
        assert_eq!(
            pv.scroll_done_count(),
            done,
            "a refresh tick must not run a pending S8 scroll",
        );
    });
}

#[test]
fn test_signal_buttons_forward_and_status() {
    // Spec D5: the detail-pane buttons carry the requested signal to the
    // app's single callback — the view itself sends no `kill(2)` — and
    // `set_status` writes the pane's feedback line. The buttons ride inside
    // the pane, which is only visible with a selection (D1/D3) — and a
    // button inside an invisible hierarchy does not activate — so select a
    // row first, as a real user would.
    run_gtk(|| {
        let pv = ProcessView::new();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        pv.connect_signal_requested(move |sig| s.borrow_mut().push(sig));

        pv.update(&[item(100, 10), item(200, 20)]);
        pv.selection()
            .set_selected(pv.display_order().iter().position(|p| *p == 100).unwrap() as u32);
        let d = pv.detail_labels();
        assert!(d.pane.is_visible(), "a selection shows the pane");
        assert_eq!(d.terminate.label().as_deref(), Some("Terminate"));
        assert_eq!(d.kill.label().as_deref(), Some("Kill"));

        // Emits "clicked" — the same signal GTK's release path fires on a
        // real pointer click. (`Button::activate` only reaches that path on
        // a *realized* widget — a headless test env never is.)
        d.terminate
            .emit_by_name::<()>("clicked", &[] as &[&dyn glib::value::ToValue]);
        d.kill
            .emit_by_name::<()>("clicked", &[] as &[&dyn glib::value::ToValue]);
        assert_eq!(
            *seen.borrow(),
            vec![Signal::Sigterm, Signal::Sigkill],
            "each button forwards exactly its own signal, in order"
        );

        pv.set_status("SIGTERM failed: Example");
        assert_eq!(d.status.label(), "SIGTERM failed: Example");
        pv.set_status("");
        assert_eq!(d.status.label(), "", "empty string clears the line");

        // Moving the selection re-applies the pane and clears a stale
        // outcome (the status line resets with the data).
        pv.set_status("stale outcome");
        pv.selection()
            .set_selected(pv.display_order().iter().position(|p| *p == 200).unwrap() as u32);
        assert_eq!(
            d.status.label(),
            "",
            "selecting a different row clears the stale status"
        );
    });
}

#[test]
fn test_detail_pane_renders_free_tier_fields() {
    // The detail pane renders the free-tier per-process details from the
    // data a selection carries; unknown fields fall back to the `—`
    // placeholder (spec V4). The Command row stays plain (not selectable):
    // selecting it would make GtkLabel install an exclusive click gesture
    // that suppresses the click-to-reveal toggle (spec D6).
    run_gtk(|| {
        let pv = ProcessView::new();

        // A fully-populated row: every field present so we can assert the
        // exact rendered text of each row.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut p = TaskMgrProcess::new("myapp".to_string(), 1234, 1000, "paul".to_string(), 12.3);
        p.cmdline = Some("myapp --flag value".to_string());
        p.state = 'S';
        p.threads = 4;
        p.nice = 5;
        p.rss_kb = Some(314_572);
        p.start_epoch = Some(now - 5);
        let full = ProcessItem::new(&p);
        pv.update(std::slice::from_ref(&full));
        pv.selection().set_selected(0);

        let d = pv.detail_labels();
        assert!(d.pane.is_visible());
        assert_eq!(d.name.label(), "Name: myapp");
        assert_eq!(d.command.label(), "Command: myapp --flag value");
        assert_eq!(d.state.label(), "State: Sleeping");
        assert_eq!(d.threads.label(), "Threads: 4");
        assert_eq!(d.nice.label(), "Nice: +5", "positive nice signs explicitly");
        assert_eq!(
            d.rss.label(),
            "Memory: 307.2 MiB",
            "314572 KiB renders as MiB"
        );
        assert!(
            d.started.label().starts_with("Started: ") && d.started.label() != "Started: —",
            "a known start time renders (got {:?})",
            d.started.label().as_str()
        );
        assert!(
            d.uptime.label().starts_with("Uptime: ") && d.uptime.label() != "Uptime: —",
            "a known start time renders an age (got {:?})",
            d.uptime.label().as_str()
        );

        // A data-free row (kernel thread / zombie with no cmdline, no RSS,
        // no birth time) renders the `—` placeholder on every unknown field,
        // while the always-present stat fields still show their values.
        let bare = item(5678, 10);
        pv.update(std::slice::from_ref(&bare));
        pv.selection()
            .set_selected(pv.display_order().iter().position(|x| *x == 5678).unwrap() as u32);
        let d = pv.detail_labels();
        assert_eq!(
            d.command.label(),
            "Command: —",
            "absent cmdline -> placeholder"
        );
        assert_eq!(d.threads.label(), "Threads: 0");
        assert_eq!(d.nice.label(), "Nice: 0", "zero nice stays unsigned");
        assert_eq!(d.rss.label(), "Memory: —", "absent RSS -> placeholder");
        assert_eq!(
            d.started.label(),
            "Started: —",
            "absent start -> placeholder"
        );
        assert_eq!(d.uptime.label(), "Uptime: —", "absent start -> no age");
    });
}

#[test]
fn test_command_click_popover_is_the_full_command() {
    // D6: a single click on the (ellipsized) `Command` line reveals the full
    // command in a popover, where it can be selected and copied, without
    // widening the pane. Here we test the wiring the click relies on and the
    // content it shows — the actual pop needs a toplevel window (the popover
    // surface) and is covered by the visual run. The first assertion guards
    // the root cause of "click does nothing": a *selectable* GtkLabel
    // installs its own exclusive click gesture that claims every press and
    // suppresses the toggle gesture, so the row must stay plain (copying
    // happens on the popover's text instead).
    run_gtk(|| {
        let pv = ProcessView::new();
        let long = format!("slack --type=renderer --sandbox-token={}", "q".repeat(400));
        let mut p = TaskMgrProcess::new("slack".to_string(), 4321, 1000, "paul".to_string(), 3.0);
        p.cmdline = Some(long.clone());
        let full = ProcessItem::new(&p);
        pv.update(std::slice::from_ref(&full));
        pv.selection().set_selected(0);

        let d = pv.detail_labels();
        assert!(
            !d.command.is_selectable(),
            "a selectable Command row would suppress the click-to-open gesture (D6)"
        );

        // The popover is anchored to the `Command` line — a click on that
        // row opens it (and no other row does).
        let anchor = d
            .command_popover
            .parent()
            .expect("the popover has an anchor");
        assert!(
            std::ptr::eq(
                anchor.as_ptr() as *const gtk4::Widget,
                d.command.as_ptr() as *const gtk4::Widget,
            ),
            "the full-command popover must anchor on the Command line"
        );

        // What gets revealed is the selected process' *full* command line
        // (not the ellipsized preview): the very long token is intact.
        assert_eq!(pv.selected_command_line(), long);

        // The content lives in a read-only, selectable text view (copyable),
        // wrapping very long tokens mid-word so it cannot widen the pane.
        let content = d.command_popover.child().expect("the popover has content");
        let scroll = content
            .downcast::<gtk4::ScrolledWindow>()
            .expect("the content is horizontally scrollable");
        let tv = scroll
            .child()
            .expect("a text view holds the command")
            .downcast::<gtk4::TextView>()
            .expect("that view is a text view");
        assert!(!tv.is_editable(), "the revealed command is read-only");
        assert_eq!(
            tv.wrap_mode(),
            gtk4::WrapMode::WordChar,
            "a very long token must wrap, not widen the popover"
        );
    });
}

#[test]
fn test_detail_pane_ellipsizes_long_command_tokens() {
    // A Slack/Chromium command line carries single tokens hundreds of
    // characters long (e.g. `--enable-features=…`). An unstyled label would
    // request as much width as its longest token, and the detail pane would
    // stretch the whole window off-screen. The pane's value labels must
    // therefore ellipsize to a bounded single line (the full value still
    // reachable via tooltip and selection), not wrap into many lines.
    run_gtk(|| {
        let pv = ProcessView::new();
        let d = pv.detail_labels();
        let long = "q".repeat(400);
        for l in [&d.name, &d.command] {
            l.set_label(&format!("x: {long}"));
            assert_eq!(
                l.ellipsize(),
                gtk4::pango::EllipsizeMode::End,
                "a detail-pane value label must ellipsize"
            );
            assert!(
                l.max_width_chars() > 0,
                "a detail-pane value label must cap its width"
            );
            let (min, nat, _, _) = l.measure(gtk4::Orientation::Horizontal, 100);
            assert!(
                min < 1000 && nat < 1000,
                "a 400-char token gave a min={min}px / nat={nat}px size request — it must stay bounded"
            );
        }
    });
}
