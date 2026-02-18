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
cargo clippy        # Run clippy linter with suggestions
cargo fmt            # Format code using rustfmt
cargo fmt --check   # Check if code is formatted
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
- Use rustfmt (auto-formats on commit)
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
```rust
// Use expect() for critical initialization failures
let cache = UsersCache::new()
    .expect("Failed to initialize user cache");

// Print errors gracefully for non-critical issues
Err(e) => {
    println!("Can't read process due to error {e:?}");
    None
}
```
- Use `expect()` for initialization that must succeed in normal operation
- Use `println!` for non-critical errors
- Use `?` operator for propagating expected errors
- Log errors with `e:?` formatting for debugging

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

### Reactive Programming
```rust
// Use signals for reactive state
let process_list_signal = create_rw_signal(Vector::new());


// Update signals directly when state changes
selected_process.set(Some(p));
```
- Use `create_rw_signal` for mutable shared state
- Clone signals when needed for closure captures
- Set signals sparingly to avoid excessive re-renders

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

1. **Before committing**: Run `cargo clippy`, `cargo fmt --check`, and `cargo test`
2. **Check formatting**: `cargo fmt --check`
3. **Fix clippy issues**: `cargo clippy` and address warnings
4. **Run tests**: `cargo test` (ensure all tests pass)
5. **Build**: `cargo build --release` to create the final binary
6. **Run**: `cargo run --release` to launch the application
7. **Commit**: Only after tests pass and code is formatted

### Important Development Notes
- **Build regularly** to fix build errors; use `rustc --explain <error number>` for help
- **Always run unit tests** before committing
- **Run cargo clippy** before committing and address all findings
- **Focus on small changes** and commit when you get something to build and pass tests
- **Read floem crate documentation** for examples on implementing UI changes
- **Read Rust crate documentation** this documentation is located in generated_docs/<crate>. To refresh run `cargo docs-md docs`
- **Use tests** to validate your changes; do not run the app
- **Always update README.md** when you add or modify features

## RTK Token Optimization

### Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

**Important**: Even in command chains with `&&`, use `rtk`:
```bash
# ❌ Wrong
git add . && git commit -m "msg" && git push

# ✅ Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
``` 

### RTK Commands by Workflow

#### Build & Compile (80-90% savings)
```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file (80%)
rtk tsc                 # TypeScript errors grouped by file/code (83%)
rtk lint                # ESLint/Biome violations grouped (84%)
rtk prettier --check    # Files needing format only (70%)
rtk next build          # Next.js build with route metrics (87%)
``` 

#### Test (90-99% savings)
```bash
rtk cargo test          # Cargo test failures only (90%)
rtk vitest run          # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
rtk test <cmd>          # Generic test wrapper - failures only
``` 

#### Git (59-80% savings)
```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff (80%)
rtk git show            # Compact show (80%)
rtk git add             # Ultra-compact confirmations (59%)
rtk git commit          # Ultra-compact confirmations (59%)
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
``` 

Note: Git passthrough works for ALL subcommands, even those not explicitly listed.

#### Files & Search (60-75% savings)
```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%)
NOT COMPATIBLE with GNU find: rtk find <pattern>      # Find grouped by directory (70%)
``` 

#### Analysis & Debug (70-90% savings)
```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
``` 

#### Network (65-70% savings)
```bash
rtk curl <url>          # Compact HTTP responses (70%)
rtk wget <url>          # Compact download output (65%)
``` 

#### Meta Commands
```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk discover            # Analyze Claude Code sessions for missed RTK usage
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to CLAUDE.md
rtk init --global       # Add RTK to ~/.claude/CLAUDE.md
```

**Note**: For workflow-specific RTK usage, see CLAUDE.md for additional context and examples.
