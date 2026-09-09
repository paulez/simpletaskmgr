# Make the unit-test suite hermetic: no live `/proc` walks in the parallel pool

Status: **done — 2026-09-09**

Supersedes the runtime mitigation shipped in `d330111` (`#[serial_test::serial]`
markers across 5 test modules + the `serial_test` dev-dep), and the earlier
"stop spawning `rocm-smi`" theory in this same file. **The rocm-smi theory was
disproven** (see "Why the earlier theories don't hold"). This change separates
*unit* tests (pure, no external I/O) from *integration* tests (the set that
reads the real system), so the parallel lib pool can never saturate the process
open-file budget.

## The rule (applies to **all** tests that touch external data)

- **Unit tests** (the `#[cfg(test)] mod tests` inside `src/*`) must be
  **hermetic**: their result must not depend on the presence, timing, speed, or
  content of anything outside the test process — no `/proc/[pid]` walks, no
  `/proc/{stat,meminfo,diskstats,uptime}`, no `/sys/…` cpufreq/hwmon reads,
  no `rocm-smi` spawn. They use fixtures / pure functions.
- **Integration tests** (the `tests/*.rs` binaries) are the small set that read
  the real source: the live `/proc` process list, the live `rocm-smi`, the
  live cpu freq/temp, the live disk snapshots. These run in their own binary,
  outside the lib's parallel pool. Because `RUST_TEST_THREADS` still applies
  inside the binary, the heavy live walks there are consolidated into a small
  number of tests (see N2 below) so the concurrent `/proc` opens stay well
  under a low `ulimit -n`.

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
`nofile` of 256 → `EMFILE`. The lib pool amplifies it; a single heavy reader
does not.

Data points (no-spawn build, `ulimit -n 256`): the single live `process_list`
init test passed 3/3 alone, but the batched `ui::tests` failed 1/3 — i.e. it is
the *combination* under the pool, not any single reader, that crosses the limit.

## Changes — DONE

### Commit `fce2bfb` (hermetic `ui` unit suite)

#### `src/ui.rs`
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
  The six tests that do live I/O (state primes metrics, refresh appends one
  sample, refresh preserves selection, kill-`sleep`, refresh-drops-killed,
  settings round-trip) were removed from the unit suite — re-added as
  integration tests below.
- The remaining `ui::tests` are pure-control-flow (settings round-trip,
  `kill` no-selection / dead-pid, `find`, `reset`, timer slot) — no `/proc`,
  no `refresh`.

#### `src/process_list.rs`
- Removed `test_init_populates_unique_rows` (live `/proc/[pid]` walk) —
  re-added as an integration test below.

#### `src/gpu_status.rs` / `Cargo.{toml,lock}`
- Removed the 2 live `rocm-smi` unit tests; kept the pure `parse_gpu_json`
  fixture tests.
- Dropped the `serial_test` dev-dep and all `#[serial_test::serial]` markers.

### Current commit (hermetic `metrics` / `cpu_status` / `disk_status` unit suite)

#### N1 — moved the last live-I/O unit tests out of the lib pool
- `src/metrics.rs`: removed `test_push_sample_caps_history`,
  `test_push_sample_updates_history_and_last`, `test_push_sample_carries_freq`,
  `test_push_sample_carries_gpu`, `test_push_sample_carries_disks` (all call
  `push_sample` → `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/sys`).
  (The plan mentioned `test_second_cpu_sample_is_measured_not_stuck_zero` —
  that test does not exist in the tree; the second-sample contract is covered
  by the disk/freq `carries_*` tests.)
- `src/cpu_status.rs`: removed `test_read_cpu0_freq_mhz_sane_when_present`,
  `test_read_cpu_temp_c_sane_when_present` (live `/sys/cpufreq`, `/sys/hwmon`).
  The `pick_*`/`hottest_*` hwmon tests stay — they build their own temp dirs.
- `src/disk_status.rs`: removed `test_snapshot_sane_when_present`,
  `test_snapshot_only_physical_disks` (live `/proc/diskstats`,
  `/proc/uptime`).

#### N2 — added/consolidated the integration coverage
`tests/integration_tests.rs` now contains the relocated tests plus the six
relocated `ui` live tests, **consolidated wherever they share the same heavy
live walk** so the binary's 16-thread pool cannot saturate a low `ulimit -n`:

- **One sequential test — `test_state_lifecycle`** — owns every full
  `/proc/[pid]` enumerator, running them strictly in sequence as steps (a)–(g):
  (a) `with_settings_path` primes metrics, (b) `refresh` appends one sample,
  (c) `refresh` preserves selection, (d) `kill(SIGKILL)` on a live `sleep`
  child, (e) `refresh` drops the killed process, (f) saved-settings round-trip,
  and (g) `refresh_process_list` returns a non-empty, unique-pid, fully-populated
  row set. Keeping exactly **one** full walk in flight is the key: two
  concurrent 420-entry `/proc` walks already exhaust a `nofile` of 256, so the
  old separate `test_init_populates_unique_rows` /
  `test_process_names_*` trio failed in concert at 256 even though each passed
  alone.
- **Light single-file readers stay in their own small tests** (each opens only a
  couple of procfs/sysfs fds, so they cannot saturate the table):
  `test_push_sample_{caps_history,updates_history_and_last,carries_freq,
  carries_gpu,carries_disks}` (`/proc/{stat,meminfo,diskstats,uptime}` +
  `/sys` cpufreq), `test_read_cpu0_freq_mhz_sane_when_present`,
  `test_read_cpu_temp_c_sane_when_present` (`/sys` cpu0 + hwmon),
  `test_snapshot_sane_when_present`, `test_snapshot_only_physical_disks`
  (`/proc/diskstats`).
- `test_read_gpu_card_sane_when_present` (from `fce2bfb`) — one `rocm-smi`
  spawn.

#### N3 — no API changes needed
`State`, `KillStatus`, `SystemMetrics`, `GpuSample`, `DiskSample`,
`ProcessList`, `ProcessItem`, `TaskMgrProcess`, `UserSettings`,
`Signal`, `RefreshInterval` and the relevant methods (`push_sample`,
`history`, `with_settings_path`, `with_processes`, `refresh`, `kill`,
`snapshot`, `init`, `refresh_process_list`) were all already `pub`, so the
integration binary could reach them without widening the API surface.

#### N4 — acceptance bar
```
cargo check --all-targets                      → clean
cargo test                                     → 299 lib + 6 + 13 integration + 0 doc-tests, all pass
cargo clippy --all-targets                     → clean
cargo fmt --check                              → clean

bash -c 'ulimit -n 256; cargo test --lib'      → 5/5 green (the SIGTRAP source pool)
bash -c 'ulimit -n 256; cargo test'            → 3/3 green (lib + integration together)
```

## Rollback

The suite was fully green at `d330111`. To roll back the whole hermetic
change, `git revert` the two commits (`fce2bfb` and this one); to roll back
only this commit, `git revert HEAD`.

## Risk / notes
- **Production behaviour is unchanged**: `State::with_settings_path` still
  does the identical `init()` + probe + sample; only the *test* entry points
  and their location changed.
- Non-GPU hosts (CI) are unaffected — `read_gpu_card()` degrades to `None`.
- Integration tests that assert "non-empty" results (`test_state_lifecycle`
  sub-checks, `test_process_list_walk_sane_when_populated`, the `push_sample`
  bookkeeping) degrade to `None`/empty under fd pressure, matching the
  documented "casualty" path for `Err → processes.clear()`.
- A future contributor adding external I/O **must** put it in an integration
  test, not a `#[cfg(test)]` unit test — the `test_state` and
  `with_processes` doc comments state this.
- A future contributor adding a *new* heavy live walk (a `#[test]` that
  enumerates `/proc/[pid]`) should consolidate it with the
  `all_processes`-family tests in the integration binary to avoid adding
  another concurrent enumerator to the pool.
