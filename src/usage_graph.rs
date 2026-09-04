use cairo::Context;

use crate::metrics::Sample;

/// Normalizes an `(r, g, b)` value in `0–255` to `0–1` for cairo drawing.
fn rgb_to_f64(rgb: (u8, u8, u8)) -> (f64, f64, f64) {
    (
        rgb.0 as f64 / 255.0,
        rgb.1 as f64 / 255.0,
        rgb.2 as f64 / 255.0,
    )
}

/// RGB triplet (0–255) for the CPU series — green, matching the old floem
/// `CPU_COLOR = rgb8(76, 175, 80)` / SVG `#4CAF50`.
pub const CPU_RGB: (u8, u8, u8) = (76, 175, 80);
/// RGB triplet (0–255) for the memory series — blue, matching the old floem
/// `MEM_COLOR = rgb8(33, 150, 243)` / SVG `#2196F3`.
pub const MEM_RGB: (u8, u8, u8) = (33, 150, 243);
/// RGB triplet (0–255) for the CPU-frequency series — purple, distinct from
/// the green CPU and blue memory swatches and the red temperature swatch.
pub const FREQ_RGB: (u8, u8, u8) = (171, 71, 189);
/// RGB triplet (0–255) for the CPU-temperature series — red.
pub const TEMP_RGB: (u8, u8, u8) = (229, 57, 53);
/// Translucent fill alpha so overlapping series areas read as distinct bands.
pub const FILL_ALPHA: f64 = 0.22;
/// Line width of each series (in pixels at the draw-time scale). Thicker than
/// a hairline so a low-but-real trace (a few percent) reads as a curve rather
/// than a hair along the baseline.
pub const LINE_WIDTH: f64 = 2.5;
/// Radius of the "latest sample" marker dot (in pixels).
pub const DOT_RADIUS: f64 = 2.5;
/// Alpha of the horizontal gridlines and baseline drawn behind the series.
const GRID_ALPHA: f64 = 0.28;
/// Neutral grey for gridlines (0–1); the baseline sits a touch darker.
const GRID_RGB: (f64, f64, f64) = (0.55, 0.55, 0.55);
const BASELINE_RGB: (f64, f64, f64) = (0.42, 0.42, 0.42);

/// Vertical padding above the plot, reserved for the topmost tick label so
/// the max-value label is never clipped by the widget edge. (The pane title
/// is rendered by a GTK label above the drawing area in `ui.rs`, so it does
/// not consume any cairo pixel budget here.)
pub const TOP_PAD: f64 = 14.0;
/// Whitespace between a label gutter and the plot boundary, so tick labels
/// don't hug the axis line.
pub const GUTTER_PAD: f64 = 4.0;
/// Baseline offset from a tick row, so the top tick (y = plot top) stays
/// inside the widget and the bottom tick (y = plot bottom) is drawn just
/// above it.
const TICK_BASELINE: f64 = 3.0;

/// Splits `values` into the index ranges of its maximal contiguous stretches
/// of `Some`. `values[i].is_some()` is the only property consulted.
///
/// The painter draws each returned run as a single connected path and leaves
/// the gaps (where the value is `None`) blank. A single sample (the common
/// case) yields one run `0..1`; an all-`None` slice (no sensor) yields no
/// runs; an empty slice yields no runs.
fn iter_some_runs(values: &[Option<f64>]) -> Vec<std::ops::Range<usize>> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for (i, v) in values.iter().enumerate() {
        let active = start.is_some();
        if v.is_some() {
            if !active {
                start = Some(i);
            }
        } else if active {
            runs.push(start.take().unwrap()..i);
        }
    }
    if let Some(s) = start {
        runs.push(s..values.len());
    }
    runs
}

/// Maps the i-th of `n` samples (index 0 = oldest) to a pixel x offset within
/// a plot of width `w`.
///
/// Two phases, continuous at the boundary so there is no visible snap:
///
/// * **Warm-up** (`n <= fill`): *left-anchored*. The oldest sample sits on the
///   left edge and each newer sample steps one slot to the right, the newest
///   still reaching the right edge only once `n` hits `fill`. At the default
///   cadence this is the "fill the pane in ~10 s" phase: the trace clearly
///   grows left→right across the first several samples instead of instantly
///   stretching a few points across the full width.
/// * **Settle + scroll** (`n > fill`): *right-anchored*. The newest sample is
///   pinned to the right edge while the slot spacing **eases linearly** from
///   the wide warm-up width `w / (fill - 1)` down to the steady width
///   `w / (capacity - 1)`. As the window stretches from `fill` to `capacity`
///   samples it therefore covers a growing span of history (the "slowly change
///   the scale" phase); once `n` reaches `capacity` the spacing is fixed and
///   the oldest samples slide off the left edge (scroll).
///
/// `fill` is the number of samples that span the full width at the end of the
/// warm-up (≈ 10 s at the default 1.5 s refresh); `capacity` is the steady
/// rolling-window width in samples (≈ 3 min at that cadence — the `MAX_HISTORY`
/// cap the deque enforces). The newest sample is always at `w`; each older
/// sample sits one slot to its left and reports a **negative** x once its slot
/// offset exceeds `w` (it has scrolled off the left edge). The painter clips
/// those off-edge samples to the plot rectangle — it must **not** clamp them
/// onto the left edge, because that would stack the entire stale history into
/// a single vertical line at `x = 0`.
pub fn sample_x(i: usize, n: usize, fill: usize, capacity: usize, w: f64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let i = i.min(n - 1);
    let fill = fill.max(2);
    // The steady window is never narrower than the warm-up window.
    let capacity = capacity.max(fill).max(2);
    let warm_slot = w / (fill - 1) as f64;
    let steady_slot = w / (capacity - 1) as f64;
    if n <= fill {
        // Warm-up: left-anchored; the newest sample lands on the right edge
        // exactly when `fill` samples have arrived (the pane is "full").
        return (i as f64 * warm_slot).clamp(0.0, w.max(0.0));
    }
    // Settle + scroll: right-anchored; the slot eases warm -> steady as the
    // window stretches from `fill` to `capacity` samples, then holds steady.
    let ramp = (capacity - fill).max(1);
    let t = ((n - fill) as f64 / ramp as f64).clamp(0.0, 1.0);
    let slot = warm_slot + t * (steady_slot - warm_slot);
    let steps_back = (n - 1 - i) as f64;
    // No low clamp: once a sample's slot offset exceeds `w` it has scrolled off
    // the left edge and reports a negative x. The painter (`draw_series`) clips
    // the series to the plot rectangle and so discards these; clamping them to
    // `0.0` here would instead stack the whole stale history into a vertical
    // line at the left edge.
    w - steps_back * slot
}

/// Maps a value in `0..=domain_max` to a normalized y position (0..=1) where
/// `domain_max` is the top (y=0) and `0` is the bottom (y=1). Values are
/// clamped so they never spill off the chart. A non-positive `domain_max`
/// maps everything to the bottom (y=1).
///
/// This generalizes the old percent-only mapping: memory and CPU pass
/// `domain_max = 100.0`, temperature passes `100.0`, and frequency passes its
/// own ceiling (`scaling_max_freq` in MHz) so a 3.4 GHz trace sits near the
/// top rather than 0.85% of the height.
pub fn frac_of(value: f64, domain_max: f64) -> f64 {
    if domain_max <= 0.0 {
        return 1.0;
    }
    1.0 - (value.clamp(0.0, domain_max) / domain_max)
}

/// Maps a 0–100 percent value to a normalized y position (0..=1). Kept as a
/// thin wrapper over [`frac_of`] (the chart's percent series use it), and it
/// remains the canonical unit-mapping test target.
pub fn sample_y_frac(value: f64) -> f64 {
    frac_of(value, 100.0)
}

/// The inner drawable rectangle of a pane. Everything outside (title band,
/// top padding band, label gutters) is reserved chrome so the series and the
/// axis labels never overlap.
///
/// * `ox, oy` — origin of the plot (lower-left corner, in Cairo y-down
///   convention `oy` is the plot *top*).
/// * `w, h` — plot size. Clamped to 0 when chrome exceeds the widget.
struct Plot {
    /// x origin of the plot — right after the left gutter.
    ox: f64,
    /// y origin of the plot — below the title and top-padding bands.
    oy: f64,
    /// Plot width (0 when the widget is too narrow to fit both gutters).
    w: f64,
    /// Plot height (0 when the title+padding bands exceed the widget
    /// height).
    h: f64,
}

impl Plot {
    /// Invariant: plot dimensions are non-negative. In the degenerate case
    /// (chrome exceeds the widget) dimensions clamp to 0 rather than going
    /// negative, so a caller that draws the plot at non-degenerate sizes still
    /// stays inside its widget.
    fn non_negative(&self) -> bool {
        self.ox >= 0.0 && self.oy >= 0.0 && self.w >= 0.0 && self.h >= 0.0
    }
}

/// Computes the inner plot rectangle for a widget of size `(w, h)` with
/// `left` and `right` gutter widths (pixels). The plot origin is pushed down
/// by `TOP_PAD` (space for the top tick label) and `left`/`right` (label
/// gutters). Dimensions clamp to 0 when chrome exceeds the widget.
fn plot_rect(w: f64, h: f64, left: f64, right: f64) -> Plot {
    let p = Plot {
        ox: left.max(0.0),
        oy: TOP_PAD.max(0.0),
        w: (w - left - right).max(0.0),
        h: (h - TOP_PAD).max(0.0),
    };
    debug_assert!(p.non_negative(), "plot dimensions must be non-negative");
    p
}

/// Width of the gutter needed to hold the widest of `labels`, measured at
/// the current font settings: `widest_label + 2*GUTTER_PAD`. An empty slice
/// or an all-empty measurement yields a zero-width gutter.
fn measure_gutter(ctx: &Context, labels: &[String]) -> f64 {
    if labels.is_empty() {
        return 0.0;
    }
    let width = labels
        .iter()
        .map(|t| ctx.text_extents(t).map(|e| e.width()).unwrap_or(0.0))
        .fold(0.0f64, f64::max);
    width + 2.0 * GUTTER_PAD
}

/// Faint horizontal gridlines at 25/50/75% of the plot height plus a slightly
/// darker baseline (the 0 line), all confined to the plot rectangle. Drawn
/// *before* the series so it reads as a backdrop, and only where the plot has
/// real area — a degenerate (zero-size) plot draws nothing.
fn draw_gridlines(ctx: &Context, plot: &Plot) {
    if plot.w <= 0.0 || plot.h <= 0.0 {
        return;
    }
    for &frac in &[0.25_f64, 0.5, 0.75] {
        let y = plot.oy + frac * plot.h;
        ctx.set_source_rgba(GRID_RGB.0, GRID_RGB.1, GRID_RGB.2, GRID_ALPHA);
        ctx.set_line_width(1.0);
        ctx.move_to(plot.ox, y);
        ctx.line_to(plot.ox + plot.w, y);
        let _ = ctx.stroke();
    }
    // Baseline (value 0) a touch darker than the interior gridlines.
    let y = plot.oy + plot.h;
    ctx.set_source_rgba(BASELINE_RGB.0, BASELINE_RGB.1, BASELINE_RGB.2, GRID_ALPHA);
    ctx.set_line_width(1.0);
    ctx.move_to(plot.ox, y);
    ctx.line_to(plot.ox + plot.w, y);
    let _ = ctx.stroke();
}

/// Draws one series (area fill + line + latest-sample dot) inside the inner
/// `plot` rectangle, laid out across `plot.w` on the right-anchored
/// `capacity`-slot window.
///
/// `values[i] = None` leaves a blank gap at that x (the run before and after
/// it is drawn as its own connected path). `domain_max` is the value that
/// maps to the top of the plot — each series passes its own domain
/// (`100.0` for the percent series, the freq ceiling for the frequency
/// series), so a series on a different scale still reads as a normal curve.
fn draw_series(
    ctx: &Context,
    plot: &Plot,
    values: &[Option<f64>],
    fill: usize,
    capacity: usize,
    domain_max: f64,
    rgb: (u8, u8, u8),
) {
    if plot.w <= 0.0 || plot.h <= 0.0 {
        return;
    }
    let runs = iter_some_runs(values);
    let n = values.len();
    let (r, g, b) = rgb_to_f64(rgb);
    let x_at = |i: usize| plot.ox + sample_x(i, n, fill, capacity, plot.w);
    let y_at = |i: usize| plot.oy + frac_of(values[i].unwrap_or(0.0), domain_max) * plot.h;
    let bottom = plot.oy + plot.h;
    for run in &runs {
        let (a, z) = (run.start, run.end);
        // Clip the series body to the plot rectangle. `sample_x` reports a
        // negative x for samples that have scrolled off the left edge during
        // the settle/scroll ramp, so without a clip those segments would paint
        // into the left gutter (and, before the fix, clamp into a vertical line
        // at the left edge). The clip discards them so the trace fades out as
        // it reaches the left edge.
        let _ = ctx.save();
        ctx.new_path();
        ctx.rectangle(plot.ox, plot.oy, plot.w, plot.h);
        ctx.clip();
        // 1) Area fill (low alpha) from the plot bottom up to the run's
        //    curve.
        ctx.new_path();
        ctx.move_to(x_at(a), bottom);
        for i in a..z {
            ctx.line_to(x_at(i), y_at(i));
        }
        ctx.line_to(x_at(z - 1), bottom);
        ctx.close_path();
        ctx.set_source_rgba(r, g, b, FILL_ALPHA);
        let _ = ctx.fill();

        // 2) Series outline for the run.
        ctx.new_path();
        ctx.move_to(x_at(a), y_at(a));
        for i in (a + 1)..z {
            ctx.line_to(x_at(i), y_at(i));
        }
        ctx.set_source_rgb(r, g, b);
        ctx.set_line_width(LINE_WIDTH);
        let _ = ctx.stroke();
        let _ = ctx.restore();
    }
    // 3) "Latest sample" marker on the newest run (the run ending at n-1),
    //    so the "now" value stays visible even before a second sample
    //    arrives.
    if let Some(last) = runs.last() {
        if last.end == n {
            ctx.new_path();
            ctx.arc(
                x_at(n - 1),
                y_at(n - 1),
                DOT_RADIUS,
                0.0,
                std::f64::consts::TAU,
            );
            ctx.set_source_rgb(r, g, b);
            let _ = ctx.fill();
        }
    }
}

/// Which half of the split usage graph a `DrawingArea` should paint.
///
/// The graph is rendered in two side-by-side panes so each pair of series
/// gets enough horizontal room to read: [`ChartPane::CpuMem`] draws the two
/// percent series (memory + CPU) on a shared 0–100% grid, and
/// [`ChartPane::FreqTemp`] draws the two hardware-sensor series (frequency,
/// temperature) on their own dedicated axes. Both panes read the same
/// rolling history and are drawn at the same cadence; only the series each
/// paints differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartPane {
    /// The CPU + memory utilization pane (percent series).
    CpuMem,
    /// The CPU-frequency + temperature pane (MHz / °C series).
    FreqTemp,
}

/// Configuration the chart needs to lay out its series, decoupled from the
/// GTK widgets that own the samples. `freq_max_mhz` is the top of the
/// frequency axis (MHz), `mem_max_mb` the top of the memory axis (MB — the
/// installed RAM, like `freq_max_mhz` is `scaling_max_freq`), `fill` is the
/// number of samples that span the full width once the warm-up finishes, and
/// `capacity` is the steady rolling-window width in samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartConfig {
    /// Top of the frequency-axis domain in MHz.
    pub freq_max_mhz: f64,
    /// Top of the memory-axis domain in MB (installed RAM).
    pub mem_max_mb: f64,
    /// Number of samples that fill the plot width at the end of the warm-up
    /// (≈ 10 s at the default 1.5 s refresh).
    pub fill: usize,
    /// Number of samples a full steady rolling window holds.
    pub capacity: usize,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            freq_max_mhz: 4000.0,
            // A typical desktop; overridden by the live `MemTotal` at runtime.
            mem_max_mb: 16384.0,
            // 7 samples ≈ 10 s at the default 1.5 s refresh — the time the
            // trace takes to grow from the left edge to fill the pane width.
            fill: 7,
            // Keep in sync with `SystemMetrics::MAX_HISTORY`.
            capacity: 120,
        }
    }
}

/// Formats a frequency-axis tick in MHz as a compact label: `"4 GHz"`,
/// `"1.5 GHz"`, `"550 MHz"`.
pub fn axis_tick_mhz(mhz: f64) -> String {
    if mhz >= 1000.0 {
        let ghz = mhz / 1000.0;
        if (ghz * 10.0).round() == ghz * 10.0 && ghz.fract() == 0.0 {
            format!("{:.0} GHz", ghz)
        } else {
            format!("{:.1} GHz", ghz)
        }
    } else {
        format!("{:.0} MHz", mhz)
    }
}

/// Formats a temperature-axis tick in °C as a compact label: `"100 °C"`.
pub fn axis_tick_celsius(celsius: f64) -> String {
    format!("{:.0} °C", celsius)
}

/// Formats a percent-axis tick as a compact label: `"100%"`, `"50%"`.
pub fn axis_tick_percent(p: f64) -> String {
    format!("{:.0}%", p)
}

/// Compact "Gigabyte" label for a value in MB, with one decimal ("14.2 GB"),
/// falling back to whole megabytes ("512 MB") when the amount is under a
/// gigabyte. Used by the pane-header readout where a decimal-GB value reads
/// better than the integer-GB axis tick.
fn gig_label(mb: f64) -> String {
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.0} MB", mb)
    }
}

/// The live readout for the CPU & Memory pane header: `Cpu 4.5% · Mem 14.2 GB`.
/// `cpu`/`mem` are percents (0–100) as stored in [`Sample`]; `mem_max_mb`
/// scales the percent into an absolute amount matching the pane's right axis.
/// The `Cpu`/`Mem` labels make a bare value unambiguous (it never reads as a
/// stray `100` or `8`), mirroring the `Freq`/`Temp` labels on the other pane.
pub fn cpu_mem_readout(cpu: f64, mem: f64, mem_max_mb: f64) -> String {
    format!(
        "Cpu {cpu:.1}% \u{b7} Mem {}",
        gig_label(mem / 100.0 * mem_max_mb)
    )
}

/// The live readout for the CPU Freq & Temp pane header:
/// `Freq 3.7 GHz · Temp 56 °C`. A `None` sensor (no frequency or temperature
/// source) reads as `"-"` so a missing reading never reads as `0`.
pub fn freq_temp_readout(freq_mhz: Option<f64>, temp_c: Option<f64>) -> String {
    let freq = freq_mhz.map(axis_tick_mhz).unwrap_or_else(|| "-".into());
    let temp = temp_c.map(axis_tick_celsius).unwrap_or_else(|| "-".into());
    format!("Freq {freq} \u{b7} Temp {temp}")
}

/// Formats a memory-axis tick (in MB) as a compact, human label: a clean
/// whole GiB multiple reads as GB (`"16 GB"`, `"2 GB"`), anything else stays
/// in MB (`"1536 MB"`, `"32 MB"`, `"0 MB"`). This keeps the top tick of a 16 GB
/// host readable as `16 GB` while a host whose MemTotal isn't a clean GiB
/// boundary still reads as a sensible MB value rather than `15.4 GC`.
pub fn axis_tick_mb(mb: f64) -> String {
    if mb >= 1024.0 && (mb / 1024.0).fract() == 0.0 {
        format!("{:.0} GB", mb / 1024.0)
    } else {
        format!("{:.0} MB", mb)
    }
}

/// The three tick rows for a chart of a given domain — the domain top, the
/// midpoint, and the bottom (0) — returned as `(fraction, label)` pairs where
/// `fraction` is the normalized y offset (0 = top, 1 = bottom) within the
/// plot.
fn axis_ticks(domain_max: f64, format: fn(f64) -> String) -> Vec<(f64, String)> {
    vec![
        (frac_of(domain_max, domain_max), format(domain_max)),
        (
            frac_of(domain_max * 0.5, domain_max),
            format(domain_max * 0.5),
        ),
        (frac_of(0.0, domain_max), format(0.0)),
    ]
}

/// Whether an axis column sits on the left or right edge of the plot.
#[derive(Clone, Copy)]
enum AxisSide {
    Left,
    Right,
}

/// Draws the tick labels for one axis *outside* the plot, in the left or
/// right gutter (as named by `side`). Labels sit `GUTTER_PAD` away from the
/// plot edge so they never overlap the series, and a faint guide is drawn
/// along the plot edge the tick belongs to.
fn draw_axis_ticks(
    ctx: &Context,
    plot: &Plot,
    domain_max: f64,
    rgb: (u8, u8, u8),
    label: fn(f64) -> String,
    side: AxisSide,
) {
    if plot.w <= 0.0 || plot.h <= 0.0 {
        return;
    }
    let (r, g, b) = rgb_to_f64(rgb);
    for (frac, text) in axis_ticks(domain_max, label) {
        let y = plot.oy + frac * plot.h;
        let width = ctx.text_extents(&text).map(|e| e.width()).unwrap_or(0.0);
        let x = match side {
            AxisSide::Left => (plot.ox - GUTTER_PAD - width).max(0.0),
            AxisSide::Right => plot.ox + plot.w + GUTTER_PAD,
        };
        // Top tick sits below its row, bottom tick above, so both stay
        // on-canvas.
        let baseline_y = if frac < 0.5 {
            y + TICK_BASELINE
        } else {
            y - TICK_BASELINE
        };
        ctx.set_source_rgb(r, g, b);
        ctx.move_to(x, baseline_y);
        let _ = ctx.show_text(&text);
    }
    // Faint guide along the plot edge this axis belongs to.
    let gx = match side {
        AxisSide::Left => plot.ox,
        AxisSide::Right => plot.ox + plot.w,
    };
    ctx.set_source_rgba(r, g, b, FILL_ALPHA);
    ctx.set_line_width(1.0);
    ctx.move_to(gx, plot.oy);
    ctx.line_to(gx, plot.oy + plot.h);
    let _ = ctx.stroke();
}

/// Paints one pane of the split rolling-window usage chart into `ctx`,
/// spanning `(w, h)`.
///
/// The pane is laid out top-to-bottom as: a top padding band (`TOP_PAD`),
/// reserved for the topmost tick label so it is never clipped by the widget
/// edge, and finally the plot. Left and right gutters hold the pane's tick
/// labels outside the plot, sized at draw time from `ctx.text_extents` so
/// even the widest label (e.g. `"100 °C"`) never clips.
///
/// The pane title (e.g. `"CPU & Memory"`) is drawn by the GTK layer as a
/// standard centered `Label` *above* this widget, using the theme's font and
/// color — cairo does not paint it here.
///
/// * [`ChartPane::CpuMem`] — a left % axis (green-tinted) for CPU + a right
///   MB axis (blue-tinted) for RAM. CPU is drawn 0–100%; RAM is scaled to the
///   installed RAM (`cfg.mem_max_mb`, like `freq_max_mhz` for the Freq/Temp
///   pane) so a 10 GB / 16 GB trace reads near the top rather than 63% of the
///   way down.
/// * [`ChartPane::FreqTemp`] — a left MHz axis (purple-tinted) + a right °C
///   axis (red-tinted), and frequency (purple, domain `cfg.freq_max_mhz`) +
///   temperature (red, 0–100 °C) series.
///
/// A series whose samples are all `None` (no sensor) is skipped. An empty
/// history paints a blank, valid chart.
pub fn paint_usage_chart(
    ctx: &Context,
    w: f64,
    h: f64,
    samples: &[Sample],
    cfg: &ChartConfig,
    pane: ChartPane,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    ctx.select_font_face(
        "sans-serif",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    // 10pt keeps the tick labels readable at the default 940px window width
    // without crowding the narrow plot — 9pt read as thin/faint on screen.
    ctx.set_font_size(10.0);
    // Both panes are the same dual-axis layout: a left and a right series,
    // each with its own `(values, domain, rgb, tick_fn)`, axes on opposite
    // sides, one drawn in front of the other. Only *which* series is on
    // which side and the z-order differ per pane, so we pick them in the
    // match and share the layout code below.
    type Side = (Vec<Option<f64>>, f64, (u8, u8, u8), fn(f64) -> String);
    let (left, right, back_is_right): (Side, Side, bool) = match pane {
        // CpuMem: left = CPU (front, % axis 0–100); right = MEM (back, MB axis).
        ChartPane::CpuMem => (
            (
                samples.iter().map(|s| Some(s.cpu)).collect(),
                100.0,
                CPU_RGB,
                axis_tick_percent,
            ),
            (
                samples
                    .iter()
                    .map(|s| Some(s.mem / 100.0 * cfg.mem_max_mb))
                    .collect(),
                cfg.mem_max_mb,
                MEM_RGB,
                axis_tick_mb,
            ),
            true,
        ),
        // FreqTemp: left = FREQ (back, MHz axis); right = TEMP (front, °C axis).
        ChartPane::FreqTemp => (
            (
                samples.iter().map(|s| s.freq).collect(),
                cfg.freq_max_mhz,
                FREQ_RGB,
                axis_tick_mhz,
            ),
            (
                samples.iter().map(|s| s.temp).collect(),
                100.0,
                TEMP_RGB,
                axis_tick_celsius,
            ),
            false,
        ),
    };
    let labels = |s: &Side| {
        let tick: fn(f64) -> String = s.3;
        let dom: f64 = s.1;
        [tick(dom), tick(dom / 2.0), tick(0.0)]
    };
    let left_gutter = measure_gutter(ctx, &labels(&left));
    let right_gutter = measure_gutter(ctx, &labels(&right));
    let plot = plot_rect(w, h, left_gutter, right_gutter);
    draw_gridlines(ctx, &plot);
    let (back, front) = if back_is_right {
        (&right, &left)
    } else {
        (&left, &right)
    };
    draw_series(ctx, &plot, &back.0, cfg.fill, cfg.capacity, back.1, back.2);
    draw_series(
        ctx,
        &plot,
        &front.0,
        cfg.fill,
        cfg.capacity,
        front.1,
        front.2,
    );
    draw_axis_ticks(ctx, &plot, left.1, left.2, left.3, AxisSide::Left);
    draw_axis_ticks(ctx, &plot, right.1, right.2, right.3, AxisSide::Right);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn sample(v: f64) -> Sample {
        Sample {
            cpu: v,
            mem: v,
            freq: None,
            temp: None,
        }
    }

    fn samples(values: &[f64]) -> Vec<Sample> {
        values.iter().map(|v| sample(*v)).collect()
    }

    // ---- plot-rect geometry -----------------------------------------

    #[test]
    fn test_plot_rect_basic_geometry() {
        let p = plot_rect(100.0, 80.0, 20.0, 10.0);
        assert_eq!(p.ox, 20.0);
        assert_eq!(p.oy, TOP_PAD);
        assert_eq!(p.w, 100.0 - 20.0 - 10.0);
        assert_eq!(p.h, 80.0 - TOP_PAD);
    }

    #[test]
    fn test_plot_rect_no_gutters_is_full_width() {
        let p = plot_rect(100.0, 80.0, 0.0, 0.0);
        assert_eq!(p.ox, 0.0);
        assert_eq!(p.w, 100.0);
        assert_eq!(p.oy, TOP_PAD);
        assert_eq!(p.h, 80.0 - TOP_PAD);
    }

    #[test]
    fn test_plot_rect_clamps_to_zero_when_gutters_exceed_width() {
        let p = plot_rect(30.0, 100.0, 40.0, 20.0);
        assert_eq!(p.w, 0.0, "plot width clamps to 0");
        assert_eq!(p.ox, 40.0, "left gutter still takes its width");
    }

    #[test]
    fn test_plot_rect_clamps_to_zero_when_top_pad_exceeds_height() {
        let p = plot_rect(100.0, 5.0, 0.0, 0.0);
        assert_eq!(p.h, 0.0, "plot height clamps to 0");
        assert_eq!(p.oy, TOP_PAD, "top padding still takes its height");
    }

    #[test]
    fn test_plot_rect_dimensions_stay_non_negative() {
        // Covers both the normal case (chrome < widget) and the degenerate
        // case where gutters or the top pad exceed the widget size —
        // `plot_rect` must clamp to 0, never go negative.
        for (w, h, l, r) in [
            (100.0, 80.0, 0.0, 0.0),
            (100.0, 80.0, 20.0, 10.0),
            (50.0, 40.0, 10.0, 10.0),
            (60.0, 40.0, 80.0, 80.0), // gutters exceed width
            (0.0, 0.0, 10.0, 10.0),
            (10.0, 5.0, 0.0, 0.0), // widget height under TOP_PAD
        ] {
            let p = plot_rect(w, h, l, r);
            assert!(
                p.non_negative(),
                "plot (ox={ox}, oy={oy}, pw={pw}, ph={ph}) at widget ({w}x{h}) gutters ({l},{r}) must be non-negative",
                ox = p.ox, oy = p.oy, pw = p.w, ph = p.h
            );
        }
    }

    // ---- axis-tick labels ---------------------------------------------

    /// Percent-axis tick labels: whole percentages, no unit separator.
    #[rstest]
    #[case::hundred(100.0, "100%")]
    #[case::fifty(50.0, "50%")]
    #[case::twentyfive(25.0, "25%")]
    #[case::zero(0.0, "0%")]
    fn test_axis_tick_percent(#[case] value: f64, #[case] expected: &str) {
        assert_eq!(axis_tick_percent(value), expected);
    }

    /// MB-axis tick labels switch to GB at whole-gig multiples.
    #[rstest]
    #[case::sixteen_gib(16384.0, "16 GB")]
    #[case::two_gib(2048.0, "2 GB")]
    #[case::four_gib(4096.0, "4 GB")]
    #[case::odd_mb(1536.0, "1536 MB")]
    #[case::sub_gib_mb(512.0, "512 MB")]
    #[case::zero(0.0, "0 MB")]
    fn test_axis_tick_mb(#[case] mb: f64, #[case] expected: &str) {
        assert_eq!(axis_tick_mb(mb), expected);
    }

    /// MHz-axis tick labels render 1000+ as GHz.
    #[rstest]
    #[case::four_ghz(4000.0, "4 GHz")]
    #[case::two_ghz(2000.0, "2 GHz")]
    #[case::one_and_half_ghz(1500.0, "1.5 GHz")]
    #[case::sub_ghz(550.0, "550 MHz")]
    #[case::zero(0.0, "0 MHz")]
    fn test_axis_tick_mhz(#[case] mhz: f64, #[case] expected: &str) {
        assert_eq!(axis_tick_mhz(mhz), expected);
    }

    /// Celsius-axis tick labels.
    #[rstest]
    #[case::top(100.0, "100 °C")]
    #[case::mid(50.0, "50 °C")]
    #[case::zero(0.0, "0 °C")]
    fn test_axis_tick_celsius(#[case] celsius: f64, #[case] expected: &str) {
        assert_eq!(axis_tick_celsius(celsius), expected);
    }

    #[test]
    fn test_axis_ticks_three_rows_evenly_spread() {
        let ticks = axis_ticks(4000.0, axis_tick_mhz);
        assert_eq!(ticks.len(), 3);
        assert_eq!(ticks[0].0, 0.0, "top tick at the ceiling");
        assert_eq!(ticks[1].0, 0.5, "midpoint tick sits half-way down");
        assert_eq!(ticks[2].0, 1.0, "bottom tick");
        assert_eq!(ticks[0].1, "4 GHz");
        assert_eq!(ticks[1].1, "2 GHz");
        assert_eq!(ticks[2].1, "0 MHz");
    }

    #[test]
    fn test_axis_tick_celsius_format() {
        let ticks = axis_ticks(100.0, axis_tick_celsius);
        assert_eq!(ticks[0].1, "100 °C");
        assert_eq!(ticks[1].1, "50 °C");
        assert_eq!(ticks[2].1, "0 °C");
    }

    #[test]
    fn test_axis_tick_mhz_units() {
        assert_eq!(axis_tick_mhz(4000.0), "4 GHz");
        assert_eq!(axis_tick_mhz(2000.0), "2 GHz");
        assert_eq!(axis_tick_mhz(1500.0), "1.5 GHz");
        assert_eq!(axis_tick_mhz(550.0), "550 MHz");
        assert_eq!(axis_tick_mhz(0.0), "0 MHz");
    }

    // ---- pane-header live readouts -------------------------------------

    /// `gig_label` formats a GiB-multiple as GB and everything else as MB.
    #[rstest]
    #[case::one_and_half_gib(1536.0, "1.5 GB")]
    #[case::sub_gib(512.0, "512 MB")]
    #[case::sixtyfour_gib(65536.0, "64.0 GB")]
    fn test_gig_label(#[case] mb: f64, #[case] expected: &str) {
        assert_eq!(gig_label(mb), expected);
    }

    /// Pane-header live readout: "Cpu x% · Mem y" with sub-GiB fallback to MB.
    #[rstest]
    #[case::gig(4.5, 24.0, 65536.0, "Cpu 4.5% \u{b7} Mem 15.4 GB")]
    #[case::mbytes(0.0, 5.0, 100.0, "Cpu 0.0% \u{b7} Mem 5 MB")]
    fn test_cpu_mem_readout(
        #[case] cpu: f64,
        #[case] mem_percent: f64,
        #[case] mem_max_mb: f64,
        #[case] expected: &str,
    ) {
        assert_eq!(cpu_mem_readout(cpu, mem_percent, mem_max_mb), expected);
    }

    /// Freq/Temp readout: a missing sensor renders as "-" rather than "0".
    #[rstest]
    #[case::present(Some(3700.0), Some(56.0), "Freq 3.7 GHz \u{b7} Temp 56 \u{b0}C")]
    #[case::freq_absent(None, Some(56.0), "Freq - \u{b7} Temp 56 \u{b0}C")]
    #[case::temp_absent(Some(550.0), None, "Freq 550 MHz \u{b7} Temp -")]
    fn test_freq_temp_readout(
        #[case] freq: Option<f64>,
        #[case] temp: Option<f64>,
        #[case] expected: &str,
    ) {
        assert_eq!(freq_temp_readout(freq, temp), expected);
    }

    // ---- x/y mappings ---------------------------------------------------

    const FILL: usize = 10;
    const CAP: usize = 120;

    #[test]
    fn test_sample_x_single_sample_sits_at_left_edge() {
        // A lone sample is the first of the warm-up phase: it anchors the
        // trace to the left edge (not the right), so the next samples visibly
        // grow to the right.
        assert_eq!(
            sample_x(0, 1, FILL, CAP, 400.0),
            0.0,
            "a single sample anchors the left edge during warm-up"
        );
    }

    #[test]
    fn test_sample_x_warmup_left_anchored_evenly_spaced() {
        // During the warm-up (n <= fill) the trace is left-anchored: the
        // oldest pinned to the left edge and each sample one (wide) slot to
        // the right, so the newest still short of the right edge.
        let w = 1200.0;
        let warm_slot = w / (FILL - 1) as f64; // 12 = 1200/99
        for n in 1..=FILL {
            for i in 0..n {
                let expected = i as f64 * warm_slot;
                assert!(
                    (sample_x(i, n, FILL, CAP, w) - expected).abs() < 1e-9,
                    "n={n} i={i}: left-anchored, {i} slots from the left"
                );
            }
        }
        // The newest is strictly below the right edge until the last warm-up
        // sample — that is what makes the trace "fill" rather than snap.
        assert!(
            (sample_x(FILL - 2, FILL - 1, FILL, CAP, w) - w).abs() > 1e-9,
            "one short of fill the newest has not yet reached the right edge"
        );
    }

    #[test]
    fn test_sample_x_fills_full_width_at_fill() {
        // By the `fill`-th sample both edges are touched: oldest at the left,
        // newest at the right — the pane is full (≈ 10 s at the default cadence).
        let w = 400.0;
        assert_eq!(
            sample_x(0, FILL, FILL, CAP, w),
            0.0,
            "oldest is on the left edge at fill"
        );
        assert_eq!(
            sample_x(FILL - 1, FILL, FILL, CAP, w),
            w,
            "newest reaches the right edge exactly at fill"
        );
    }

    #[test]
    fn test_sample_x_settle_ramp_is_continuous_and_monotonic() {
        // Past `fill` the newest stays pinned to the right edge while the slot
        // spacing eases monotonically from the warm width down to the steady
        // width. Continuity at the boundary: the warm and settle expressions
        // agree at n = fill (both reach the left edge at i = 0).
        let w = 400.0;
        let warm_slot = w / (FILL - 1) as f64;
        let steady_slot = w / (CAP - 1) as f64;
        let ramp = (CAP - FILL) as f64;
        let mut prev_slot = warm_slot;
        for n in FILL + 1..=CAP {
            let t = (n - FILL) as f64 / ramp;
            let slot = warm_slot + t * (steady_slot - warm_slot);
            assert!(
                slot <= prev_slot + 1e-12,
                "n={n}: slot must monotonically shrink while settling"
            );
            prev_slot = slot;
            // Newest always pinned to the right edge once settled.
            assert!(
                (sample_x(n - 1, n, FILL, CAP, w) - w).abs() < 1e-9,
                "newest of {n} settles to the right edge"
            );
            // The window spans a growing time span: the oldest sample reports a
            // negative x once it scrolls off the left edge (the painter clips it),
            // rather than clamping to the left edge where a cluster would stack.
            let x_oldest = sample_x(0, n, FILL, CAP, w);
            assert!(
                x_oldest <= w,
                "n={n}: oldest x stays at or off the left edge (may be negative)"
            );
        }
        // At the steady window the full capacity of samples spans the width
        // (a float hair past 0 is fine: at the boundary the oldest just scrolls
        // off the edge).
        assert!(
            (sample_x(0, CAP, FILL, CAP, w) - 0.0).abs() < 1e-6,
            "oldest of a full window sits on (or off) the left edge"
        );
        assert_eq!(
            sample_x(CAP - 1, CAP, FILL, CAP, w),
            w,
            "newest of a full window sits on the right edge"
        );
    }

    #[test]
    fn test_sample_x_scroll_offs_the_left_edge() {
        // Beyond capacity, samples that sit more than `w` behind the newest have
        // scrolled off the left edge: `sample_x` reports a negative x for them
        // (the painter clips them away) instead of clamping them onto the left
        // edge, where the whole stale history would stack into one vertical line.
        // The newest stays pinned to the right edge and the left half is in-bounds.
        let w = 400.0;
        for n in CAP..=CAP + 50 {
            for i in 0..n {
                let x = sample_x(i, n, FILL, CAP, w);
                assert!(
                    x <= w && x > -w * 2.0,
                    "n={n} i={i}: x in (-{w2}, {w}] (negative = off the left edge)",
                    w2 = w * 2.0
                );
            }
            assert!(
                (sample_x(n - 1, n, FILL, CAP, w) - w).abs() < 1e-9,
                "newest at the right edge"
            );
            // At least the left half of the window is still in-bounds (the newest
            // samples), so the trace still reaches the left edge rather than
            // vanishing entirely.
            assert!(sample_x(n / 2, n, FILL, CAP, w) >= 0.0);
        }
    }

    #[test]
    fn test_sample_y_frac_inverts_and_clamps() {
        assert_eq!(sample_y_frac(100.0), 0.0);
        assert_eq!(sample_y_frac(0.0), 1.0);
        assert_eq!(sample_y_frac(50.0), 0.5);
        assert_eq!(sample_y_frac(150.0), 0.0, "clamped to top");
        assert_eq!(sample_y_frac(-20.0), 1.0, "clamped to bottom");
    }

    #[test]
    fn test_frac_of_maps_by_domain_and_clamps() {
        assert!((frac_of(50.0, 100.0) - 0.5).abs() < 1e-9);
        assert!((frac_of(3200.0, 4000.0) - 0.2).abs() < 1e-9);
        assert_eq!(frac_of(5000.0, 4000.0), 0.0, "over-domain clamps to top");
        assert_eq!(frac_of(-10.0, 4000.0), 1.0, "under-domain clamps to bottom");
        assert_eq!(frac_of(500.0, 0.0), 1.0, "zero domain -> bottom");
        assert!((sample_y_frac(50.0) - frac_of(50.0, 100.0)).abs() < 1e-9);
    }

    // ---- run splitting --------------------------------------------------

    #[test]
    fn test_iter_some_runs_contiguous() {
        let v: Vec<Option<f64>> = vec![Some(1.0), Some(2.0), Some(3.0)];
        assert_eq!(iter_some_runs(&v), vec![0..3]);
    }

    #[test]
    fn test_iter_some_runs_blank_middle() {
        let v: Vec<Option<f64>> = vec![Some(1.0), None, Some(3.0), None, Some(5.0)];
        assert_eq!(iter_some_runs(&v), vec![0..1, 2..3, 4..5]);
    }

    #[test]
    fn test_iter_some_runs_all_none() {
        let v: Vec<Option<f64>> = vec![None, None, None];
        assert!(iter_some_runs(&v).is_empty());
    }

    #[test]
    fn test_iter_some_runs_empty() {
        let v: Vec<Option<f64>> = Vec::new();
        assert!(iter_some_runs(&v).is_empty());
    }

    #[test]
    fn test_iter_some_runs_leading_trailing_gaps() {
        let v: Vec<Option<f64>> = vec![None, Some(1.0), Some(2.0), None];
        assert_eq!(iter_some_runs(&v), vec![1..3]);
    }

    // ---- palette + layout bounds ----------------------------------------

    #[test]
    fn test_series_points_land_in_bounds() {
        let s = samples(&[0.0, 25.0, 50.0, 75.0, 100.0]);
        let w = 400.0;
        let h = 90.0;
        let left = 20.0;
        let right = 10.0;
        let plot = plot_rect(w, h, left, right);
        let n = s.len();
        for (i, smp) in s.iter().enumerate() {
            let x = plot.ox + sample_x(i, n, FILL, CAP, plot.w);
            let y = plot.oy + frac_of(smp.cpu, 100.0) * plot.h;
            assert!((0.0..=w).contains(&x), "x at sample {i} in [0, {w})");
            assert!((0.0..=h).contains(&y), "y at sample {i} in [0, {h})");
        }
        assert_eq!(plot.w + left + right, w, "plot + gutters fill widget width");
    }

    #[test]
    fn test_four_series_values_land_in_bounds() {
        let w = 400.0;
        let h = 110.0;
        let plot = plot_rect(w, h, 30.0, 30.0);
        let vals: Vec<Option<f64>> = vec![
            Some(800.0),
            Some(1200.0),
            Some(2600.0),
            Some(4000.0),
            Some(5000.0), // over-domain clamps
        ];
        for (i, v) in vals.iter().enumerate() {
            let v = v.expect("present");
            let x = plot.ox + sample_x(i, vals.len(), FILL, CAP, plot.w);
            let y = plot.oy + frac_of(v, 4000.0) * plot.h;
            assert!((0.0..=w).contains(&x), "freq x in [0, {w})");
            assert!((0.0..=h).contains(&y), "freq y in [0, {h})");
        }
        let temps: Vec<Option<f64>> = vec![Some(20.0), Some(45.0), Some(75.0), Some(100.0)];
        for v in temps.iter() {
            let v = v.expect("present");
            let y = plot.oy + frac_of(v, 100.0) * plot.h;
            assert!((0.0..=h).contains(&y), "{v} °C must fit in [0, {h})");
        }
    }

    #[test]
    fn test_single_sample_is_left_anchored_in_bounds() {
        let w = 300.0;
        let h = 80.0;
        let plot = plot_rect(w, h, 25.0, 0.0);
        // A lone sample anchors the left edge of the plot (just inside the
        // left gutter), so the trace grows to the right from there.
        let x = plot.ox + sample_x(0, 1, FILL, CAP, plot.w);
        let y = plot.oy + frac_of(55.0, 100.0) * plot.h;
        assert_eq!(x, plot.ox, "a lone 'now' sample sits on the left plot edge");
        assert!((0.0..=w).contains(&x), "x in [0, {w}]");
        assert!((0.0..=h).contains(&y));
    }
}
