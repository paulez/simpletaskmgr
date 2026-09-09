# `cargo test` failures from a low open-fd limit

Status: **in progress — root fix (hermetic unit tests) underway — 2026-09-09**

Companion to `TEST_FD_LIMIT_FIX_PLAN.md`, which holds the current plan and the
concrete next steps. This file records *what the bug was* and *the honest
evolution of the fix*.

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
   thread '…' panicked at src/process_list.rs:…:
   init must see at least the current process
   ```

A hard `abort()` from `glib` when `pipe2()` returns `ENFILE`/`EMFILE` is signal
5 / SIGTRAP — an OS-level kill, **not catchable in Rust**; the whole test binary
dies. The empty-list assertion (the `process_list` one) is a *casualty* of the
same fd exhaustion (see root cause), not a second bug.

Environment where it reproduces: any shell whose soft `nofile` limit is ~256-4096
(Docker default, tmux, VS Code's terminal, a restrictive
`/etc/security/limits.conf`). A healthy limit (~524288) never shows it, so it is
session-dependent.

## Root cause — the honest version

**The fd hog is the live `/proc` walk, not the GPU probe.**

`ProcessList::init()` / `refresh_process_list()` (src/process_list.rs) enumerates
*every* `/proc/[pid]` entry — **≈420 on this host** — and for each opens
`stat`/`status`/`statm`/`io`. `SystemMetrics::push_sample()` (src/metrics.rs)
additionally reads `/proc/{stat,meminfo,uptime,diskstats}` and one `/sys`
cpufreq + hwmon sensor.

Previously *many* **unit** tests triggered these live reads: the ~15
`ui::tests` (each built `State` via `with_settings_path` → `init()` →
`push_sample`) plus the `process_list` / `metrics` / `cpu_status` / `disk_status`
live tests, all running on the default 16-thread pool. That is thousands of
short-lived procfs opens in flight under a `nofile` of 256 → `EMFILE`.

Two earlier hypotheses were checked and set aside:

- **`serial_test` (commit `d330111`)** — serialised the fd-sensitive tests. It
  worked but **hid the defect** and slowed the whole pool. Reverted.
- **"stop spawning `rocm-smi`"** — **disproven as the root cause**. Removing the
  two live `rocm-smi` unit tests and re-running under `ulimit -n 256` *still*
  left `ui::tests` failing flakily. `rocm-smi` is at most a minor contributor;
  the `/proc/[pid]` walk is the real cost. (Data point: the single live
  `process_list` init test passed 3/3 in isolation, but the batched `ui::tests`
  failed 1/3 — it is the *combination* under the 16-thread pool, not any single
  reader, that crosses the limit.)

## Current fix state (this commit)

The direction is **separate hermetic unit tests from integration tests**:

- **`serial_test` removal** — the `#[serial_test::serial]` markers and the dev-dep
  are gone (reverted to the pre-`d330111` dependency state).
- **`ui::tests` made hermetic** (the main fd hog): `src/ui.rs` now has a
  fixture constructor `State::with_processes(path, items)` — `ProcessList::new()`
  (no `/proc` walk), empty `metrics` (no `/proc/stat` read), `gpu_available:false`.
  `ui::tests::test_state` (src/ui.rs:1221) uses it. The six `ui` tests that need
  live I/O (`refresh`, `kill`-on-live-child, "primes metrics") were removed from
  the unit suite and will return as **integration** tests.
- **`process_list` live init test removed** from the unit suite
  (`test_init_populates_unique_rows` is gone).
- **Public API for integration tests**: `State`, `KillStatus`, and the
  `refresh`/`kill`/`set_*`/`take_timer_id` methods are now `pub` so
  `tests/*.rs` can drive the same code paths that used to be unit-tested.
- **`rocm-smi` covered by ONE integration test**
  (`tests/integration_tests.rs::test_read_gpu_card_sane_when_present`), in its
  own binary where a single spawn cannot contend with the pool.
- Production behaviour is **unchanged**: `State::with_settings_path` still does
  the identical `init()` + GPU probe + first sample.

**Not yet done** (tracked as N1–N4 in `TEST_FD_LIMIT_FIX_PLAN.md`):
- Move the remaining live-I/O unit tests (`metrics::test_push_sample_*`,
  `cpu_status::test_read_cpu0_freq/temp_*`, `disk_status::test_snapshot_*`) into
  `tests/*.rs` as integration tests.
- Write the integration coverage that replaces the six removed `ui` tests.
- Acceptance bar: `bash -c 'ulimit -n 256; cargo test --lib'` green ≥5×, and the
  full `cargo test` (lib + integration) green.

## What we did NOT do

- Did not make `ProcessList` tolerant of an empty list — that would mask a real
  "init reads zero processes" regression. The invariant stays valid in
  integration tests that can read `/proc`.
- Did not ship a `.cargo/config.toml` `[env] RUST_TEST_THREADS` cap, and did not
  use `--test-threads=1` — both are user/tool-level mitigations that hide the
  defect and slow the ~300 pure-logic tests. The fix is meant to be in test
  *structure* (unit = hermetic, integration = real I/O), which is visible in
  source.
- Did not re-add `#[serial_test::serial]` as a stop-gap; only `d330111` is the
  rollback if the hermetic path proves infeasible for the remaining live tests.

## Cross-references

- `src/process_list.rs` — `ProcessList::init()` / `refresh_process_list()`
  (the per-`/proc/[pid]` walk that is the real fd hog); the
  `Err → self.processes.clear()` branch is what produced the empty-list casualty.
- `src/metrics.rs` — `push_sample()` / `sample_cpu()` (`/proc/stat`,
  `/proc/meminfo`, `/proc/diskstats`, `/sys` cpufreq+hwmon).
- `src/ui.rs:1221` — `ui::tests::test_state`, now fixture-based via
  `State::with_processes` (src/ui.rs). The live paths live in
  `tests/integration_tests.rs`.
- `tests/integration_tests.rs` — the real-I/O coverage (live `rocm-smi`, the
  live `/proc` process list) in its own binary.
- Gate: `cargo check --all-targets && cargo test && cargo clippy --all-targets
  && cargo fmt --check` (AGENTS.md). Acceptance under a low limit:
  `bash -c 'ulimit -n 256; cargo test --lib'`.
