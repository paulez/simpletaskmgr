# Floem Reactive Programming Guide

This guide explains how to implement reactive UI patterns in Floem, specifically for the process detailed view feature.

## Core Concepts

### Signals

Floem uses a signal-based reactive system:
- `RwSignal<T>` - A read-write signal that holds a value of type T
- `create_rw_signal(initial_value)` - Creates a new signal
- `.get()` - Reads the current value (reactive)
- `.set(new_value)` - Updates the value (triggers reactivity)

### Reactivity

When you access a signal value inside a view function, Floem automatically tracks dependencies and re-renders when the value changes.

## Problem: Process Detail View

The goal is to show a detailed view of a selected process when a user clicks on it in the process list.

### Option 1: Signal-Based Selection (Recommended)

```rust
fn app_view() -> impl IntoView {
    // Create a signal to hold the selected process
    let selected_process = create_rw_signal(None);

    // Create the process list view
    let processes = RwSignal::new(vector![/* process data */]);
    let process_list = process_list_view(processes.clone())
        .on_click(move |process: Process| {
            // Update the signal when a process is clicked
            selected_process.set(Some(process.clone()));
        });

    // Show different views based on the selected state
    show_match!(
        selected_process,
        |Some(process)| {
            h_stack((
                process_list,
                scroll(process_detail_view(process))
            ))
        },
        |None| {
            process_list
        }
    )
}
```

### Option 2: Dynamic View with Signal

```rust
fn app_view() -> impl IntoView {
    let selected_process = create_rw_signal(None);
    let processes = RwSignal::new(vector![/* process data */]);

    // Use a dynamic container to switch views
    dyn_container(move || {
        match selected_process.get() {
            Some(process) => h_stack((
                process_list_view(processes.clone())
                    .on_click(move |p: Process| selected_process.set(Some(p))),
                scroll(process_detail_view(process))
            )),
            None => process_list_view(processes.clone())
                .on_click(move |p: Process| selected_process.set(Some(p))),
        }
    })
}
```

### Option 3: Using create_effect for Side Effects

```rust
fn app_view() -> impl IntoView {
    let selected_process = create_rw_signal(None);
    let processes = RwSignal::new(vector![/* process data */]);

    // Create an effect that runs when selection changes
    create_effect(move |_| {
        selected_process.track(); // Track changes to selected_process
        log::info!("Selected process: {:?}", selected_process.get());
    });

    process_list_view(processes.clone())
        .on_click(move |process: Process| {
            selected_process.set(Some(process.clone()));
        })
}
```

## Key Points

1. **Always use `.set()` to update signals** - Direct assignment won't trigger reactivity
2. **Access signals inside view functions** - This creates the reactive dependency
3. **Use `show_match!` or `dyn_container`** - These are the idiomatic ways to switch views based on state
4. **Track signals in effects** - Use `.track()` to create side effects that run when signals change

## Best Practices

- Keep signals in the root of your app view to share state across components
- Use `Rc` to share signals between different parts of your UI
- Prefer immutable operations - create new data structures rather than mutating existing ones
- Use `move` closures to capture signal values properly

## Example: Process Detail View

```rust
fn process_detail_view(process: Process) -> impl IntoView {
    v_stack((
        label(move || format!("PID: {}", process.pid)),
        label(move || format!("Name: {}", process.name)),
        label(move || format!("CPU: {:.1}%", process.cpu_percent)),
    ))
    .style(|s| s.gap(8).padding(20))
}
```

For more examples, see the Floem documentation on signals and the todo-complex example in the Floem repository.