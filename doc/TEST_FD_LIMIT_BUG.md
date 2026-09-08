# `cargo test` failures from a low open-fd limit

Status: **resolved (fixed in tree) — 2026-09-08**

## Symptom

`cargo test` fails with two distinct-looking errors:

1. **Hard abort** (the first, and the real one):
   ```
   (process:…): GLib-ERROR **: …: Creating pipes for GWakeup: Too many open files
   error: test failed, to rerun pass `--lib`
   Caused by:
     process didn't exit successfully: `…/simpletaskmgr-…` (signal: 5, SIGTRAP: trace/breakpoint trap)
   ```
2. **An unrelated-looking assertion** in a completely different test:
   ```
   ---- process_list::tests::test_init_populates_unique_rows stdout ----
   thread '…' panicked at src/process_list.rs:393:9:
   init must see at least the current process
   ```

## Root cause

The process-global soft open-fd limit (`ulimit -n`) is too low for the
parallel test suite. Two tests compete for that budget:

- **`ui::tests`** (src/ui.rs:1153): each test constructs a `State`
  (src/ui.rs:111), which on a GPU host spawns `rocm-smi … --json` twice
  (src/gpu_status.rs:81, from `gpu_available()` and `push_sample`) and
  realizes the thread's `glib::MainContext` (a `GWakeup` pipe pair plus
  signal sources). The `rocm-smi` child inherits *every* parent fd.
  Running several of these in parallel against a low limit exhausts the
  process fd table.
- **`process_list::tests::test_init_populates_unique_rows`**
  (src/process_list.rs:383): `ProcessList::init()` opens one fd per live
  `/proc/[pid]` directory for `statm`/`status`/`io`. On this host that is
  ~111 opens in a single test. If it overlaps the `rocm-smi` spawns, the
  process has run out of fds by the time the second batch of `/proc`
  entries is read, and the `Err → self.processes.clear()` branch
  (src/process_list.rs:71) leaves the list empty. The
  `assert!(… !list.processes.is_empty())` trips on the consequence — this
  test is a *casualty*, not a second bug.

When glib then needs another pipe and `pipe2()` returns `EMFILE`, it calls
`abort()` → signal 5 / SIGTRAP. That is a hard OS abort, **not catchable in
Rust** — the whole test binary dies.

Environment where it reproduces: any shell whose soft `nofile` limit is
~256-4096 (Docker default, tmux, VS Code's terminal, a restrictive
`/etc/security/limits.conf`). A healthy limit (~524288) never shows it,
which is why it can pass in one session and fail in another.

## The fix — `serial_test` in-tree

We pinned `serial_test = "4"` (a dev-only dep) and marked the two
fd-hungry tests with `#[serial_test::serial]`:

- `src/ui.rs:1153` — the `mod tests` block gets a module-level
  `#[serial_test::serial]`, so no two `ui::tests` run at the same time
  (and hence no two `rocm-smi` spawns are in flight simultaneously).
- `src/process_list.rs:387` — `test_init_populates_unique_rows` also gets
  `#[serial_test::serial]`, so it never overlaps the `rocm-smi` / glib
  context-creation window that is exhausting the fd budget.

The 11 `refresh_list` tests, the 6 `process_row` tests, and the 4
`cell_label` tests are also marked `#[serial]` for the same reason — the
glib-object tests in this codebase do in fact create a per-thread
`GWakeup` context (each test is a glib-object test and thus the first
such test on its worker thread triggers the context realisation), so
leaving them free to overlap with the `rocm-smi`-spawning tests would
re-open the same race.

Serializing these tests is enough to keep concurrent open-fd usage under a
soft limit of **256** (the lowest "reasonable" developer-machine value;
Docker default is 1024). Verification:

```bash
bash -c 'ulimit -n 256; cargo test --lib'
# → 321 passed, 5/5 consecutive runs green (2026-09-08)

cargo test
# → 321 lib tests + 3 integration tests + doc-tests all green
```

## What we did NOT do

- Did not make `test_init_populates_unique_rows` lenient about an empty
  list — that would mask a genuine "init reads zero processes from /proc"
  regression. Its `assert!(… !list.processes.is_empty())` must stay: it is
  a valid invariant when the environment can actually read `/proc`.
- Did not ship a `.cargo/config.toml` `[env] RUST_TEST_THREADS` cap. A
  tool-level cap was the right first hypothesis, but: (a) the glib-object
  tests in this crate are *mostly* zero-fd (a `glib::Object` subclass
  doesn't create a `GWakeup` context on every thread until one actually
  needs to run a signal source) and the real fd hog was `ui::tests`
  spawning `rocm-smi`; (b) capping `RUST_TEST_THREADS` to 2 also slowed
  the ~300 pure-logic tests needlessly; (c) you asked for the fix to be
  "contained in cargo" in the sense of *dev-deps and test code*, not in a
  hidden config file. `#[serial_test::serial]` is exactly the right
  scope: only the fd-sensitive tests are affected, at no cost to the rest
  of the suite, and the attribute is a *visible* source-level marker a
  future maintainer can find and reason about.

- Did not use `--test-threads=1` — that still works, is slower, and is
  now a user-level mitigation rather than a code-level one.

## Cross-references

- `src/gpu_status.rs:81` — the `rocm-smi` `Command::output()` call that
  is the parent-fd holder during the GPU probe.
- `src/process_list.rs:71` — the `Err → self.processes.clear()` branch
  that produces the empty list under fd pressure.
- `src/process_list.rs:387` — `test_init_populates_unique_rows` with its
  `#[serial]` marker and the `assert!(… !is_empty())` invariant.
- `src/ui.rs:111-141` — `State::with_settings_path` (constructs `State`
  from `UserSettings::load`, spawns the `gpu_available()` probe, primes
  the metric history, and calls `read_cpu0_freq_mhz` / `read_mem_total_mb`
  which open a handful of `/proc` files).
- Gate: `cargo check && cargo test && cargo clippy --all-targets && cargo fmt --check`
  (AGENTS.md). Under a low `ulimit -n`, `cargo test` now works without
  extra flags: `#[serial_test::serial]` keeps all fd-hungry tests off the
  worker pool at once.
