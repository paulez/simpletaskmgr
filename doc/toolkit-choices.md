# Toolkit Choice — Decision Record

## Decision: **switch to GTK 4 (`gtk4-rs`)**

**Status: DECIDED.** We move the GUI from floem (0.2.0) to GTK 4.

### Main reasons (stated by the owner, in the order given)
1. **Testability** — GTK widgets can be constructed and driven **headlessly** in a
   `#[test]` (no display server, no running event loop required for property/signal
   assertions). This directly fixes the current gap: today we only unit-test the data
   layer (`metrics.rs` parsers, sort/filter), not the views. GTK lets us assert on real
   widget state (labels, list-model items, button signals, `visible()`, etc.).
2. **Simplicity of implementing features** — GTK's model is retained widgets + **explicit
   `connect_*` callback wiring**. There is no reactive effect graph (no
   `create_effect`/`exec_after`/`update_value` closure plumbing to navigate). Data flow is
   linear and readable: a `glib::timeout_add` tick reads `/proc`, mutates the model, calls
   widget `set_*`. This removes the #1 friction of working in floem.
3. **License compatibility** — we plan a **BSD-like (permissive) license** for the project.
   GTK's Rust bindings (`gtk4`, `glib`) are **MIT**; the underlying C stack (glib, cairo,
   pango, GTK) is **LGPL-3.0**, which is fully compatible with permissive source **as long
   as those libraries stay dynamically linked** (our case). This is in contrast to Slint,
   whose usable grant is **GPL-3.0-only (copyleft) or paid**, which is incompatible with a
   BSD-like project.

### Distribution model (resolved)
We ship **only the program binary**. We **expect the target system to provide the GTK
runtime** (the `libgtk-4` / `glib` / `cairo` / `pango` shared libraries + a display server
and fonts). Concretely:

- **We do NOT bundle a static GTK.** GTK's C libraries are shared-only on glibc Linux
  (there is no `libgtk-4.a`), so "statically built, zero dynamic deps" is **not** the goal
  here. The "standalone / lightweight" driver (previously #3) is intentionally relaxed in
  exchange for testability, feature-simplicity, and license compatibility.
- The artifact is a single `simpletaskmgr` ELF. Consumers must have a normal Linux desktop
  with the GTK 4 runtime installed (present on virtually every mainstream distribution;
  e.g. `libgtk-4-0`, `libglib2.0-0`, `libcairo2`, `libpango-1.0-0`, `libgdk-pixbuf-2.0-0`).
- Because we keep GTK **dynamically** linked, LGPL-3.0 §6 (the relinking / source-of-those-
  libs obligation) is satisfied by the fact that the C libs are not statically linked into
  our artifact. No action required beyond shipping the binary and, for transparency, the
  list of shared-library dependencies.
- This is the standard model for GTK apps on Linux (same as `gnome-*`, `gedit`, etc.).

---

## Driver priorities that informed the choice

1. **UI testability**
2. **Complexity to implement features** (lower is better)
3. **Standalone / lightweight binary** — *relaxed, see Distribution model above*
4. **Look & feel**

GTK scores strongly on the two highest-priority drivers (#1 and, functionally, the pain
behind #2) and is license-safe under a permissive project; it "regresses" only the
intentionally-relaxed #3. On the owner's stated weights this is the best trade.

---

## Options considered (supporting analysis)

Legend: ○ strong · ◐ moderate/fair · ✕ weak · ✕✕ fails
Licenses verified via the crates.io API.

| Driver (priority) | floem (baseline) | egui (`eframe`) | **GTK (`gtk4-rs`)** | Slint |
|---|---|---|---|---|
| **1. UI testability** | ✕ data layer only; `update_value` closure / effects not assertable outside a running loop | ◐ test the *model* you decoupled, not the widgets; no standard widget-test hooks | **○ real widgets constructible & drivable headless** | ○ first-class headless `i-slint-backend-testing` |
| **2. Implement complexity** | ✕✕ reactive effect graph, hard to navigate | ○ simplest (immediate mode) | **○ explicit `connect_*` callbacks, linear; no effect graph** | ◐ declarative `.slint` + one-way bindings; new DSL |
| **3. Standalone / lightweight** | ○ cleanest `ldd` | ○ small link table | ✕ big hard-linked C + host theme/icon assets | ◐ winit dlopen + one C lib (fontconfig) |
| **4. Look & feel** | ◐ baseline | ◐ clean | ○ best (native theme, real a11y, dnd) | ◐ themeable, not native |
| **License fit (BSD-like project)** | ○ `floem` = MIT | ○ `egui`/`eframe` = MIT/Apache | **○ bindings = MIT; C stack = LGPL-3.0 (OK, dynamic)** | ✕ **GPL-3.0-only / paid** (copyleft — incompatible) |

### Why not the others (given these weights + license)
- **Slint** — strongest headless-testing story *and* lightest C footprint, but its usable
  license grant is **GPL-3.0-only or paid**, which conflicts with a BSD-like project. Ruled
  out on license grounds.
- **egui/eframe** — clean permissive (MIT/Apache) and the simplest to implement on, and
  light on #3, but the **weakest on the top driver (widget testability)** — its testing is
  "decouple the model and test that," not "drive real widgets." Loses on #1.
- **floem (current)** — best #3 as-is but worst #1 and the source of the #2 pain. Staying
  only makes sense if #3 dominates the other three, which it no longer does.

---

## GTK invariants (still true post-migration)
- GTK 4 is **single-threaded / not thread-safe**; no widget struct is `Send`/`Sync`.
- All widget construction & mutation happens on the main thread; the refresh timer
  (`glib::timeout_add`) fires on the main loop, and any future async work must hop back
  via `glib::MainContext::default().spawn_local` or `glib::idle_add`.
- The data layer stays framework-free and thread-agnostic (`metrics` parsers,
  `cpu_tracker`, `process` core), so it is unit-testable with no GTK runtime.
