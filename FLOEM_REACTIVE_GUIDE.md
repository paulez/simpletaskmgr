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
let selected_process = create_rw_signal(None);
let tick = create_rw_signal(());
```

- `selected_process` tracks which process is currently selected
- `tick` is used to trigger periodic updates of the process list

### 2. Dynamic View with Signal-Driven Content

```rust
let main_view = dyn_container(
    move || selected_process.get(),
    move |selected_process_item| {
        // View logic based on current state
    }
);
```

The `dyn_container` creates a dynamic view that re-renders whenever `selected_process` changes.

### 3. Periodic Updates with Effects

```rust
create_effect(move |_| {
    tick_for_effect.track();
    let process_list_for_effect = Rc::clone(&process_list);
    exec_after(Config::refresh_interval(), move |_| {
        process_list_for_effect.update_process_list();
        tick_for_effect.set(());
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
    selected_process.set(Some(process.clone()));
})
```

Click events on process items update the `selected_process` signal, which triggers the UI to show the detail view.

## Complete Example

```rust
fn app_view() -> impl IntoView {
    let selected_process = create_rw_signal(None);
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
        move || selected_process.get(),
        move |selected_process_item| {
            let process_scroll = process_list_view(process_list_for_view.processes, move |process: Process| {
                selected_process.set(Some(process.clone()));
            })
            .style(|s| s.max_width_full().width_full())
            .scroll()
            .style(|s| s.padding(10).padding_right(14))
            .scroll_style(|s| s.shrink_to_fit().handle_thickness(8));

            let main_container = match selected_process_item {
                Some(process) => container(
                    h_stack((
                        process_scroll,
                        scroll(process_detail_view(process))
                            .style(|s| s.width(50_i32.pct()).height_full()),
                    ))
                    .style(|s| s.width_full().height_full()),
                ),
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

1. **Use `dyn_container` for conditional rendering** - It provides a clean way to switch between different views based on signal state
2. **Combine signals with effects** - Effects can be used to create side effects that run when signals change
3. **Use `exec_after` for periodic tasks** - This is the recommended way to implement timers in Floem
4. **Share state with `Rc`** - Wrap signals in `Rc` to share them between different parts of your UI
5. **Update signals to trigger re-renders** - The `tick` pattern is useful for forcing UI updates

## Best Practices

- Keep signals in the root of your app view to share state across components
- Use `Rc` to share signals between different parts of your UI
- Prefer immutable operations - create new data structures rather than mutating existing ones
- Use `move` closures to capture signal values properly
- Consider using a separate signal for each piece of state that needs independent reactivity
- Use effects sparingly - only when you need side effects that shouldn't trigger UI re-renders directly

## Styling with Signals

The implementation shows how to apply styles based on the current state:

```rust
.style(|s| s.width(50_i32.pct()).height_full())
```

These style closures create reactive styles that adapt to the current layout and state.