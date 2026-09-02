# Simple Task Manager - Agent Guidelines

## Build, Test, and Lint Commands

```bash
# Lint
cargo clippy --all-targets   # Required: lints lib, tests, and binaries
# Format
cargo fmt                    # Verify with `cargo fmt --check`
# Run / test
cargo test                   # Run specific test with `cargo test <name>`
# Validate (run this before every commit)
cargo check && cargo test && cargo clippy --all-targets && cargo fmt --check
```

## Code Style

1. **Imports**: Group std > external > local
2. **Types**: Use `Option` for nullable values, `Result` for fallible operations. Use `ruid` instead of `uid` for process owner ID.
3. **Naming**:
   - snake_case for functions/variables
   - PascalCase for structs/enums
   - SCREAMING_SNAKE_CASE for constants
4. **Error Handling**: Use `anyhow` with `?` operator for expected errors. Log with `e:?` for debugging.
5. **Logging**: Always use the `log` crate, avoid `println!`, `eprintln!`, `dbg!`:
   - `error!`: Affects functionality
   - `warn!`: Non-fatal issues
   - `info!`: User-facing diagnostics
   - `debug!`: Development information
6. **Testing**:
   - Place in `#[cfg(test)] mod tests`
   - Prefix with `test_`
   - Use `super::*` for parent items
7. **Documentation**: Document all public items with rustdoc comments.

## UI Toolkit

This app uses **GTK4** from Rust via the gtk-rs crate.
- Look at examples in src/ui.rs and src/process_row.rs for common patterns
- Keep GTK object references in struct fields (e.g., `Gtk::Builder`, widgets)
- Bind data changes to signals (use callback handlers for list updates, user input)
- For performance with large lists, consider virtual scrolling/pagination

## Development Workflow

Work in small, focused changes. Follow this gate before committing:

1. **Implement** one small change
2. **Add tests** in `#[cfg(test)] mod tests`, run `cargo test`
3. **Lint** with `cargo clippy --all-targets` (must pass with 0 warnings)
4. **Format** with `cargo fmt`
5. **Commit** with a clear message (no `Co-Authored-By` or AI attribution)

**Commit rules:**
- Always commit when completing an approved plan, once the validation gate above passes
- Never commit failing tests or clippy warnings
- Do not push unless explicitly asked
- Use `rustc --explain <error number>` for build errors
- Read GTK4 documentation when adding UI components (see https://gtk-rs.org/docs-rs/latest/ or gtk-rs source for patterns)
- Keep UI code reactive: bind data to signal handlers, update UI when state changes

## Graphical Verification (screenshots)

Logic is verified with tests (never by running the app). But **UI/layout**
changes — graph sizing, axis placement, label legibility, spacing — can only
be confirmed by rendering. Do this with a headless display + a screenshot, and
capture a **before** and **after** so the change is visible:

```bash
# 1. Start a fixed-display virtual X server with access control off.
#    A fixed display (e.g. :96) is required — xvfb-run -a's auto-assigned
#    display + `import` fails with "Authorization required".
Xvfb :96 -ac -screen 0 1024x768x24 >/tmp/opencode/xvfb.log 2>&1 &

# 2. Launch the app against that display.
DISPLAY=:96 GDK_BACKEND=x11 target/debug/simpletaskmgr >/tmp/opencode/app.log 2>&1 &
APP_PID=$!

# 3. Wait a few refresh cycles so the rolling graph has data, then screenshot
#    the whole root window (ImageMagick `import`).
sleep 8
DISPLAY=:96 import -window root /tmp/opencode/AFTER_graph.png

# 4. Tear down.
kill "$APP_PID" 2>/dev/null
kill %1 2>/dev/null
```

- Open the PNG to confirm the change (height, axis side/units, label
  legibility) instead of guessing.
- `import -window root` captures the full screen; the app window is in the
  top-left, which is where the graph row lives.
- The binary must be built first (`cargo build`); use the debug build.
- This is a **manual eyeball check** complementing the gate — it does not
  replace `cargo test` / clippy / fmt, and it is not part of the commit gate.

## Important Notes

- Read Rust crate docs in `generated_docs/<crate>` (refresh with `cargo docs-md docs`)
- Read GTK4 examples and docs before writing UI code
- Validate logic with tests (do not run the app to confirm behavior); use the
  **Graphical Verification** section only to eyeball layout/UI changes
- Always update README.md when adding/modifying features
