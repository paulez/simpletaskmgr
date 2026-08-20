use floem::prelude::{Color, RwSignal, SignalGet};
use floem::views::{container, empty, h_stack, label, svg, v_stack, Decorators};

use crate::metrics::Sample;

/// Solid colors for each series (mirrored between the SVG and the legend).
pub const CPU_COLOR: Color = Color::rgb8(76, 175, 80);
pub const MEM_COLOR: Color = Color::rgb8(33, 150, 243);
/// 255-alpha equivalents of the above used inside the generated SVG.
pub const CPU_HEX: &str = "#4CAF50";
pub const MEM_HEX: &str = "#2196F3";

/// Logical width (in SVG user units) of the generated chart.
pub const CHART_W: f64 = 100.0;
/// Logical height of the generated chart.
pub const CHART_H: f64 = 60.0;

/// Maps the i-th of `len` samples to an x position across the full width.
///
/// A single sample is centered so it isn't pinned to the left edge; two or
/// more span edge to edge. Indexes past the end clamp to the last position.
pub fn sample_x(i: usize, len: usize) -> f64 {
    sample_x_in(i, len, CHART_W)
}

/// Maps a 0-100 percent value to a y position, where 100 is the top (y=0) and
/// 0 is the bottom (y=h). Values are clamped so they never spill off the chart.
pub fn sample_y(value: f64, h: f64) -> f64 {
    let clamped = value.clamp(0.0, 100.0);
    h - (clamped / 100.0) * h
}

fn sample_x_in(i: usize, len: usize, w: f64) -> f64 {
    if len <= 1 {
        return w / 2.0;
    }
    (i.min(len - 1) as f64 / (len - 1) as f64) * w
}

/// Renders a single metric's area fill, top line and current-value marker into
/// `out`.
fn draw_series(
    out: &mut String,
    samples: &[Sample],
    value_of: impl Fn(&Sample) -> f64,
    color: &str,
) {
    if samples.is_empty() {
        return;
    }
    let pts: Vec<String> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{:.2},{:.2}",
                sample_x_in(i, samples.len(), CHART_W),
                sample_y(value_of(s), CHART_H)
            )
        })
        .collect();

    // Area: bottom-left corner, the series in order, bottom-right corner.
    let mut area = vec![format!("{:.2},{:.2}", 0.0, CHART_H)];
    area.extend(pts.iter().cloned());
    area.push(format!("{:.2},{:.2}", CHART_W, CHART_H));

    out.push_str(&format!(
        "<polygon points=\"{}\" fill=\"{}\" fill-opacity=\"0.22\"/>",
        area.join(" "),
        color
    ));
    out.push_str(&format!(
        "<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\"/>",
        pts.join(" "),
        color
    ));

    // A small dot on the latest sample so the "now" value is always visible,
    // even before a second sample arrives to form a line.
    if let Some(s) = samples.last() {
        let cx = sample_x_in(samples.len() - 1, samples.len(), CHART_W);
        let cy = sample_y(value_of(s), CHART_H);
        out.push_str(&format!(
            "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"1.5\" fill=\"{}\"/>",
            cx, cy, color
        ));
    }
}

/// Renders the current sample history as an SVG string.
///
/// Memory (blue) is drawn first so CPU (green) is painted on top. The result is
/// a self-contained, valid SVG at all times (including an empty history), so it
/// can be handed directly to an `svg` view.
pub fn render_usage_svg(samples: &[Sample]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {0} {1}\">",
        CHART_W, CHART_H
    ));
    draw_series(&mut out, samples, |s| s.mem, MEM_HEX);
    draw_series(&mut out, samples, |s| s.cpu, CPU_HEX);
    out.push_str("</svg>");
    out
}

/// A small colored square used as a legend swatch.
fn swatch(color: Color) -> impl floem::IntoView {
    container(empty()).style(move |s| {
        s.width(12.0)
            .height(12.0)
            .background(color)
            .border_radius(2.0)
    })
}

/// Renders a legend row pairing a colored swatch with its metric name.
fn legend_item(color: Color, name: &'static str) -> impl floem::IntoView {
    h_stack((swatch(color), label(move || name.to_string())))
        .style(move |s| s.items_center().gap(4.0).font_size(13.0))
}

/// A rolling-window area chart of system CPU% and memory% over time.
///
/// The `svg` view re-renders whenever `history` changes (each refresh appends a
/// sample), so the graph scrolls continuously to the right. Both metrics share
/// a 0-100% vertical axis; memory is drawn first (below) and CPU on top with
/// translucent fills so overlapping regions read as distinct bands. A legend on
/// the right identifies the two series.
pub fn usage_graph_view(history: RwSignal<Vec<Sample>>) -> impl floem::IntoView {
    let graph = svg(String::new())
        .update_value(move || render_usage_svg(&history.get()))
        .style(move |s| s.size_full());

    let legend = v_stack((legend_item(CPU_COLOR, "CPU"), legend_item(MEM_COLOR, "Mem")))
        .style(move |s| s.gap(4.0).padding(6.0).items_center());

    container(
        h_stack((
            container(graph).style(move |s| {
                s.size_full()
                    .background(Color::rgb8(250, 250, 250))
                    .border(1.0)
            }),
            container(legend).style(move |s| {
                s.border_left(1.0)
                    .border_color(Color::rgb8(200, 200, 200))
                    .padding_left(6.0)
                    .height_full()
                    .items_center()
            }),
        ))
        .style(move |s| {
            s.width_full()
                .height(90.0)
                .padding(8.0)
                .gap(6.0)
                .items_center()
        }),
    )
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
    fn test_sample_x_spans_width() {
        assert_eq!(sample_x(0, 5), 0.0);
        assert_eq!(sample_x(4, 5), CHART_W);
        assert_eq!(sample_x(2, 5), CHART_W / 2.0);
        assert_eq!(sample_x(9, 5), CHART_W, "indexes past the end clamp");
    }

    #[test]
    fn test_sample_x_single_is_centered() {
        assert_eq!(sample_x(0, 1), CHART_W / 2.0);
    }

    #[test]
    fn test_sample_y_inverts_and_clamps() {
        assert_eq!(sample_y(100.0, CHART_H), 0.0);
        assert_eq!(sample_y(0.0, CHART_H), CHART_H);
        assert_eq!(sample_y(50.0, CHART_H), CHART_H / 2.0);
        assert_eq!(sample_y(150.0, CHART_H), 0.0, "clamped to top");
        assert_eq!(sample_y(-20.0, CHART_H), CHART_H, "clamped to bottom");
    }

    #[test]
    fn test_render_usage_svg_is_valid_for_various_lengths() {
        for values in [&[] as &[f64], &[42.0], &[0.0, 100.0], &[10.0, 50.0, 90.0]] {
            let svg = render_usage_svg(&samples(values));
            assert!(svg.starts_with("<svg"), "should be well-formed: {svg}");
            assert!(svg.ends_with("</svg>"));
            if values.is_empty() {
                // Still a valid, empty chart.
                continue;
            }
            assert!(svg.contains(CPU_HEX), "CPU series present");
            assert!(svg.contains(MEM_HEX), "memory series present");
            assert!(svg.contains("<polyline"), "a line is drawn");
        }
    }

    /// A known sample must land at a predictable coordinate (value 100% sits on
    /// the top edge, 0% on the bottom edge).
    #[test]
    fn test_render_usage_svg_point_positions() {
        let svg = render_usage_svg(&samples(&[0.0, 100.0]));
        // First sample (value 0) maps to the bottom-left; second (value 100)
        // maps to the top-right. Both colors use the same coordinates.
        assert!(
            svg.contains(&format!("{:.2},{:.2}", CHART_W, 0.0)),
            "100% sample should be at the top-right"
        );
        assert!(
            svg.contains(&format!("{:.2},{:.2}", 0.0, CHART_H)),
            "0% sample should be at the bottom-left"
        );
    }
}
