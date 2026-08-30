use cairo::Context;

use crate::metrics::Sample;

/// RGB triplet (0–255) for the CPU series — green, matching the old floem
/// `CPU_COLOR = rgb8(76, 175, 80)` / SVG `#4CAF50`.
pub const CPU_RGB: (u8, u8, u8) = (76, 175, 80);
/// RGB triplet (0–255) for the memory series — blue, matching the old floem
/// `MEM_COLOR = rgb8(33, 150, 243)` / SVG `#2196F3`.
pub const MEM_RGB: (u8, u8, u8) = (33, 150, 243);
/// Translucent fill alpha so overlapping series areas read as distinct bands.
pub const FILL_ALPHA: f64 = 0.22;
/// Line width of each series (in pixels at the draw-time scale).
pub const LINE_WIDTH: f64 = 1.5;
/// Radius of the "latest sample" marker dot (in pixels).
pub const DOT_RADIUS: f64 = 1.5;

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

/// Maps a 0–100 percent value to a normalized y position (0..=1) where 100 is
/// the top (y=0) and 0 is the bottom (y=1). Values are clamped so they never
/// spill off the chart.
pub fn sample_y_frac(value: f64) -> f64 {
    let clamped = value.clamp(0.0, 100.0);
    1.0 - clamped / 100.0
}

/// Draws one series (area fill + line + latest-sample dot) into `ctx`, laid out
/// across the widget's pixel size `(w, h)` on the right-anchored `capacity`-slot
/// window. `value_of` picks the sampled value for either the CPU or memory
/// field.
fn draw_series(
    ctx: &Context,
    w: f64,
    h: f64,
    samples: &[Sample],
    capacity: usize,
    value_of: impl Fn(&Sample) -> f64,
    rgb: (u8, u8, u8),
) {
    if samples.is_empty() {
        return;
    }
    let (r, g, b) = (
        rgb.0 as f64 / 255.0,
        rgb.1 as f64 / 255.0,
        rgb.2 as f64 / 255.0,
    );
    let to_px = |i: usize| {
        (
            sample_x(i, samples.len(), capacity, w),
            sample_y_frac(value_of(&samples[i])) * h,
        )
    };

    let n = samples.len();

    // 1) Area fill (low alpha): the bottom of the first sample's column, the
    //    samples in order, then the bottom-right corner. Filling starts under
    //    the first (oldest) sample, so the unfilled left portion of the window
    //    stays blank until the history grows to full capacity.
    let (x0, y0) = to_px(0);
    ctx.new_path();
    ctx.move_to(x0, h);
    for i in 0..n {
        let (x, y) = to_px(i);
        ctx.line_to(x, y);
    }
    ctx.line_to(w, h);
    ctx.close_path();
    ctx.set_source_rgba(r, g, b, FILL_ALPHA);
    let _ = ctx.fill();

    // 2) Series outline.
    ctx.new_path();
    ctx.move_to(x0, y0);
    for i in 1..n {
        let (x, y) = to_px(i);
        ctx.line_to(x, y);
    }
    ctx.set_source_rgb(r, g, b);
    ctx.set_line_width(LINE_WIDTH);
    let _ = ctx.stroke();

    // 3) "Latest sample" marker so the "now" value stays visible even before a
    //    second sample arrives to form a line.
    let (cx, cy) = to_px(n - 1);
    ctx.new_path();
    ctx.arc(cx, cy, DOT_RADIUS, 0.0, std::f64::consts::TAU);
    ctx.set_source_rgb(r, g, b);
    let _ = ctx.fill();
}

/// Paints the rolling-window usage chart into `ctx`, spanning `(w, h)`.
/// `capacity` is the sample count a full window holds (history cap); fewer
/// samples occupy only the right-hand portion of the width. Memory is drawn
/// first (below) and CPU on top; both share a 0–100% vertical axis with
/// translucent fills. An empty history paints a blank, valid chart.
pub fn paint_usage_chart(ctx: &Context, w: f64, h: f64, samples: &[Sample], capacity: usize) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    draw_series(ctx, w, h, samples, capacity, |s| s.mem, MEM_RGB);
    draw_series(ctx, w, h, samples, capacity, |s| s.cpu, CPU_RGB);
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
        // One sample short of a full window leaves exactly one slot (w/(cap-1))
        // of blank space on the left. Tolerance comparison: the function
        // computes `w - (n-1) * (w/(cap-1))`, which rounds a few uops apart
        // from `w * (cap-n)/(cap-1)`.
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
    fn test_palette_constants_are_distinct() {
        assert_ne!(
            CPU_RGB, MEM_RGB,
            "CPU and mem swatches must be distinguishable"
        );
        assert_eq!(FILL_ALPHA, 0.22);
    }

    #[test]
    fn test_series_points_land_in_bounds() {
        // Every sample of a short history must stay within the widget's pixel
        // bounds: right-anchored, stepping left by one slot each.
        let s = samples(&[0.0, 25.0, 50.0, 75.0, 100.0]);
        let w = 400.0;
        let h = 90.0;
        let n = s.len();
        for (i, smp) in s.iter().enumerate() {
            let (x, y) = (sample_x(i, n, 120, w), sample_y_frac(smp.cpu) * h);
            assert!((0.0..=w).contains(&x));
            assert!((0.0..=h).contains(&y));
        }
    }

    #[test]
    fn test_single_sample_is_right_anchored_in_bounds() {
        let w = 300.0;
        let h = 80.0;
        let (x, y) = (sample_x(0, 1, 120, w), sample_y_frac(55.0) * h);
        assert_eq!(x, w, "the lone 'now' sample sits on the right edge");
        assert!((0.0..=h).contains(&y));
    }
}
