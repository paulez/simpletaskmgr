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