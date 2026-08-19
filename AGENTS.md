# Simple Task Manager - Agent Guidelines

## Build, Test, and Lint Commands

### Building
```bash
cargo build         # Debug build
cargo build --release  # Release build with optimizations
cargo check         # Fast check without building
```

### Testing
```bash
cargo test          # Run all tests
cargo test <test_name>  # Run a specific test function
cargo test -- --nocapture  # Run tests with output
cargo test -- --test-threads=1  # Run tests sequentially (useful for UI/state testing)
```

### Linting
```bash
cargo clippy --all-targets   # Lint lib AND tests/binaries (required: plain `cargo clippy` skips test files)
cargo clippy                 # Lint the lib/bin only
cargo fmt                    # Format code using rustfmt
cargo fmt --check            # Check if code is formatted
```

### Running
```bash
cargo run
```

## Code Style Guidelines

### General Structure
- Use `pub` for public items by default
- Group related items using `mod` declarations
- Place crate-level documentation at the top of each module
- Keep each module focused on a single responsibility

### Imports
```rust
// Organize imports: std first, then external crates, then local dependencies
use std::cell::RefCell;
use std::time::Duration;

use floem::prelude::*;
use im::Vector;
use crate::Process;
```
- Group imports: std > external > local
- Use `*` only for broad prelude imports

### Code Formatting
- Use cargo fmt (auto-formats on commit)
- Default indentation: 4 spaces
- Consistent spacing: 1 space around operators, 2 spaces between fields

### Type Conventions
```rust
// Prefer early returns for cleaner flow
pub fn get_process(pid: i32) -> Option<Process> {
    process::all_processes()
        .expect("Can't read /proc")
        .filter_map(|p| match p {
            Ok(p) if p.pid() == pid => Some(p),
            _ => None,
        })
        // ...
}

// Use explicit types for clarity
pub struct Process {
    pub name: String,
    pub pid: i32,
    pub ruid: u32,
    pub username: String,
    pub cpu_percent: f64,
}
```
- Use `Option` for nullable values, `Result` for operations that can fail
- Prefer early returns over nested if-let chains
- Use field `ruid` instead of `uid` for process owner identification

### Naming Conventions
```rust
// Clear, descriptive names
let process_list_signal = create_rw_signal(Vector::new());
let selected_process = create_rw_signal(None);
let cpu_tracker = RefCell::new(CpuTracker::new());

// Use snake_case for functions and variables
pub fn process_names(filter: UserFilter) -> im::Vector<Process> { ... }

// Use PascalCase for structs and enums
pub struct Process { ... }
pub enum UserFilter { Current, All }

// Use SCREAMING_SNAKE_CASE for constants
```

### Error Handling
Use anyhow to handle and propagate errors.

- Use `?` operator for propagating expected errors
- Log errors with `e:?` formatting for debugging

### Logging and Error Handling

Use the `log` crate for all diagnostic output. Avoid using `println!`, `eprintln!`, and `dbg!` in production code.

#### Logging Levels
```rust
// Use log::error! for actual errors
log::error!("Failed to update process list: {}", e);

// Use log::warn! for warnings (non-fatal issues)
log::warn!("Can't read process due to error: {}", e);

// Use log::info! for informational messages and diagnostics
log::info!("Process detail dialog closed");

// Use log::debug! for debug-level information
log::debug!("CPU percent calculation: {:.2}%", cpu_percent);
```

#### Guidelines
- **Consistent logging**: Always use the `log` crate instead of `println!` or `eprintln!`
- **Single reporting**: Report each error or event only once (don't log to both console and log)
- **Appropriate levels**: Use the correct log level for each message type
  - `error`: For actual errors that affect functionality
  - `warn`: For unexpected situations that don't break the application
  - `info`: For informational messages and user-facing diagnostics
  - `debug`: For debugging information that's useful during development
- **Error details**: Include relevant error details using `{:?}` format for debugging
- **Context**: Provide enough context to understand the error without being verbose

### Testing
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_struct_creation() {
        let p = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        assert_eq!(p.name, "test");
        // ...
    }

    #[test]
    fn test_process_struct_clone() {
        let p1 = Process::new("test".to_string(), 123, 456, "user".to_string(), 0.0);
        let p2 = p1.clone();
        assert_eq!(p1, p2);
    }
}
```
- Place tests in `#[cfg(test)] mod tests` block
- Use `super::*` to access parent module items
- Prefix test functions with `test_`
- Run tests with `cargo test <function_name>`

### UI toolkit

This app uses floem as UI toolkit. Please read its documentation at `generated_docs/floem/index.md`.
Look at floem code examples at `git/floem/examples/`.

Focus on the reactive programming concept documented in Floem documentation and examples.

### Documentation
```rust
/// A structure representing a running process with CPU usage statistics.
pub struct Process {
    // ...
}

/// Creates a new Process instance with the specified attributes.
pub fn new(name: String, pid: i32, ruid: u32, username: String, cpu_percent: f64) -> Self {
    // ...
}
```
- Document all public structs, enums, and functions
- Use rustdoc comments for documentation
- Keep doc comments concise and informative
- Include `pub` in doc comments for public API items

## Development Workflow

Work in a tight loop, completing one small, self-contained change at a time.
Treat steps 2-4 as a single **validation gate**: you may commit *only* when every
one of them passes together, run *after your last edit*.

1. **Implement** - one focused change at a time; keep it small
2. **Add unit tests** - add or update tests in `#[cfg(test)] mod tests`, then run `cargo test`; every test must pass (this also compiles the code)
3. **Lint** - run `cargo clippy --all-targets`; it must finish with **zero warnings** (`--all-targets` is required so test files are linted too)
4. **Format** - run `cargo fmt` (verify clean with `cargo fmt --check`)
5. **Commit** - only once steps 2-4 are all green, with a message stating what changed, why, and how it was verified. Do NOT include a `Co-Authored-By` trailer (no Claude/AI attribution line) in commit messages.

Gate rules:
- Re-run the whole gate after every edit; a green gate from an earlier state does not count
- Never commit with failing tests or any clippy warning
- Do not push unless explicitly asked
- Commit messages must stay clean: no `Co-Authored-By` or similar AI attribution trailers

### Important Development Notes
- **Build regularly** to fix build errors; use `rustc --explain <error number>` for help
- **Always run unit tests** before committing
- **Run `cargo clippy --all-targets`** before committing and address all findings
- **Focus on small changes** and commit when you get something to build and pass tests
- **Read floem crate documentation** for examples on implementing UI changes
- **Read Rust crate documentation** this documentation is located in generated_docs/<crate>. To refresh run `cargo docs-md docs`
- **Use tests** to validate your changes; do not run the app
- **Always update README.md** when you add or modify features
