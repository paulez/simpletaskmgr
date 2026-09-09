# Make the unit-test suite hermetic: no live `/proc` walks in the parallel pool

Status: **in progress — 2026-09-09**

Supersedes the runtime mitigation shipped in `d330111` (`#[serial_test::serial]`
markers across 5 test modules + the `serial_test` dev-dep), and the earlier
"stop spawning `rocm-smi`" theory in this same file. **The rocm-smi theory was
disproven** (see "Why the earlier theories don't hold"). This plan separates
*unit* tests (pure, no external I/O) from *integration* tests (the small set
that reads the real system), so the parallel lib pool can never saturate the
process open-file budget.

## The rule (applies to **all** tests that touch external data)

- **Unit tests** (the `#[cfg(test)] mod tests` inside `src/*`) must be
  **hermetic**: their result must not depend on the presence, timing, speed, or
  content of anything outside the test process — no `/proc/[pid]` walks, no
  `/proc/{stat,meminfo,diskstats,uptime}`, no `/sys/…` cpufreq/hwmon reads,
  no `rocm-smi` spawn. They use fixtures / pure functions.
- **Integration tests** (the `tests/*.rs` binaries) are the *small* set that
  read the real source: the live `/proc` process list, the live `rocm-smi`,
  the live cpu freq/temp, the live disk snapshots. These run in their own
  binary, outside the lib's parallel pool, where only a handful of such reads
  are in flight at a time — so they cannot saturate the fd table.

## Why the earlier theories don't hold

1. **serial_test (d330111)** — worked, but hid the defect and slowed the whole
   pool. Removed. Not the root fix.
2. **"stop spawning rocm-smi"** — disproven. Deleting the two live `rocm-smi`
   unit tests and re-running under `ulimit -n 256` still left `ui::tests`
   failing flakily. `rocm-smi` is at most a minor contributor.

**The real fd hog:** the per-user process table walk. `ProcessList::init()` →
`refresh_process_list()` enumerates **every** `/proc/[pid]` entry (≈420 on this
host) and, for each, opens `stat`/`status`/`statm`/`io` — plus `push_sample()`
reads `/proc/{stat,meminfo,uptime,diskstats}` and one `/sys` cpufreq + hwmon
sensor. Previously ~15 `ui::tests` (each `State::with_settings_path` → `init()`
→ `push_sample`) plus the `process_list`/`metrics` live tests ran on the default
16-thread pool. That is thousands of short-lived procfs opens in flight under a
`nofile` of 256 → `EMFILE`. The lib pool amplifies it; the integration binary
does not (few reads, sequential).

Data points (no-spawn build, `ulimit -n 256`): the single live `process_list`
init test passed 3/3 alone, but the batched `ui::tests` failed 1/3 — i.e. it is
the *combination* under the pool, not any single reader, that crosses the limit.

## Changes — DONE (this commit, uncommitted at write-time)

### `src/ui.rs`
- **`State`, `KillStatus` and the methods integration tests need are now
  `pub`** (`State`, `KillStatus::Sent/NoSelection/Failed`, `refresh`, `kill`,
  `set_show_all`, `set_refresh_interval`, `reset_settings`,
  `take_timer_id`), so `tests/*.rs` can drive the same code paths.
- **`State::with_settings_path`** restored to its real production body
  (fixes the broken `with_settings_path_and_gpu` reference / `E0599`):
  `ProcessList::init()` → `gpu_available()` probe → first `push_sample`.
  Production behaviour is unchanged.
- **`State::with_processes(path, items)`** — the **fixture** constructor.
  `ProcessList::new()` (no `/proc` walk), `metrics` empty (no `/proc/stat`
  baseline read), `gpu_available: false`, `selected_pid: None`. This is what
  `ui` unit tests build `State` from.
- **`ui::tests::test_state`** now calls `with_processes(path, vec![item(123)])`.
  The six tests that do live I/O are **removed from the unit suite** (they will
  return as integration tests): `test_state_new_primes_metrics`,
  `test_refresh_appends_one_metric_sample`, `test_refresh_preserves_selection`,
  `test_kill_sigkill_on_sleep_child`, `test_refresh_drops_killed_process`,
  `test_state_initializes_from_saved_settings`.
- The remaining `ui::tests` are pure-control-flow (settings round-trip,
  `kill` no-selection / dead-pid, `find`, `reset`, timer slot) — no `/proc`,
  no `refresh`.

### `src/process_list.rs`
- Removed `test_init_populates_unique_rows` (live `/proc/[pid]` walk).

### `src/gpu_status.rs` / `Cargo.{toml,lock}` / `tests/integration_tests.rs`
- Removed the 2 live `rocm-smi` unit tests; kept the pure `parse_gpu_json`
  fixture tests.
- Added integration `test_read_gpu_card_sane_when_present`.
- Dropped the `serial_test` dev-dep and all `#[serial_test::serial]` markers.

## Changes — NEXT (not yet done)

### N1. Move the remaining live-I/O **unit** tests to integration
These still read the real system and must leave the lib pool:
- `src/metrics.rs`: `test_push_sample_*` (caps_history, updates_history,
  carries_freq, carries_gpu, carries_disks) and
  `test_second_cpu_sample_is_measured_not_stuck_zero` (all call `push_sample`
  / `sample_cpu` → `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/sys`).
- `src/cpu_status.rs`: `test_read_cpu0_freq_mhz_sane_when_present`,
  `test_read_cpu_temp_c_sane_when_present` (live `/sys/cpufreq`, `/sys/hwmon`).
- `src/disk_status.rs`: `test_snapshot_sane_when_present`,
  `test_snapshot_only_physical_disks` (live `/proc/diskstats`, `/proc/uptime`).

(The `cpu_status` hwmon `pick_*`/`hottest_*` tests are **fine to keep** — they
build their own temp dirs, not the real `/sys` tree.)

### N2. Write the corresponding integration tests
One per removed area, in `tests/integration_tests.rs`, using the now-`pub`
items: `ProcessList::init()` unique-rows + non-empty; `SystemMetrics`
`push_sample` bookkeeping + freq/gpu/disk carry-over; cpu second-sample; a
`State::with_settings_path` + `refresh` tick appends exactly one sample and
preserves the selection; the kill-`sleep`-child + refresh-drops-it contract.
These run one binary at a time → no fd saturation.

### N3. (Optional, only if a test *needs* it) make `SystemMetrics::sample_cpu`
and its `stat_baseline` field accessible from the binary — e.g. `pub(crate)`
is insufficient for `tests/`; either add a thin `pub` accessor or cover the
"second sample is measured, not stuck zero" behaviour through the public
`push_sample`/`history()` API (preferred, keeps the surface small).

### N4. Gate + verify (the acceptance bar)
```bash
cargo check --all-targets && cargo test && cargo clippy --all-targets && cargo fmt --check
bash -c 'ulimit -n 256; cargo test --lib'   # repeat ≥5× — the lib pool must NOT drop to SIGTRAP
bash -c 'ulimit -n 256; cargo test'         # + integration, still green
```
The suite that currently SIGTRAPs is the **lib** pool; the acceptance check is
`cargo test --lib` under `ulimit -n 256`.

## Rollback

The suite was fully green at `d330111`. To roll back everything:
`git revert` this commit (or `git checkout d330111 -- src tests Cargo.toml
Cargo.lock`) restores the serial_test mitigation.

## Risk / notes
- **Production behaviour is unchanged**: `State::with_settings_path` still does
  the identical `init()` + probe + sample; only the *test* entry point changed.
- Non-GPU hosts (CI) are unaffected — `read_gpu_card()` degrades to `None`.
- A future contributor who adds external I/O must put it in an **integration**
  test, not a `#[cfg(test)]` unit test — the `test_state` and `with_processes`
  doc comments say so.
