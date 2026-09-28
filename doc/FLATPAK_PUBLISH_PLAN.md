# Flatpak Publishing Plan

Status: proposal. Companion to `TEST_AND_RELEASE_PLAN.md` — this plan adds a
**third channel** (GNOME-native, flathub) alongside the existing tag-driven
ones (GitHub tarball + crates.io), it does not replace them.

## Why a flatpak channel

The end-user install problem we hit on this machine (2026-09-28): the desktop
entry is invisible to GNOME Shell's app registry whenever `Exec=` cannot be
resolved on the *shell's* PATH, so the dash tile and even the App Grid entry
are missing for locally installed builds (root-cause probes and the registry
behaviour are recorded in the icon work — see the README's taskbar-icon
section and the shell PATH findings). Flatpak bypasses that whole class of
problem:

- The shell attributes sandboxed windows by app id (the first matching branch
  in `shell-window-tracker`), and matches them against flatpak's exported
  desktop entries — no PATH/`Exec` resolution at all.
- Flatpak exports `.desktop` + `share/icons` to
  `~/.local/share/flatpak/exports/share`, which is already in the shell's
  `XDG_DATA_DIRS` here — so the themed icon (`simpletaskmgr`, SVG + 48/64/128/
  256 PNG renders we already ship) is found with zero per-machine install.
- One install command for all users (`flatpak install flathub …`), a built-in
  update channel, and standard discovery via GNOME Software.
- Config/state land under `~/.var/app/org.simpletaskmgr.simpletaskmgr/` (the
  `dirs` crate follows the XDG overrides flatpak sets — no code change).

Everything identity-related we already have is directly reusable: the app id
`org.simpletaskmgr.simpletaskmgr`, the desktop entry, the icon name, and the
tests that pin their consistency.

## Gate 0 — functional feasibility (mandatory, first)

**The core risk:** a process *watcher* needs the host-wide process list.
Flatpak sandboxes run in a **private PID namespace** (local `bwrap(1)`: bwrap
"runs a minimal pid 1 process in the sandbox" when a PID namespace is in
use; `flatpak-run(1)` documents per-sandbox PID namespaces via
`--parent-share-pids`/`--parent-expose-pids`). Practical consequence if so:
`/proc` inside the sandbox enumerates only the sandbox's own processes
(runtime launcher + our app), even though `/proc/stat` and `/proc/meminfo`
still report system-wide counters.

Impacted features (this app's core):

| Feature | Under private PID ns? |
|---|---|
| CPU/Mem/Disk graphs (`/proc/stat`, `/proc/meminfo`, `/sys` freq) | ✅ expected to work |
| Process list (per-pid walk) | ❌ only ~2 sandbox processes |
| "All processes" toggle | ❌ same |
| Detail pane (stat/status/io per pid) | ❌ same |
| Terminate/Kill (`kill(2)` on host pids) | ❌ ESRCH — host pids not visible in the sandbox |
| Dash icon / App Grid entry | ✅ (that's the point of this plan) |

**Test** (on this machine; flatpak 1.16.6 + flathub remote already present;
installs runtimes only, no system packages — reversible with
`flatpak uninstall`):

```bash
flatpak install flathub org.gtk.Gtk-Shell-v4
flatpak run org.gtk.Gtk-Shell-v4 sh -c '
  echo "PIDs visible:"; ls /proc | grep -E "^[0-9]+$" | sort -n | tr "\n" " "; echo
  echo "proc/stat present:"; head -1 /proc/stat
  echo "meminfo present:"; grep -m1 MemTotal /proc/meminfo'
```

- **PASS (only if host PIDs are listed):** continue to Stage 1.
- **FAIL (expected):** the flatpak build is only a "system stats" app, not a
  task manager → take the **Fallback** branch below. Record the outcome in
  this doc so the question is never re-argued.

Secondary checks while the runtime is installed (only meaningful on PASS):
cold start, `flatpak run …` single-instance behaviour (GApplication id
ownership across the flatpak bus proxy), and — before any work beyond the
manifest — verifying Flathub's app-id policy accepts
`org.simpletaskmgr.simpletaskmgr` (the `org.<org>.<app>` shape with a domain
part that may not be registrable; if Flathub rejects it, that is a hard stop
for this channel and costs nothing to find out via their app-id docs/UI *before*
freezing the manifest).

## Stage 1 — manifest + build assets (only if Gate 0 passes)

One new file, Flathub layout in-repo: `flatpak/org.simpletaskmgr.simpletaskmgr.yaml`.
Draft shape (exact runtime versions picked at implementation):

```yaml
app-id: org.simpletaskmgr.simpletaskmgr
runtime: org.gtk.Gtk-Shell-v4
runtime-version: stable
sdk: org.freedesktop.Platform
sdk-extensions:
  - org.freedesktop.Sdk.Extension.rust-stable
buildsystem: cargo
separator: /
modules:
  - name: simpletaskmgr
    builddir: false
    sources:
      - type: git
        url: https://github.com/paulez/simpletaskmgr
        tag: v1.0.0            # Flathub rebuilds from each new v* tag
    build-options:
      cxxflags: -I/app/include
      env:
        RUSTUP_HOME: /usr
        CARGO_HOME: /usr
        PATH: /usr/lib/sdk/rust-stable/bin:{{ PATH }}
    finish-args: []            # no network, no host files — pure /proc reader
    build-commands:
      - cargo build --release --locked
      - install -Dm755 target/release/simpletaskmgr -t /app/bin
      # Desktop entry + icon (flatpak rewrites Exec on export; the shell
      # then matches by app id and resolves Icon= from the exported theme):
      - install -Dm644 share/org.simpletaskmgr.simpletaskmgr.desktop -t /app/share/applications
      - install -Dm644 icons/simpletaskmgr.svg /app/share/icons/hicolor/scalable/apps/simpletaskmgr.svg
      # one line per size (repo files are `simpletaskmgr-<px>.png`):
      - install -Dm644 icons/simpletaskmgr-48.png /app/share/icons/hicolor/48x48/apps/simpletaskmgr.png
      - install -Dm644 icons/simpletaskmgr-64.png /app/share/icons/hicolor/64x64/apps/simpletaskmgr.png
      - install -Dm644 icons/simpletaskmgr-128.png /app/share/icons/hicolor/128x128/apps/simpletaskmgr.png
      - install -Dm644 icons/simpletaskmgr-256.png /app/share/icons/hicolor/256x256/apps/simpletaskmgr.png
      - install -Dm644 icons/simpletaskmgr-512.png /app/share/icons/hicolor/512x512/apps/simpletaskmgr.png   # to be added (rendered from the SVG)
    # appstream block: name/summary/description from Cargo.toml,
    # categories System;Monitor;, license "MIT OR Apache-2.0" (flag for
    # flathub lint), supports: wayland+x11
```

Decisions to make at implementation:

- **Icon size set:** Flathub prefers scalable SVG (we have it) + 512 class
  PNG for the app grid; the repo currently tops out at 256 → add a
  `icons/simpletaskmgr-512.png` render (same SVG, same pipeline as the
  existing 48/64/128/256).
- **License field:** Cargo.toml is dual-licensed; state it honestly in
  appstream and confirm flathub lint accepts the expression.
- **No code changes are expected** (no portal needs, no network, `dirs`-based
  config works under flatpak's XDG overrides) — if any appear, that is a
  design smell and a stop-the-line review point.

## Stage 2 — local build & verification protocol

```bash
flatpak install flathub org.gtk.Gtk-Shell-v4 org.freedesktop.Platform \
  org.freedesktop.Sdk.Extension.rust-stable
flatpak-builder --user --install builddir flatpak/org.simpletaskmgr.simpletaskmgr.yaml
flatpak run --branch=stable org.simpletaskmgr.simpletaskmgr   # in the live session
```

Checklist (run once, with the app from the *flatpak* channel only):

1. Process list is complete and matches `ps aux` (Gate 0's PASS condition
   holds in the real build).
2. CPU/Mem/Disk graphs track live usage; GPU tab appears (this box has a GPU).
3. Select a heavy process → detail pane correct → Terminate the *sandbox's
   own* harmless child to prove `kill` works in-ns; Terminate a host pid to
   confirm the expected ESRCH is reported cleanly (dialog, not crash).
4. Relaunch → single instance activates (no second window).
5. **Dash icon visible** with the live running app; App Grid shows the entry
   with icon; both survive logout/login.
6. Settings (refresh interval) persist under `~/.var/app/org.simpletaskmgr.simpletaskmgr/`.

Note: flatpak apps need a real session bus, so the headless `cage` harness
(`tools/test_ui_render.sh`) does not apply here — logic stays covered by the
`cargo test` suite, visuals by one live-session check per candidate release.

## Fallback (expected branch) — flatpak unsuitable; native channels carry it

If Gate 0 fails (or the app-id policy blocks), close the distribution story
with what we already have plus a `.deb` — best fit for this user's OS
(Debian 13) and it resolves the dash-icon issue structurally:

1. **`.deb` artifact in the existing release workflow** (the workflow already
   builds on a `debian:trixie` container — packaging is a few extra steps
   there): binary → `/usr/bin/simpletaskmgr`, entry →
   `/usr/share/applications/org.simpletaskmgr.simpletaskmgr.desktop`, icons →
   `/usr/share/icons/hicolor/{sizes}/apps/simpletaskmgr.png` + scalable SVG.
   `/usr/bin` is always on the shell's PATH here (`/usr/local/sbin:
   /usr/local/bin:/usr/sbin:/usr/bin`), so the registry accepts the entry,
   the App Grid and dash icon appear, and `apt install ./…` + `apt update`
   give a real native update channel.
2. **Document the flatpak finding** (PID-namespace limitation) in the README
   so end-users and future contributors see an informed "no flatpak".
3. Keep `cargo install simpletaskmgr` as the power-user path; if its icon
   integration matters there, add a small `simpletaskmgr install --desktop`
   subcommand (writes the entry + hicolor files, and *warns* when
   `command -v simpletaskmgr` is not on the shell's PATH — the exact
   diagnostic we needed on this box).

### Commit sequence (one change per commit, each through the gate)

- `.deb` packaging steps in `release.yml` + build/test of the `.deb` in
  CI (container step), artifact + smoke install check.
- README: install section for `apt`/`.deb`, and the "Why no flatpak" note
  with a pointer to this doc.
- (Optional, only if the cargo-install path is kept first-class) the
  `install --desktop` subcommand with tests.

## Open decisions (user calls)

1. Run Gate 0 now? (Downloads ~230 MB of flatpak runtimes; reversible.)
2. If Gate 0 fails: is a `.deb` in scope for the `1.0.0` release train, or is
   the crate/tarball story sufficient for now?
3. App-id policy check on Flathub (only matters on the PASS branch).

## Risk register

| Risk | Branch | Severity | Mitigation |
|---|---|---|---|
| Private PID namespace hides host processes | PASS/FAIL | **Fatal to core** | Gate 0 decides before any other work |
| Flathub rejects the app-id shape | PASS | High | Verify policy before freezing the manifest; renames are the most expensive part of the whole plan |
| Runtime/sdk version drift (`Gtk-Shell-v4`) | PASS | Low | Flathub rebuilds per tag; pin to `stable` and smoke-test on the release train |
| Crates fetch at Flathub build time | PASS | Low | `--locked` + existing `Cargo.lock`; Flathub proxies crates.io |
| AppStream/lint strictness (license, screenshots) | PASS | Low | Lint in a dry-run manifest PR early |
| Cold start latency of flatpak launch | PASS | Cosmetic | One-time; acceptable |
