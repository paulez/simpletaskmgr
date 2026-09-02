use cairo::Context;

use crate::metrics::Sample;

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
/// Line width of each series (in pixels at the draw-time scale).
pub const LINE_WIDTH: f64 = 1.5;
/// Radius of the "latest sample" marker dot (in pixels).
pub const DOT_RADIUS: f64 = 1.5;

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

/// Maps the i-th of `n` samples (index 0 = oldest) to a pixel x offset on the
/// right-anchored rolling window of width `w`.
///
/// The latest sample (index `n - 1`) always lands on the right edge (`w`);
/// each older sample sits one fixed slot to its left, where
/// `slot = w / (capacity - 1)`. With `capacity == n` a full history spans the
/// entire width; a partial history occupies only the right-hand portion, so
/// the trace grows from right to left until it is full and then scrolls
/// left. Positions past the left edge clamp to 0. Indexes past `n - 1` clamp
/// to the latest position.
pub fn sample_x(i: usize, n: usize, capacity: usize, w: f64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let cap = capacity.max(2);
    let slot = w / (cap - 1) as f64;
    let steps_back = (n - 1 - i.min(n - 1)) as f64;
    (w - steps_back * slot).max(0.0)
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
    capacity: usize,
    domain_max: f64,
    rgb: (u8, u8, u8),
) {
    if plot.w <= 0.0 || plot.h <= 0.0 {
        return;
    }
    let runs = iter_some_runs(values);
    let n = values.len();
    let (r, g, b) = (
        rgb.0 as f64 / 255.0,
        rgb.1 as f64 / 255.0,
        rgb.2 as f64 / 255.0,
    );
    let x_at = |i: usize| plot.ox + sample_x(i, n, capacity, plot.w);
    let y_at = |i: usize| plot.oy + frac_of(values[i].unwrap_or(0.0), domain_max) * plot.h;
    let bottom = plot.oy + plot.h;
    for run in &runs {
        let (a, z) = (run.start, run.end);
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
/// installed RAM, like `freq_max_mhz` is `scaling_max_freq`), and `capacity`
/// is the rolling-window width in samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartConfig {
    /// Top of the frequency-axis domain in MHz.
    pub freq_max_mhz: f64,
    /// Top of the memory-axis domain in MB (installed RAM).
    pub mem_max_mb: f64,
    /// Number of samples a full rolling window holds.
    pub capacity: usize,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            freq_max_mhz: 4000.0,
            // A typical desktop; overridden by the live `MemTotal` at runtime.
            mem_max_mb: 16384.0,
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
    let (r, g, b) = (
        rgb.0 as f64 / 255.0,
        rgb.1 as f64 / 255.0,
        rgb.2 as f64 / 255.0,
    );
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
    ctx.set_font_size(9.0);
    match pane {
        ChartPane::CpuMem => {
            // Each series has its own axis: CPU on the left in % (0–100), and
            // RAM on the right in MB scaled to the installed RAM, so both read
            // as full-height curves rather than sharing one % scale.
            let cpu: Vec<Option<f64>> = samples.iter().map(|s| Some(s.cpu)).collect();
            let mem: Vec<Option<f64>> = samples
                .iter()
                .map(|s| Some(s.mem / 100.0 * cfg.mem_max_mb))
                .collect();
            let left_gutter = measure_gutter(
                ctx,
                &[
                    axis_tick_percent(100.0),
                    axis_tick_percent(50.0),
                    axis_tick_percent(0.0),
                ],
            );
            let right_gutter = measure_gutter(
                ctx,
                &[
                    axis_tick_mb(cfg.mem_max_mb),
                    axis_tick_mb(cfg.mem_max_mb / 2.0),
                    axis_tick_mb(0.0),
                ],
            );
            let plot = plot_rect(w, h, left_gutter, right_gutter);
            // Draw order: memory (bottom), cpu (top).
            draw_series(ctx, &plot, &mem, cfg.capacity, cfg.mem_max_mb, MEM_RGB);
            draw_series(ctx, &plot, &cpu, cfg.capacity, 100.0, CPU_RGB);
            // CPU axis on the left (green, matching the CPU series), RAM axis
            // on the right (blue, matching the memory series).
            draw_axis_ticks(
                ctx,
                &plot,
                100.0,
                CPU_RGB,
                axis_tick_percent,
                AxisSide::Left,
            );
            draw_axis_ticks(
                ctx,
                &plot,
                cfg.mem_max_mb,
                MEM_RGB,
                axis_tick_mb,
                AxisSide::Right,
            );
        }
        ChartPane::FreqTemp => {
            let freq: Vec<Option<f64>> = samples.iter().map(|s| s.freq).collect();
            let temp: Vec<Option<f64>> = samples.iter().map(|s| s.temp).collect();
            let freq_labels = vec![
                axis_tick_mhz(cfg.freq_max_mhz),
                axis_tick_mhz(cfg.freq_max_mhz / 2.0),
                axis_tick_mhz(0.0),
            ];
            let temp_labels = vec![
                axis_tick_celsius(100.0),
                axis_tick_celsius(50.0),
                axis_tick_celsius(0.0),
            ];
            let left_gutter = measure_gutter(ctx, &freq_labels);
            let right_gutter = measure_gutter(ctx, &temp_labels);
            let plot = plot_rect(w, h, left_gutter, right_gutter);
            // Draw order: frequency, temperature (top).
            draw_series(ctx, &plot, &freq, cfg.capacity, cfg.freq_max_mhz, FREQ_RGB);
            draw_series(ctx, &plot, &temp, cfg.capacity, 100.0, TEMP_RGB);
            // Frequency axis on the left, temperature axis on the right — a
            // conventional dual-axis layout where each non-percent series
            // reads its own colored tick column outside the plot.
            draw_axis_ticks(
                ctx,
                &plot,
                cfg.freq_max_mhz,
                FREQ_RGB,
                axis_tick_mhz,
                AxisSide::Left,
            );
            draw_axis_ticks(
                ctx,
                &plot,
                100.0,
                TEMP_RGB,
                axis_tick_celsius,
                AxisSide::Right,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_axis_tick_percent_format() {
        assert_eq!(axis_tick_percent(100.0), "100%");
        assert_eq!(axis_tick_percent(25.0), "25%");
        assert_eq!(axis_tick_percent(0.0), "0%");
        assert_eq!(axis_tick_percent(50.0), "50%");
    }

    #[test]
    fn test_axis_tick_mb_format() {
        // A clean GiB multiple switches to GB.
        assert_eq!(axis_tick_mb(16384.0), "16 GB");
        assert_eq!(axis_tick_mb(2048.0), "2 GB");
        // An odd MB value stays in MB (not a fractional GB).
        assert_eq!(axis_tick_mb(4096.0), "4 GB");
        assert_eq!(axis_tick_mb(1536.0), "1536 MB");
        assert_eq!(axis_tick_mb(512.0), "512 MB");
        assert_eq!(axis_tick_mb(0.0), "0 MB");
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

    // ---- x/y mappings ---------------------------------------------------

    #[test]
    fn test_sample_x_latest_pins_to_right_edge() {
        let w = 400.0;
        assert_eq!(sample_x(4, 5, 120, w), w);
        assert_eq!(
            sample_x(0, 1, 120, w),
            w,
            "a lone sample sits on the right edge"
        );
        assert_eq!(
            sample_x(9, 5, 120, w),
            w,
            "indexes past the end clamp to the latest position"
        );
    }

    #[test]
    fn test_sample_x_steps_left_one_slot_older() {
        let w = 480.0;
        let cap = 10;
        let slot = w / (cap - 1) as f64;
        let n = 3;
        assert_eq!(
            sample_x(n - 1, n, cap, w),
            w,
            "latest sits on the right edge"
        );
        assert_eq!(
            sample_x(n - 2, n, cap, w),
            w - slot,
            "one older steps back one slot"
        );
        assert_eq!(
            sample_x(n - 3, n, cap, w),
            w - 2.0 * slot,
            "two older step back two slots"
        );
    }

    #[test]
    fn test_sample_x_full_window_reaches_left_edge() {
        let w = 400.0;
        let cap = 120;
        assert_eq!(
            sample_x(0, cap, cap, w),
            0.0,
            "a full history touches the left edge"
        );
        let expected = w * (cap - (cap - 1)) as f64 / (cap - 1) as f64;
        assert!(
            (sample_x(0, cap - 1, cap, w) - expected).abs() < 1e-9,
            "one sample short of a full window leaves one slot blank on the left"
        );
    }

    #[test]
    fn test_sample_x_clamps_at_left_edge() {
        let w = 400.0;
        let x = sample_x(0, 200, 5, w);
        assert!((0.0..=w).contains(&x), "over-capacity histories clamp at 0");
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
    fn test_palette_constants_are_distinct() {
        let all = [CPU_RGB, MEM_RGB, FREQ_RGB, TEMP_RGB];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "swatch {i} and {j} differ");
            }
        }
        assert_eq!(FILL_ALPHA, 0.22);
    }

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
            let x = plot.ox + sample_x(i, n, 120, plot.w);
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
            let x = plot.ox + sample_x(i, vals.len(), 120, plot.w);
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
    fn test_single_sample_is_right_anchored_in_bounds() {
        let w = 300.0;
        let h = 80.0;
        let plot = plot_rect(w, h, 25.0, 0.0);
        // With a left gutter, the rightmost plot edge is w - right_gutter,
        // so the latest sample sits on that edge (not the widget edge).
        let x = plot.ox + sample_x(0, 1, 120, plot.w);
        let y = plot.oy + frac_of(55.0, 100.0) * plot.h;
        assert_eq!(
            x, w,
            "lone 'now' sample sits on the rightmost plot edge when right_gutter is 0"
        );
        assert!((0.0..=h).contains(&y));
    }

    #[test]
    fn test_chart_config_defaults() {
        let cfg = ChartConfig::default();
        assert!(cfg.freq_max_mhz > 0.0);
        assert!(cfg.mem_max_mb > 0.0);
        assert!(cfg.capacity >= 2);
    }

    #[test]
    fn test_chart_pane_is_copy_and_eq() {
        let a = ChartPane::CpuMem;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(ChartPane::CpuMem, ChartPane::FreqTemp);
        let f = ChartPane::FreqTemp;
        assert_eq!(f, ChartPane::FreqTemp);
    }
}
