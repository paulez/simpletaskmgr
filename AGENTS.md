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

Work in small, focused changes. For **each** change, follow this gate and
**commit it as soon as the gate passes** — do not batch several changes into
one commit:

1. **Implement** one small change
2. **Add tests** in `#[cfg(test)] mod tests`, run `cargo test` (new + existing all pass)
3. **Lint** with `cargo clippy --all-targets` (must pass with 0 warnings)
4. **Format** with `cargo fmt`
5. **Commit** with a clear message (no `Co-Authored-By` or AI attribution)

**Commit rules:**
- **Always commit after implementing a change**, once the gate above passes. The
  user pulls and tests remotely between changes, so a committed, green state is
  the only way to test it — do not leave changes uncommitted in the working tree.
- Commit **per logical change** (a feature, a fix, a readability tweak); keep the
  history granular so `git log` makes it easy to see exactly what was done.
- **Track what has been done**: the commit message is the record. State, in
  imperative form, what changed and why; name the feature/fix. Prefer one short
  subject line + a body of 1–3 lines over a vague one-liner.
- Never commit failing tests or clippy warnings
- Do not push unless explicitly asked
- Use `rustc --explain <error number>` for build errors
- Read GTK4 documentation when adding UI components (see https://gtk-rs.org/docs-rs/latest/ or gtk-rs source for patterns)
- Keep UI code reactive: bind data to signal handlers, update UI when state changes

## Important Notes

- Read Rust crate docs in `generated_docs/<crate>` (refresh with `cargo docs-md docs`)
- Read GTK4 examples and docs before writing UI code
- Validate logic with tests (do not run the app to confirm behavior); use the
  **Graphical Verification** section only to eyeball layout/UI changes
- Always update README.md when adding/modifying features
