# Floem Reactive Programming Guide

This guide explains how to implement reactive UI patterns in Floem, specifically demonstrating the patterns used in the simpletaskmgr application.

## Core Concepts

### Signals

Floem uses a signal-based reactive system:
- `RwSignal<T>` - A read-write signal that holds a value of type T
- `create_rw_signal(initial_value)` - Creates a new signal
- `.get()` - Reads the current value (reactive)
- `.set(new_value)` - Updates the value (triggers reactivity)
- `.track()` - Tracks signal changes for effects

### Reactivity

When you access a signal value inside a view function, Floem automatically tracks dependencies and re-renders when the value changes.

## Implementation: Process Manager App

The application demonstrates several reactive patterns:

### 1. State Management with Signals

```rust
let selected_process_id = create_rw_signal(None);  // Store just the ID
let tick = create_rw_signal(());
```

- `selected_process_id` tracks the ID of the currently selected process
- `tick` is used to trigger periodic updates of the process list

**Note**: We store only the process ID, not the full process object, to ensure we always get the latest data from the process list.

### 2. Dynamic View with Signal-Driven Content

```rust
let main_view = dyn_container(
    move || selected_process_id.get(),
    move |selected_process_id_item| {
        // View logic based on current state
    }
);
```

The `dyn_container` creates a dynamic view that re-renders whenever `selected_process_id` changes.

### 3. Periodic Updates with Effects

```rust
create_effect(move |_| {
    tick_for_effect.track();
    let process_list_for_effect = Rc::clone(&process_list);
    exec_after(Config::refresh_interval(), move |_| {
        process_list_for_effect.update_process_list();
        tick_for_effect.set(());  // Update tick to trigger re-render
    });
});
```

This effect:
- Tracks changes to the `tick` signal
- Periodically updates the process list using `exec_after`
- Triggers a re-render by updating the `tick` signal

### 4. Event Handling

```rust
process_list_view(process_list_for_view.processes, move |process: Process| {
    selected_process_id.set(Some(process.pid));  // Store just the PID
})
```

Click events on process items update the `selected_process_id` signal with the process PID, which triggers the UI to show the detail view.

### 5. Looking Up Live Data (Key Fix!)

```rust
let process_scroll = process_list_view(process_list_for_view.processes, move |process: Process| {
    selected_process_id.set(Some(process.pid));
})
.style(|s| s.max_width_full().width_full())
.scroll()
.style(|s| s.padding(10).padding_right(14))
.scroll_style(|s| s.shrink_to_fit().handle_thickness(8));

let main_container = match selected_process_id_item {
    Some(pid) => {
        // Look up the process from the current process list
        let current_process = process_list_for_view.processes.get()
            .iter()
            .find(|p| p.pid == pid)
            .cloned();

        if let Some(process) = current_process {
            container(
                h_stack((
                    process_scroll,
                    scroll(process_detail_view(process))
                        .style(|s| s.width(50_i32.pct()).height_full()),
                ))
                .style(|s| s.width_full().height_full()),
            )
        } else {
            container(process_scroll)  // Fallback if process not found
        }
    },
    None => container(process_scroll),
};
```

This is the key improvement: instead of storing the entire process object, we store just the PID and look up the current process data each time the view renders. This ensures we always show the latest metrics.

## Complete Updated Example

```rust
fn app_view() -> impl IntoView {
    let selected_process_id = create_rw_signal(None);  // Store PID only
    let tick = create_rw_signal(());
    let process_list = Rc::new(ProcessList::init());
    let process_list_for_view = Rc::clone(&process_list);
    let tick_for_effect = tick;

    create_effect(move |_| {
        tick_for_effect.track();
        let process_list_for_effect = Rc::clone(&process_list);
        exec_after(Config::refresh_interval(), move |_| {
            process_list_for_effect.update_process_list();
            tick_for_effect.set(());
        });
    });

    let main_view = dyn_container(
        move || selected_process_id.get(),
        move |selected_process_id_item| {
            let process_scroll = process_list_view(process_list_for_view.processes, move |process: Process| {
                selected_process_id.set(Some(process.pid));  // Set PID only
            })
            .style(|s| s.max_width_full().width_full())
            .scroll()
            .style(|s| s.padding(10).padding_right(14))
            .scroll_style(|s| s.shrink_to_fit().handle_thickness(8));

            let main_container = match selected_process_id_item {
                Some(pid) => {
                    // Look up current process data
                    let current_process = process_list_for_view.processes.get()
                        .iter()
                        .find(|p| p.pid == pid)
                        .cloned();

                    if let Some(process) = current_process {
                        container(
                            h_stack((
                                process_scroll,
                                scroll(process_detail_view(process))
                                    .style(|s| s.width(50_i32.pct()).height_full()),
                            ))
                            .style(|s| s.width_full().height_full()),
                        )
                    } else {
                        container(process_scroll)  // Process not found
                    }
                },
                None => container(process_scroll),
            };
            main_container.style(|s| s.width_full().height_full().border(1.0))
        },
    );

    main_view.style(|s| {
        s.size(100_i32.pct(), 100_i32.pct())
            .padding_vert(20.0)
            .flex_col()
            .items_center()
    })
}
```

## Key Points

1. **Store identifiers, not objects**: When selecting an item, store just the ID/PID rather than the entire object
2. **Look up live data**: Use the identifier to look up current data from the source each time the view renders
3. **Use `dyn_container` for conditional rendering**: It provides a clean way to switch between different views based on signal state
4. **Combine signals with effects**: Effects can be used to create side effects that run when signals change
5. **Use `exec_after` for periodic tasks**: This is the recommended way to implement timers in Floem
6. **Share state with `Rc`**: Wrap signals in `Rc` to share them between different parts of your UI
7. **Handle missing data gracefully**: Check if the looked-up data exists before displaying it

## Best Practices

- Keep signals in the root of your app view to share state across components
- Use `Rc` to share signals between different parts of your UI
- Prefer immutable operations - create new data structures rather than mutating existing ones
- Use `move` closures to capture signal values properly
- Consider using a separate signal for each piece of state that needs independent reactivity
- Use effects sparingly - only when you need side effects that shouldn't trigger UI re-renders directly
- **Store references/IDs, not objects**: This ensures your UI always shows current data
- **Handle missing or stale data**: Provide fallback views when data isn't available

## Styling with Signals

The implementation shows how to apply styles based on the current state:

```rust
.style(|s| s.width(50_i32.pct()).height_full())
```

These style closures create reactive styles that adapt to the current layout and state.

## Problem Solved: Updating Detail View on Refresh

The key insight is that **cloning an object at selection time means you have a snapshot**, not a live reference. The solution is to:

1. Store only the identifier (PID) when a process is selected
2. Look up the current process data each time the detail view is rendered
3. The lookup happens automatically because the view re-renders on each refresh (via the tick signal)

This pattern ensures the detail view always shows the latest data from the refreshed process list.

---

## Source-Level Findings (verified against floem 0.2.0 source tree)

This section documents non-obvious behaviors of floem 0.2.0 internals that matter
for designing the reactive UI model in this codebase. All line references are
to `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/floem_reactive-0.2.0/`
and `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/floem-0.2.0/`.

### Signal identity vs. value equality

`RwSignal<T>` is `Copy + Clone` and implements `Eq + PartialEq`, but **does not
implement `Hash`**.

The `Eq`/`PartialEq` implementations compare by the signal's internal `id`
(a monotonically-incrementing `u64`), **not by the value stored**. Two
`RwSignal<T>` values are "equal" if and only if they are the same underlying
signal, regardless of what value each currently holds.

Practical consequences:
- You cannot use an `RwSignal<T>` as a `HashMap` key or `HashSet` member
  (no `Hash` trait bound).
- `sig_a == sig_b` is `true` only when both refer to the identical runtime
  signal; it gives no information about the stored values.
- Two signals can hold identical `TaskMgrProcess` snapshots yet not be
  `==` — this is expected and not a bug.

### Signal read / subscribe semantics

| Method | Subscribes current effect? | Notes |
|--------|:---:|-------|
| `.get()` | yes | calls `subscribe()` internally, then `get_untracked()` |
| `.get_untracked()` | no | plain read, safe outside an effect |
| `.track()` | yes | only subscribes; does not return a value |
| `.with(f)` | yes | reads + subscribes, `f` borrows the value |
| `.with_untracked(f)` | no | reads, `f` borrows the value |
| `.set(val)` / `.update(f)` | n/a (write) | writes value, then calls `run_effects()` unconditionally |

`Signal::subscribe()` is a **no-op when no effect is currently running**
(signal.rs:234-243): it checks `runtime.current_effect` and skips if `None`.
This is what allows data-layer unit tests to create and mutate signals without
a UI loop — `subscribe()` is harmless outside an effect context.

### Write-path: no value deduplication

`Signal::set` → `update_value(f)` → **`run_effects()` unconditionally**
(signal.rs:204-232). There is no check for whether the value actually changed.

- **Non-batching mode** (the default): `run_effects()` calls `run_effect()`
  immediately on every subscriber, even if the value is identical to before.
- **Batching mode** (`batch` closure): `add_pending_effect(effect)` deduplicates
  by `effect.id()` (runtime.rs:51-60); `run_pending_effects()` then runs each
  unique pending effect once (runtime.rs:62-67).

Implication: rapid successive `set(same_value)` calls will redundantly re-render
subscribed views outside of a `batch` block.

### Effect re-run: `run_effect` lifecycle

`run_effect(effect)` (effect.rs:161-178) performs, in order:
1. `effect_id.dispose()` — removes the effect's `Id` from the runtime's
   children and signals maps; recursively disposes any children and any
   signal registered under that id (id.rs:38-57).
2. `observer_clean_up(effect)` — for every signal the effect is currently
   subscribed to, removes the effect from that signal's subscriber list,
   then clears the effect's own observer list.
3. Creates a new scope for this run and calls `effect.run()` inside it.
4. Resets `current_effect` to `None`.

This means **each run is self-contained**: previous subscriptions are fully
cleaned up before the new run executes. Stale subscriptions cannot accumulate
across runs.

### `Scope` and resource cleanup

- Every signal, effect, and updater is registered in a `Scope` (a `u64` id).
- `Id::dispose()` removes the id from `runtime.signals` (destroying the signal
  data) and from `runtime.children`; it recurses into any child ids and for
  any recovered signal, calls `observer_clean_up` on each of its subscribers
  (id.rs:38-57).
- `as_child_of_current_scope(f)` wraps `f` so that when called, it:
  1. Creates a child scope of the current scope.
  2. Runs `f(item)` with the current scope set to the child.
  3. Returns `(result, child_scope)`.

  Disposing the child scope cascades cleanup to all signals/effects created
  within it (scope.rs:151-175).

This is the mechanism by which `dyn_stack` destroys a row's reactive
subscribers when the row is removed: each row's view is created inside a
child scope, and `scope.dispose()` (called by `remove_index` in
dyn_stack.rs:281-285) disposes all signals and updaters within that scope.

### `label` reactivity mechanism

```rust
// label.rs:129-138
pub fn label<S: Display + 'static>(label: impl Fn() -> S + 'static) -> Label {
    let id = ViewId::new();
    let initial_label = create_updater(
        move || label().to_string(),           // ← compute: reads signals
        move |new_label| id.update_state(new_label),  // ← on_change: updates text
    );
    Label::new(id, initial_label) /* … */
}
```

- The `compute` closure (`|| label().to_string()`) calls the user's closure,
  which reads signals via `.get()`. Each `.get()` subscribes the updater
  effect to that signal (because `subscribe()` is a no-op inside an effect
  but fires when one is running).
- When a subscribed signal fires, `run_effect` re-runs the updater: `compute`
  runs again (re-subscribes), then `on_change` pushes the new text string
  via `id.update_state(new_label)`.
- **The `Label` view itself is never destroyed or recreated on a text change** —
  only its text content is updated. The DOM node persists. This is the
  "in-place update" mechanism that makes `ProcessItem` row labels cheap to
  refresh.

### `dyn_stack` — keyed diff-based view container

Signature (dyn_stack.rs:81-88):
```rust
pub fn dyn_stack<IF, I, T, KF, K, VF, V>(
    each_fn: IF,    // Fn() -> I, I: IntoIterator<Item = T>
    key_fn:  KF,    // Fn(&T) -> K, K: Eq + Hash + 'static   ← key type
    view_fn: VF,    // Fn(T) -> V, V: IntoView
) -> DynStack<T>
```

Internally uses `create_effect` (dyn_stack.rs:92). Each effective run:
1. `each_fn()` returns the current item list.
2. `key_fn(&item)` maps each item to a hashable `K`.
3. Keys are placed in an `FxIndexSet` (ordered hash set, indexmap-based).
4. The previous keys set and the new keys set are diffed (dyn_stack.rs:194-274)
   into four categories:
   - **`added`** — keys in new but not old. Their item is pulled from the
     current list and `view_fn` is called to create a new child view.
   - **`removed`** — keys in old but not new. Their existing child view's
     scope is disposed (destroyed).
   - **`moved`** — keys in both, but their relative position changed after
     accounting for inserts/removals. Their existing child view is **moved**
     (not re-created).
   - **`unchanged`** — keys in both, same relative position, and not in any
     of the above three. Their child view is **left completely untouched**.

**Key design implication:**

| Keying strategy | On data change (e.g. CPU%) | Cost |
|-----------------|---------------------------|------|
| `key_fn = \|item\| item.pid` (stable `i32`) | Key set unchanged → "unchanged" category → **view persists** | Labels update in place via their own signal subscriptions; no DOM teardown |
| `key_fn = \|item\| item.clone()` (whole struct) | Key changes → old key "removed" + new key "added" → **view destroyed + recreated** | Full DOM teardown per changed row |

This is the core reason Approach B (key on `pid`, value on a signal) is
strictly better for list performance than the old approach.

**Key type constraint:**
`K: Eq + Hash + 'static`. Since `RwSignal<T>` does not implement `Hash`,
and `f64` is not `Eq`/`Hash`, the key must be extracted from stable fields
of the item (e.g., an `i32` pid).

### `dyn_container` — state-gated view swap

Signature (dyn_container.rs:92-114):
```rust
pub fn dyn_container<CF, T, IV>(
    update_view: impl Fn() -> T + 'static,  // the "state" closure
    child_fn: CF,                           // Fn(T) -> IV
) -> DynamicContainer<T>
```

- Internally uses `create_updater(update_view, on_change)`.
- `update_view()` subscribes `dyn_container`'s updater effect to whatever
  signals it reads. It fires (re-runs the updater) **only when those signals
  fire**, not when other signals change.
- On fire: `on_change` → `id.update_state(DynMessage::Val(val))` →
  `DynamicContainer::update` → `new_val` → `swap_val`:
  1. `child_fn(val)` creates the new child view.
  2. The old child id and scope are disposed.
  3. The new child replaces the old one.

**Every update destroys the old child and creates a new one.** There is no
diffing or skipping of unchanged sub-components. This is intentional:
`dyn_container` is for **mutually exclusive states** (e.g., "selected /
unselected") where the entire subtree should be swapped.

**Use in this codebase:** `process_detail_view` currently wraps the detail
content in `dyn_view`, which subscribes to *both* `selected_pid` and
`processes`. This means the detail panel is rebuilt on every process refresh,
not just on selection change. A `dyn_container` subscribed only to
`selected_pid` would be more efficient — the `processes` signal's items were
already reused (same ids), so the detail labels' own `value` signal
subscriptions would keep them in sync without requiring a full rebuild.

### `dyn_view` — unconditional re-evaluation + child swap

Signature (dyn_view.rs:14-34):
```rust
pub fn dyn_view<VF, IV>(view_fn: VF) -> DynamicView
where VF: Fn() -> IV + 'static, IV: IntoView
```

- Uses `create_updater(move || view_fn(()), on_change)`.
- `view_fn(())` returns the new child view.
- `on_change` **unconditionally** disposes the old child scope and installs
  the new child. There is no equivalence check.

`dyn_view` is the **most aggressive** of the three container families: it
rebuilds its child on every fired update, even if the result is identical.
It is appropriate for content that is genuinely computed from scratch each
time (e.g., a conditional layout with no stable identity).

### Comparison of the reactive view families

| Family | Trigger | Child on update | Cost when data-only changes |
|--------|---------|-----------------|:---:|
| `label` | subscribed signals | Text updates in place; view node persists | Lowest |
| `dyn_stack` (key = stable id) | key set change | Added → create; removed → destroy; unchanged → **keep** | Low (unchanged rows untouched) |
| `dyn_container` | signals in `update_view` closure | Old child disposed, new child created | Medium (full rebuild of child subtree) |
| `dyn_view` | signals in `view_fn` closure | Old child disposed, new child created | Highest (unconditional rebuild) |

### The `ProcessItem` model (implemented in this codebase)

```rust
pub struct ProcessItem {
    pub pid: i32,                        // ← stable key for dyn_stack
    pub value: RwSignal<TaskMgrProcess>, // ← data, swapped in place on refresh
}
```

Why this works:
- `dyn_stack` is keyed on `item.pid` (an `i32`, which is `Eq + Hash`).
- On refresh, `update_process_list` reuses existing items for pids that are
  still alive (calling `item.copy_from(&new_task_mgr_process)`, which
  `value.set(new_snapshot)`), and creates fresh items only for new pids.
- Since the `pid` keys and their relative order are unchanged, `dyn_stack`'s
  diff classifies every existing row as "unchanged" → the `Label` views
  survive.
- Each label in a row reads `item.value.get()` (reactive). When `value.set()`
  is called from `copy_from`, the label's updater fires and pushes the new
  string to the still-alive `Label` view → **in-place text update, no DOM
  teardown**.
- Rows that appear this refresh (new `pid`) are in the "added" category →
  `process_item_view` is called to build their view.
- Rows that disappeared (dead `pid`) are in the "removed" category → their
  child scope is disposed, cleaning up their label subscriptions.

### Pitfalls and open issues

1. **`dyn_view` in `process_detail_view` rebuilds on every refresh.**
   The detail panel holds a `dyn_view` whose compute closure reads both
   `selected_pid` and `processes`. Since `processes.set(...)` fires on every
   tick, the detail panel is destroyed and recreated each cycle. Switching to
   `dyn_container` (subscribed only to `selected_pid`) would let the detail
   labels update in place via their own `value` signal subscriptions.
   (Not yet done; functionally correct, just less efficient.)

2. **No value dedup in non-batching mode.**
   If a refresh produces identical data (`copy_from` with the same snapshot),
   `value.set()` still fires the label updaters, pushing the same string to
   the same `Label` view. This costs a trivial re-render but is not visible
   to the user. In `batch()` mode, the dedup mechanism (pending-effect
   deduplication by effect id) would prevent this, but the codebase currently
   does not wrap its set operations in `batch`.

3. **`RwSignal::Eq` compares by id, not value.**
   Writing tests or assertions that use `==` to compare signal containers
   (e.g., `assert_eq!(item.signal_a, item.signal_b)`) is asserting *signal
   identity*, not value equality. Always compare the resolved values via
   `.get()` / `.get_untracked()` instead.

4. **`TaskMgrProcess` contains `f64` → cannot be `Hash`.**
   `f64` has no `Hash` impl (and `Eq`/`PartialEq` are non-reflexive for NaN).
   This is fine: `dyn_stack`'s key type in this codebase is `i32` (`pid`),
   never the whole struct. Do not attempt to key on `TaskMgrProcess` directly.