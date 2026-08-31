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

/// Maps the i-th of `n` samples (index 0 = oldest) to a pixel x coordinate on
/// the right-anchored rolling window of width `w`.
///
/// The latest sample (index `n - 1`) always lands on the right edge (`w`);
/// each older sample sits one fixed slot to its left, where
/// `slot = w / (capacity - 1)`. With `capacity == n` a full history spans the
/// entire width; a partial history occupies only the right-hand portion, so
/// the trace grows from right to left until it is full and then scrolls left.
/// Positions past the left edge clamp to 0. Indexes past `n - 1` clamp to the
/// latest position.
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
/// thin wrapper over [`frac_of`](the chart's percent series use it), and it
/// remains the canonical unit-mapping test target.
pub fn sample_y_frac(value: f64) -> f64 {
    frac_of(value, 100.0)
}

/// Draws one series (area fill + line + latest-sample dot) into `ctx`, laid
/// out across the widget's pixel size `(w, h)` on the right-anchored
/// `capacity`-slot window.
///
/// `value_of` picks the sampled value for each sample; returning `None` for a
/// sample leaves a blank gap at that x (the run before and after it is drawn
/// as its own connected path). `domain_max` is the value that maps to the top
/// of the chart — each series passes its own domain (`100.0` for the percent
/// series, the freq ceiling for the frequency series), so a series on a
/// different scale still reads as a normal curve.
fn draw_series(
    ctx: &Context,
    w: f64,
    h: f64,
    values: &[Option<f64>],
    capacity: usize,
    domain_max: f64,
    rgb: (u8, u8, u8),
) {
    let runs = iter_some_runs(values);
    let n = values.len();
    let (r, g, b) = (
        rgb.0 as f64 / 255.0,
        rgb.1 as f64 / 255.0,
        rgb.2 as f64 / 255.0,
    );
    let x_at = |i: usize| sample_x(i, n, capacity, w);
    let y_at = |i: usize| frac_of(values[i].unwrap_or(0.0), domain_max) * h;
    for run in &runs {
        let (a, z) = (run.start, run.end);
        // 1) Area fill (low alpha) from the bottom up to the run's curve.
        ctx.new_path();
        ctx.move_to(x_at(a), h);
        for i in a..z {
            ctx.line_to(x_at(i), y_at(i));
        }
        ctx.line_to(x_at(z - 1), h);
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
    //    so the "now" value stays visible even before a second sample arrives.
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
/// frequency axis (MHz); `capacity` is the rolling-window width in samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartConfig {
    /// Top of the frequency-axis domain in MHz.
    pub freq_max_mhz: f64,
    /// Number of samples a full rolling window holds.
    pub capacity: usize,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            freq_max_mhz: 4000.0,
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

/// The right-side axis tick rows for a chart of height `h`: `(fraction,
/// label)` pairs where the fraction is the normalized y position (0 = top,
/// 1 = bottom) and the label is the value at that row. Three ticks — the
/// domain top, a quarter, and the bottom (0) — are drawn, which is enough for
/// a quick read without crowding the graph.
fn axis_ticks(domain_max: f64, format: fn(f64) -> String) -> Vec<(f64, String)> {
    vec![
        (frac_of(domain_max, domain_max), format(domain_max)),
        (
            frac_of(domain_max * 0.25, domain_max),
            format(domain_max * 0.25),
        ),
        (frac_of(0.0, domain_max), format(0.0)),
    ]
}

/// Tick label columns: the frequency axis reads on the left of the chart, the
/// temperature axis on the right (a conventional dual-axis layout — each
/// non-percent series gets its own axis and side so the two never overlap).
const TICK_INSET: f64 = 4.0;
/// Baseline offset from a tick row, so the top tick (y=0) stays inside the
/// widget and the bottom tick (y=h) is drawn just above it.
const TICK_BASELINE: f64 = 3.0;

/// Whether an axis column sits on the left or right edge of the chart.
#[derive(Clone, Copy)]
enum AxisSide {
    Left,
    Right,
}

/// Draws the tick labels for one axis: three rows (the domain top, 25% of the
/// domain, and 0) tinted with `rgb` so the axis reads as belonging to that
/// series (frequency purple on the left, temperature red on the right).
fn draw_axis_ticks(
    ctx: &Context,
    w: f64,
    h: f64,
    domain_max: f64,
    rgb: (u8, u8, u8),
    label: fn(f64) -> String,
    side: AxisSide,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (r, g, b) = (
        rgb.0 as f64 / 255.0,
        rgb.1 as f64 / 255.0,
        rgb.2 as f64 / 255.0,
    );
    ctx.select_font_face(
        "sans-serif",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    ctx.set_font_size(9.0);
    for (frac, text) in axis_ticks(domain_max, label) {
        let y = frac * h;
        let width = ctx.text_extents(&text).map(|e| e.width()).unwrap_or(0.0);
        let x = match side {
            AxisSide::Left => TICK_INSET,
            AxisSide::Right => (w - width - TICK_INSET).max(0.0),
        };
        // Top tick sits below its row, bottom tick above, so both stay on-canvas.
        let baseline_y = if frac < 0.5 {
            y + TICK_BASELINE
        } else {
            y - TICK_BASELINE
        };
        ctx.set_source_rgb(r, g, b);
        ctx.move_to(x, baseline_y);
        let _ = ctx.show_text(&text);
    }
    // Faint guide along that side.
    let gx = match side {
        AxisSide::Left => 1.0,
        AxisSide::Right => w - 1.0,
    };
    ctx.set_source_rgba(r, g, b, FILL_ALPHA);
    ctx.set_line_width(1.0);
    ctx.move_to(gx, 0.0);
    ctx.line_to(gx, h);
    let _ = ctx.stroke();
}

/// Paints one pane of the split rolling-window usage chart into `ctx`,
/// spanning `(w, h)`.
///
/// `pane` selects which two series live in this pane:
///
/// * [`ChartPane::CpuMem`] — memory (blue) then CPU (green), both on the
///   shared 0–100% grid. No axis ticks: the percent scale is self-evident.
/// * [`ChartPane::FreqTemp`] — frequency (purple, domain `cfg.freq_max_mhz`)
///   then temperature (red, 0–100 °C), with their two dedicated axes drawn
///   color-matched (frequency on the left, temperature on the right).
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
    match pane {
        ChartPane::CpuMem => {
            let mem: Vec<Option<f64>> = samples.iter().map(|s| Some(s.mem)).collect();
            let cpu: Vec<Option<f64>> = samples.iter().map(|s| Some(s.cpu)).collect();
            // Draw order: memory (bottom), cpu (top).
            draw_series(ctx, w, h, &mem, cfg.capacity, 100.0, MEM_RGB);
            draw_series(ctx, w, h, &cpu, cfg.capacity, 100.0, CPU_RGB);
        }
        ChartPane::FreqTemp => {
            let freq: Vec<Option<f64>> = samples.iter().map(|s| s.freq).collect();
            let temp: Vec<Option<f64>> = samples.iter().map(|s| s.temp).collect();
            // Draw order: frequency, temperature (top).
            draw_series(ctx, w, h, &freq, cfg.capacity, cfg.freq_max_mhz, FREQ_RGB);
            draw_series(ctx, w, h, &temp, cfg.capacity, 100.0, TEMP_RGB);
            // Axes for the two non-percent series: frequency on the left,
            // temperature on the right, drawn after the fills so the labels
            // stay readable over the translucent bands.
            draw_axis_ticks(
                ctx,
                w,
                h,
                cfg.freq_max_mhz,
                FREQ_RGB,
                axis_tick_mhz,
                AxisSide::Left,
            );
            draw_axis_ticks(
                ctx,
                w,
                h,
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

    fn samples(values: &[f64]) -> Vec<Sample> {
        values
            .iter()
            .map(|v| Sample {
                cpu: *v,
                mem: *v,
                freq: None,
                temp: None,
            })
            .collect()
    }

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
        // 50% of a 100-domain sits at mid-height.
        assert!((frac_of(50.0, 100.0) - 0.5).abs() < 1e-9);
        // 3.2 GHz on a 4 GHz ceiling sits at 20% from the top (y).
        assert!((frac_of(3200.0, 4000.0) - 0.2).abs() < 1e-9);
        // Over-domain clamps to the top.
        assert_eq!(frac_of(5000.0, 4000.0), 0.0, "over-domain clamps to top");
        // Under-domain clamps to the bottom.
        assert_eq!(frac_of(-10.0, 4000.0), 1.0, "under-domain clamps to bottom");
        // Zero domain degenerates to the bottom (unused axis).
        assert_eq!(frac_of(500.0, 0.0), 1.0, "zero domain -> bottom");
        // The old percent mapping is preserved via sample_y_frac.
        assert!((sample_y_frac(50.0) - frac_of(50.0, 100.0)).abs() < 1e-9);
    }

    #[test]
    fn test_iter_some_runs_contiguous() {
        let v: Vec<Option<f64>> = vec![Some(1.0), Some(2.0), Some(3.0)];
        let runs = iter_some_runs(&v);
        assert_eq!(runs, vec![0..3]);
    }

    #[test]
    fn test_iter_some_runs_blank_middle() {
        let v: Vec<Option<f64>> = vec![Some(1.0), None, Some(3.0), None, Some(5.0)];
        let runs = iter_some_runs(&v);
        assert_eq!(runs, vec![0..1, 2..3, 4..5]);
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
        let runs = iter_some_runs(&v);
        assert_eq!(runs, vec![1..3]);
    }

    #[test]
    fn test_palette_constants_are_distinct() {
        // Every pair of series colors must be distinguishable.
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
        let n = s.len();
        for (i, smp) in s.iter().enumerate() {
            let x = sample_x(i, n, 120, w);
            let y = frac_of(smp.cpu, 100.0) * h;
            assert!((0.0..=w).contains(&x));
            assert!((0.0..=h).contains(&y));
        }
    }

    #[test]
    fn test_four_series_values_land_in_bounds() {
        // Frequency on its own ceiling and temperature on 0–100 °C must both
        // normalize into the chart, even though they share no scale with % or MHz.
        let w = 400.0;
        let h = 110.0;
        let vals: Vec<Option<f64>> = vec![
            Some(800.0),
            Some(1200.0),
            Some(2600.0),
            Some(4000.0),
            Some(5000.0), // over-domain clamps
        ];
        for (i, v) in vals.iter().enumerate() {
            let v = v.expect("present");
            let x = sample_x(i, vals.len(), 120, w);
            let y = frac_of(v, 4000.0) * h;
            assert!((0.0..=w).contains(&x));
            assert!((0.0..=h).contains(&y));
        }
        let temps: Vec<Option<f64>> = vec![Some(20.0), Some(45.0), Some(75.0), Some(100.0)];
        for v in temps.iter() {
            let v = v.expect("present");
            let y = frac_of(v, 100.0) * h;
            assert!((0.0..=h).contains(&y), "{v} °C must fit");
        }
    }

    #[test]
    fn test_axis_ticks_three_rows_evenly_spread() {
        let ticks = axis_ticks(4000.0, axis_tick_mhz);
        assert_eq!(ticks.len(), 3);
        // frac is the y position: top (0.0) = domain top, then 25% of the
        // domain value is 75% down the chart, and the bottom row is 0.
        assert_eq!(ticks[0].0, 0.0, "top tick at the ceiling");
        assert_eq!(
            ticks[1].0, 0.75,
            "quarter-value tick sits three-quarter down"
        );
        assert_eq!(ticks[2].0, 1.0, "bottom tick");
        assert_eq!(ticks[0].1, "4 GHz");
        assert_eq!(ticks[1].1, "1 GHz");
        assert_eq!(ticks[2].1, "0 MHz");
    }

    #[test]
    fn test_axis_tick_celsius_format() {
        let ticks = axis_ticks(100.0, axis_tick_celsius);
        assert_eq!(ticks[0].1, "100 °C");
        assert_eq!(ticks[1].1, "25 °C");
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

    #[test]
    fn test_single_sample_is_right_anchored_in_bounds() {
        let w = 300.0;
        let h = 80.0;
        let x = sample_x(0, 1, 120, w);
        let y = frac_of(55.0, 100.0) * h;
        assert_eq!(x, w, "the lone 'now' sample sits on the right edge");
        assert!((0.0..=h).contains(&y));
    }

    #[test]
    fn test_chart_config_defaults() {
        let cfg = ChartConfig::default();
        assert!(cfg.freq_max_mhz > 0.0);
        assert!(cfg.capacity >= 2);
    }

    #[test]
    fn test_chart_pane_is_copy_and_eq() {
        // Copy so the draw closures can reuse the selector freely.
        let a = ChartPane::CpuMem;
        let b = a;
        assert_eq!(a, b);
        assert_ne!(ChartPane::CpuMem, ChartPane::FreqTemp);
        let f = ChartPane::FreqTemp;
        assert_eq!(f, ChartPane::FreqTemp);
    }
}
