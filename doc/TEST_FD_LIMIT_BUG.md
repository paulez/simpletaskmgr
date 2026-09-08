# `cargo test` failures from a low open-fd limit

Status: **resolved (environmental, not a code defect)** — 2026-09-08

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

## Root cause — one thing: an open-file-descriptor ceiling

Both are the **same** problem: the process's soft open-fd limit (`ulimit -n`)
is too low for GTK/glib under a parallel test run.

- `cargo test` runs the lib tests on a thread pool (default ≈ number of CPUs).
- GTK/glib lazily creates a **per-OS-thread `GMainContext`**; each holds a
  wakeup pipe pair plus signal/unix-socket sources (`GWakeup`). Across ~320
  tests that accumulate fds until the limit is hit.
- When glib then needs another pipe and `pipe2()` returns `EMFILE` ("Too many
  open files"), it calls `abort()` → **signal 5 / SIGTRAP**. This is a hard OS
  abort, **not catchable in Rust** — the whole test binary dies. That's failure
  #1.
- Failure #2 (`test_init_populates_unique_rows`) is a *casualty of the same
  pressure*, not a second bug: by the time that test runs on another thread,
  fds are already exhausted, so the `/proc/[pid]` reads inside
  `ProcessList::init()` fail and `update_process_list` runs its
  `Err → self.processes.clear()` branch, leaving the list empty. The
  "must see at least the current process" assert trips on the consequence.

So there is nothing to fix in the library: the app itself runs fine (it was
used to confirm the cell-desync fix); only the parallel test binary reaches
the wall.

## Reproduction

Lower the fd limit and run the default (parallel) suite:

```bash
bash -c 'ulimit -n 256; cargo test --lib'
# → (process:…): GLib-ERROR **: … Creating pipes for GWakeup: Too many open files
# → process didn't exit successfully: … (signal: 5, SIGTRAP: …)
```

Confirm the two mitigations:

```bash
bash -c 'ulimit -n 256; cargo test --lib -- --test-threads=1'   # 321 passed
bash -c 'ulimit -n 65535; cargo test --lib'                     # 321 passed
```

Environment where it reproduces: any shell whose soft `nofile` limit is
~1024–4096 (tmux, VS Code's terminal, Docker default, a restrictive
`/etc/security/limits.conf`). Note a healthy limit (~524288) never shows it,
which is why it can pass in one session and fail in another.

## The fix

### Option A — raise the fd limit (preferred; keeps parallel tests)

Check what you're running with, then raise it for the session:

```bash
ulimit -n                 # current soft limit
ulimit -n 1048576         # raise, then
cargo test
```

If the hard cap won't allow it, raise it persistently:
- **Docker:** `docker run --ulimit nofile=1048576:1048576 …`
- **`/etc/security/limits.conf`** (or a systemd `LimitNOFile=` on the user's
  slice):
  ```
  *  soft  nofile  1048576
  *  hard  nofile  1048576
  ```

### Option B — serialize the test threads (no limit change)

```bash
cargo test -- --test-threads=1
```

Slower, but a clean one-liner; GTK's per-context fd usage then stays within a
low limit because contexts don't pile up across 8+ concurrent threads.

## What we did NOT do

- No code changes. The library is correct; the failure is an OS-resource
  ceiling on the *test harness*, surfaced through GLib's `abort()`.
- Did not make `test_init_populates_unique_rows` lenient about an empty list —
  that would mask a genuine "init reads zero processes from /proc" regression.
  Its `assert!(… !list.processes.is_empty())` should stay: it is a valid
  invariant when the environment can actually read `/proc`.

## Cross-references

- `src/process_list.rs:71-74` — the `Err → self.processes.clear()` branch that
  produces the empty list under fd pressure.
- `src/process_list.rs:393` — the `!is_empty()` assert, valid but sensitive to
  this environment issue.
- Gate: `cargo check && cargo test && cargo clippy --all-targets && cargo fmt --check`
  (AGENTS.md). Under a low `ulimit -n`, `cargo test` needs `--test-threads=1`
  or a raised `nofile`.
