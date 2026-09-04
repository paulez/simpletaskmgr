//! GPU status readouts via `rocm-smi`: utilization, VRAM, and temperature.
//!
//! Data is collected by spawning `rocm-smi --showuse --showmemuse --showtemp
//! --json` and parsing the JSON it writes to stdout (diagnostic warnings are
//! sent to stderr and intentionally ignored). The first listed card (`card0`)
//! is the representative GPU, matching the way core 0 is used as the
//! representative CPU. Every read degrades gracefully to `None` when
//! `rocm-smi` is absent or reports no card, so the UI can hide its tab and
//! the sampler can carry the previous reading forward.

use log::{debug, warn};

/// A single GPU card's status reading. `use_pct` and `vram_pct` are 0–100
/// percentages; `temp_c` is the edge-sensor temperature in °C.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuSample {
    /// GPU utilization percent (0–100), from `rocm-smi` "GPU use (%)".
    pub use_pct: f64,
    /// VRAM allocation percent (0–100), from `rocm-smi` "GPU Memory Allocated (VRAM%)".
    pub vram_pct: f64,
    /// GPU edge temperature in °C, from `rocm-smi` "Temperature (Sensor edge) (C)".
    pub temp_c: f64,
}

/// The `rocm-smi` JSON key we require: the first GPU card. A system with
/// multiple cards exposes `card0`, `card1`, …; we report `card0` as the
/// representative, consistent with the core-0 CPU convention.
pub const CARD_KEY: &str = "card0";

/// The `GPU use (%)` field name exactly as emitted by `rocm-smi --json`.
/// Kept as a constant so a rename in the tool is a single-line fix.
const FIELD_USE: &str = "GPU use (%)";
/// The `GPU Memory Allocated (VRAM%)` field name.
const FIELD_VRAM: &str = "GPU Memory Allocated (VRAM%)";
/// The edge-sensor temperature field name.
const FIELD_TEMP_EDGE: &str = "Temperature (Sensor edge) (C)";

/// Parses one card's section of the `rocm-smi --showuse --showmemuse
/// --showtemp --json` document into a [`GpuSample`].
///
/// The input is the *raw* JSON document (the outer object keyed by
/// `card0`/`card1`/…). Returns `None` when the document is not valid JSON,
/// the requested `card` is absent, or any of the required fields is missing
/// or unparseable. All numbers are expected from `rocm-smi` as *strings*
/// (e.g. `"97"`), but a real number is also accepted so future changes that
/// drop the quote do not silently break us.
///
/// Pure: no I/O, so it is easy to test against fixed fixtures.
pub fn parse_gpu_json(doc: &str, card: &str) -> Option<GpuSample> {
    let root: serde_json::Value = serde_json::from_str(doc).ok()?;
    let card_obj = root.get(card)?;
    let use_pct = read_number(card_obj, FIELD_USE)?;
    let vram_pct = read_number(card_obj, FIELD_VRAM)?;
    let temp_c = read_number(card_obj, FIELD_TEMP_EDGE)?;
    Some(GpuSample {
        use_pct,
        vram_pct,
        temp_c,
    })
}

/// Reads a single numeric field from a JSON object. `rocm-smi` emits numbers
/// as strings (`"97"`); we accept both strings and JSON numbers so a format
/// change is tolerated. Returns `None` when the field is missing, `"N/A"`,
/// or non-numeric.
fn read_number(obj: &serde_json::Value, key: &str) -> Option<f64> {
    let v = obj.get(key)?;
    if let Some(s) = v.as_str() {
        s.trim().parse::<f64>().ok()
    } else {
        v.as_f64()
    }
}

/// Reads the first card (`card0`) from the live `rocm-smi`. Returns `None`
/// (and logs at debug) when `rocm-smi` is missing, exits non-zero, or reports
/// a document we cannot parse — a normal, expected condition on hosts without
/// an AMD GPU. Used both by the per-sample `push_sample` and by the
/// startup probe (`gpu_available`).
pub fn read_gpu_card() -> Option<GpuSample> {
    let output = std::process::Command::new("rocm-smi")
        .args(["--showuse", "--showmemuse", "--showtemp", "--json"])
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            debug!("rocm-smi exited {}: {:?}", o.status, o.status);
            return None;
        }
        Err(e) => {
            // rocm-smi not on PATH is the common case on non-AMD hosts.
            debug!("Can't run rocm-smi: {e:?}");
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_gpu_json(&stdout, CARD_KEY) {
        Some(s) => {
            debug!(
                "GPU use {0:.0}% vram {1:.0}% temp {2:.1} C",
                s.use_pct, s.vram_pct, s.temp_c
            );
            Some(s)
        }
        None => {
            warn!("rocm-smi produced a document without card0 or the required fields");
            None
        }
    }
}

/// Whether a GPU is available *for us to sample* — i.e. `rocm-smi` exists,
/// runs successfully, and reports a readable `card0`. Called exactly once at
/// startup by the UI: when `false` the UI hides the GPU tab and the sampler
/// skips the per-tick `rocm-smi` spawn entirely.
///
/// This is the same spawn path as [`read_gpu_card`]; the name is what the UI
/// reads, the helper is what the sampler reads — both mean "the read we need
/// will succeed at least sometimes."
pub fn gpu_available() -> bool {
    read_gpu_card().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Pure parser --------------------------------------------------

    /// A representative `rocm-smi --json` document: card0 with numbers as
    /// strings. Parses to the expected (use, vram, temp) triple.
    #[test]
    fn test_parse_gpu_json_valid() {
        let doc = r#"{"card0": {"GPU use (%)": "42", "GPU Memory Allocated (VRAM%)": "77", "Temperature (Sensor edge) (C)": "61.0"}}"#;
        assert_eq!(
            parse_gpu_json(doc, "card0"),
            Some(GpuSample {
                use_pct: 42.0,
                vram_pct: 77.0,
                temp_c: 61.0,
            })
        );
    }

    /// `rocm-smi` may emit numbers either as strings or as JSON numbers; both
    /// parse. The string form is what we see today.
    #[test]
    fn test_parse_gpu_json_accepts_json_numbers() {
        let doc = r#"{"card0": {"GPU use (%)": 12, "GPU Memory Allocated (VRAM%)": 33, "Temperature (Sensor edge) (C)": 44.5}}"#;
        assert_eq!(
            parse_gpu_json(doc, "card0"),
            Some(GpuSample {
                use_pct: 12.0,
                vram_pct: 33.0,
                temp_c: 44.5,
            })
        );
    }

    /// Missing card → `None`. The caller is expected to try the next card if
    /// it wants to (we do not — we only report `card0`).
    #[test]
    fn test_parse_gpu_json_missing_card() {
        let doc = r#"{"card1": {"GPU use (%)": "42"}}"#;
        assert_eq!(parse_gpu_json(doc, "card0"), None);
    }

    /// Missing a required field → `None`, not zero. A `use` reading that
    /// failed to parse must not be confused with a real `0%` reading.
    #[rstest::rstest]
    #[case::missing_use(r#"{"card0": {"GPU Memory Allocated (VRAM%)": "97", "Temperature (Sensor edge) (C)": "70.0"}}"#)]
    #[case::missing_vram(
        r#"{"card0": {"GPU use (%)": "0", "Temperature (Sensor edge) (C)": "70.0"}}"#
    )]
    #[case::missing_temp(
        r#"{"card0": {"GPU use (%)": "0", "GPU Memory Allocated (VRAM%)": "97"}}"#
    )]
    #[case::empty_card(r#"{"card0": {}}"#)]
    fn test_parse_gpu_json_missing_field(#[case] doc: &str) {
        assert_eq!(parse_gpu_json(doc, "card0"), None);
    }

    /// `rocm-smi` emits `"N/A"` for values that cannot be measured. The
    /// parser must reject any non-numeric field rather than coercing it to 0.
    #[test]
    fn test_parse_gpu_json_rejects_na() {
        let doc = r#"{"card0": {"GPU use (%)": "N/A", "GPU Memory Allocated (VRAM%)": "97", "Temperature (Sensor edge) (C)": "70.0"}}"#;
        assert_eq!(parse_gpu_json(doc, "card0"), None);
    }

    /// Garbage (non-JSON) input → `None`. `rocm-smi` is supposed to emit JSON
    /// on `--json`, but the parser must survive a format regression without a
    /// panic.
    #[test]
    fn test_parse_gpu_json_rejects_non_json() {
        assert_eq!(parse_gpu_json("not json", "card0"), None);
        assert_eq!(parse_gpu_json("", "card0"), None);
    }

    // ---- Live I/O (smoke test) ----------------------------------------

    /// On a host with an AMD GPU and `rocm-smi` on PATH, the reading is
    /// present and physically sane. On a host without either (CI, CI, or a
    /// non-AMD box), `read_gpu_card` returns `None` — both are acceptable
    /// outcomes. A `Some` reading must be in the 0–100% band and a positive
    /// (but not ridiculous) temperature.
    #[test]
    fn test_read_gpu_card_sane_when_present() {
        if let Some(s) = read_gpu_card() {
            assert!((0.0..=100.0).contains(&s.use_pct), "use_pct in 0–100");
            assert!((0.0..=100.0).contains(&s.vram_pct), "vram_pct in 0–100");
            assert!(s.temp_c > 0.0, "a present GPU temp must be positive");
            assert!(
                s.temp_c < 120.0,
                "a GPU temp above 120 °C indicates a parsing bug"
            );
        }
    }

    /// `gpu_available` must agree with `read_gpu_card` on the same host.
    #[test]
    fn test_gpu_available_matches_read_gpu_card() {
        assert_eq!(gpu_available(), read_gpu_card().is_some());
    }
}
