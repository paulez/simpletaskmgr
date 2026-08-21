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

/// Maps the i-th of `len` samples to a normalized x position (0..=1) across the
/// full width. A single sample is centered so it isn't pinned to the left edge;
/// two or more span edge to edge. Indexes past the end clamp to the last
/// position.
pub fn sample_x_frac(i: usize, len: usize) -> f64 {
    if len <= 1 {
        return 0.5;
    }
    let i = i.min(len - 1) as f64;
    let denominator = (len - 1) as f64;
    i / denominator
}

/// Maps a 0–100 percent value to a normalized y position (0..=1) where 100 is
/// the top (y=0) and 0 is the bottom (y=1). Values are clamped so they never
/// spill off the chart.
pub fn sample_y_frac(value: f64) -> f64 {
    let clamped = value.clamp(0.0, 100.0);
    1.0 - clamped / 100.0
}

/// Draws one series (area fill + line + latest-sample dot) into `ctx`, laid out
/// across the widget's pixel size `(w, h)`. `value_of` picks the sampled value
/// for either the CPU or memory field.
fn draw_series(
    ctx: &Context,
    w: f64,
    h: f64,
    samples: &[Sample],
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
            sample_x_frac(i, samples.len()) * w,
            sample_y_frac(value_of(&samples[i])) * h,
        )
    };

    // 1) Area fill (low alpha): bottom-left, samples in order, bottom-right.
    let n = samples.len();
    ctx.new_path();
    ctx.move_to(0.0, h);
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
    let (x0, y0) = to_px(0);
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

/// Paints the rolling-window usage chart into `ctx`, spanning `(w, h)`. Memory
/// is drawn first (below) and CPU on top; both share a 0–100% vertical axis
/// with translucent fills. An empty history paints a blank, valid chart.
pub fn paint_usage_chart(ctx: &Context, w: f64, h: f64, samples: &[Sample]) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    draw_series(ctx, w, h, samples, |s| s.mem, MEM_RGB);
    draw_series(ctx, w, h, samples, |s| s.cpu, CPU_RGB);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[f64]) -> Vec<Sample> {
        values
            .iter()
            .map(|v| Sample {
                t: 0.0,
                cpu: *v,
                mem: *v,
            })
            .collect()
    }

    #[test]
    fn test_sample_x_frac_spans_width() {
        assert_eq!(sample_x_frac(0, 5), 0.0);
        assert_eq!(sample_x_frac(4, 5), 1.0);
        assert_eq!(sample_x_frac(2, 5), 0.5);
        assert_eq!(sample_x_frac(9, 5), 1.0, "indexes past the end clamp");
    }

    #[test]
    fn test_sample_x_frac_single_is_centered() {
        assert_eq!(sample_x_frac(0, 1), 0.5);
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
        // First sample (value 0) and latest (value 100) must stay within the
        // widget's pixel bounds for a typical paint.
        let s = samples(&[0.0, 100.0]);
        let w = 400.0;
        let h = 90.0;
        let n = s.len();
        let first = (sample_x_frac(0, n) * w, sample_y_frac(s[0].cpu) * h);
        let last = (sample_x_frac(n - 1, n) * w, sample_y_frac(s[n - 1].cpu) * h);
        for (x, y) in [first, last] {
            assert!((0.0..=w).contains(&x));
            assert!((0.0..=h).contains(&y));
        }
    }

    #[test]
    fn test_single_sample_is_centered_in_bounds() {
        let s = samples(&[55.0]);
        let w = 300.0;
        let h = 80.0;
        let (x, y) = (sample_x_frac(0, 1) * w, sample_y_frac(s[0].cpu) * h);
        assert!((0.0..=w).contains(&x));
        assert!((0.0..=h).contains(&y));
    }
}
