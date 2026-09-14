# Test & Release Plan (1.0.0)

Status: in preparation. This document is the checklist walked before and
during each step of the `1.0.0-beta.1` → `1.0.0-rc.1` → `1.0.0` ladder.
It covers what is tested, how, and the exact release mechanics (the
`release` workflow publishes from a tag — see the README's `## Releases`
section for the summary).

## What 1.0.0 delivers over 0.1

- **Terminate (SIGTERM) / Kill (SIGKILL) buttons** in the process detail
  pane: `Signal::Sigterm` added to `Signal`, the detail pane offers the
  two buttons with a status line, and the app-side handler delivers via
  `kill(2)` through the existing `State::kill` path (spec D5 implemented).
- **A failure pop-up** when a signal cannot be delivered (EPERM/ESRCH):
  the pane status line and a dismissible dialog both report the error;
  the process is left exactly as it was.
- **Expanded, compact process detail pane**: the pane now shows the full
  command line (wrapping, selectable to copy), state (Sleeping/Running/…),
  thread count, signed nice, absolute memory (RSS in a human unit), start
  time, and a running uptime — while the values the row already shows
  (PID, CPU%, MEM%, disk speed) stay in the list, keeping the pane compact
  enough to fit the window at default size; unknown values render a `—`
  placeholder, never a misleading zero.
- Signal delivery is covered by unit tests, an integration test, and a
  live test (send SIGTERM to a spawned `sleep` and assert it exits).

## Test gate (run on every change)

```bash
cargo check && cargo test && cargo clippy --all-targets && cargo fmt --check
```

CI (`rust.yml`) runs the same gate on every push and PR — a green local
gate is the definition of done for each commit.

## Test plan

### 1. Headless build & smoke render

```bash
./tools/test_ui_render.sh 0 1 3 5
```

- App starts inside the `cage` compositor, renders all three graph tabs,
  the process list, and settles per the delay points.
- Visual pass: no missing widgets, correct ordering (CPU%-desc default),
  legends below each graph.
- **New for 1.0.0**: the list renders cleanly on a machine with many
  processes (thousands of PIDs) without layout breakage.

### 2. Functional GUI test (real session)

| # | Scenario | Expected |
|---|---|---|
| 1 | Start app; select a process | Detail pane shows name, command line, state, threads, nice, memory, started, uptime, and the two signal buttons — and **fits the window at default size** (no inner scrolling; PID/CPU%/MEM%/disk columns from the list are not duplicated). |
| 2 | Click **Terminate** on a long-running benign process you own (e.g. `sleep 300` started from a terminal) | Pane status line shows `SIGTERM sent to pid N`; the process exits; the list row disappears within one refresh; graph tabs unaffected. |
| 3 | Click **Kill** on the same kind of process | `SIGKILL sent to pid N`; process exits immediately; row disappears. |
| 4 | Click **Terminate** on a root-owned process (e.g. `gnome-terminal-server`) while running unprivileged | Button does **not** crash the app; the pane status line **and the pop-up error dialog** both show an EPERM-derived message (the send failed, no process changed); dismissing the dialog leaves the pane and list intact. |
| 5 | Click **Terminate** then immediately **Kill** on the same target (if it survived) | Second click behaves correctly for the (possibly now-zombie) pid; no assertion/abort. |
| 6 | Deselect (click header / empty area) | Detail pane hidden (NO_SELECTION); status line resets on the next selection. |
| 7 | Select a *different* process right after a failed delivery | Status line is cleared by the selection change (no stale red text). |
| 8 | Fast refresh (0.5 s) while clicking | Signals still delivered to the selected PID (pinning), not to a shifted row. |
| 9 | Select the running `simpletaskmgr` itself | Detail pane: full command line visible, State `Sleeping`, Threads ≥ 1, Memory a human-readable size consistent with the MEM% column, Started a plausible time, Uptime climbing across refreshes. |
| 10 | Select a kernel thread (e.g. a `kworker` row) | Unknown fields show the `—` placeholder (command line is empty); a negative nice renders signed (e.g. `Nice: -20`). |

### 3. Signal-delivery correctness (unit/integration)

Already covered, but re-run and re-verify:

- `src/signal.rs` — `Signal::{Sigterm,Sigkill}::name()`/`number()`;
  delivery tests for both variants (the existing SIGKILL live test plus a
  new SIGTERM live test).
- `src/ui.rs::State::kill` — sent/success, EPERM, EINVAL mapping (existing
  tests, including the new SIGTERM path).
- `tests/process_view_gtk.rs` — `test_signal_buttons_forward_and_status`
  (buttons carry the right signals, status line set/clear/replace, last
  registered handler wins semantics).

### 4. Regression sweep (unchanged subsystems)

- List: sort by each column (asc/desc), refresh-in-place, no flicker,
  blank-value rows bottom-pinned (existing GTK tests + smoke render).
- Graphs: CPU/Mem + Disk I/O + GPU (if present) tabs render; legends
  correct; time window stable across refreshes.
- Settings: `show_all`, `refresh` toggles persist across restarts;
  corrupt-file fallback.
- No `dbg!`/`println!` in the binary; `-v` logging still works.

### 5. Release-artifact verification (on a clean Debian 13 x86_64 VM/container)

```bash
# For a prerelease the package name carries its version — for
# 1.0.0-beta.1 that is simpletaskmgr-1.0.0-beta.1-x86_64-linux.
tar xzf simpletaskmgr-1.0.0-x86_64-linux.tar.gz
sha256sum -c SHA256SUMS
./simpletaskmgr-1.0.0-x86_64-linux/
```

- Binary runs unmodified (GTK 4.18 present); list, tabs, and the detail
  pane all appear — select a row and confirm the full field set (command
  line, state, threads, nice, memory, started, uptime) renders.
- One **Terminate** click against a `sleep 300` process works end-to-end,
  and a failed one (root-owned target) pops the error dialog.
- No GPU tab is shown on a machine without `rocm-smi` (correct hiding),
  and the rest of the app is unaffected.

Before the prerelease is published, the same artifact can be produced
locally (exactly what the workflow does):

```bash
cargo build --release --locked
cd /tmp && rm -rf rtest && mkdir -p rtest/dist && cd -
V=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
PKG="simpletaskmgr-${V}-x86_64-linux"
mkdir -p "/tmp/rtest/dist/${PKG}"
cp target/release/simpletaskmgr "/tmp/rtest/dist/${PKG}/"
cp README.md LICENSE "/tmp/rtest/dist/${PKG}/"
tar -czf "/tmp/rtest/dist/${PKG}.tar.gz" -C /tmp/rtest/dist "${PKG}"
(cd /tmp/rtest/dist && sha256sum "${PKG}.tar.gz" > SHA256SUMS)
```

and transferred to the VM for the checks above.

## Release mechanics (checklist)

The 1.0.0 line ships through a prerelease ladder (SemVer; each step sorts
below the next): `1.0.0-beta.1` → `1.0.0-rc.1` (→ `1.0.0-rc.2` if rc.1
surfaces issues) → `1.0.0`. Each step is the *same* procedure below; only
the target version changes. Prereleases publish as GitHub **pre-releases**
(never "Latest"); the final `1.0.0` supersedes them and becomes Latest.

1. **Version bump**: `version = "<target>"` in `Cargo.toml` —
   `1.0.0-beta.1` for the first public build, `1.0.0` for the final —
   commit (gate green). `Cargo.lock` updates in the same commit (the root
   package entry; required or CI's `--locked` build fails).
2. **Tag**: `git tag v<target>`, `git push origin main --tags`.
3. **`release.yml`** runs on Debian 13 (x86_64), verifies the tag matches
   `Cargo.toml` — full `X.Y.Z` **or** `X.Y.Z-prerelease` (fails loudly
   otherwise) — and publishes for a prerelease: mark it `--prerelease` so
   it is not offered as "Latest":
   - `simpletaskmgr-1.0.0-x86_64-linux.tar.gz` (binary + README + LICENSE)
   - `SHA256SUMS`
   - automatic source tarball/zip
4. **`rust.yml`** keeps gating every push/PR (format, clippy, tests,
   build); it must stay green while the release is open.
5. **Post-release sanity**: the VM check in section 5, then close this
   checklist / mark the tag as shipped.

## Out of scope for the GUI (by spec)

- The `Signal` enum (`src/signal.rs`) carries `Sighup`/`Sigterm`/`Sigkill`;
  the **GUI exposes Terminate (SIGTERM) and Kill (SIGKILL)** only — SIGHUP
  remains reachable through the library/`State::kill` path and is covered by
  the delivery tests, but has no button. SIGCONT/SIGSTOP/SIGUSR1/SIGUSR2 are
  not in the enum at all. See `doc/PROCESS_LIST_VIEW_SPEC.md` (D5).

