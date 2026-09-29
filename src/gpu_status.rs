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

/// Bounded wait for a `rocm-smi` answer. The spawn runs on the GTK main
/// loop every sampling tick, so a wedged `rocm-smi` (driver faults happen;
/// the process can block in the kernel) must not be able to freeze the UI.
const ROCSMI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The failure modes of [`run_with_timeout`].
pub enum GpuRunError {
    /// `rocm-smi` could not be spawned (usually: not on `PATH`).
    Spawn(std::io::Error),
    /// `rocm-smi` exited non-zero, with this code.
    ExitCode(i32),
    /// `rocm-smi` did not finish within the deadline (it was killed).
    TimedOut(std::time::Duration),
}

impl std::fmt::Debug for GpuRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuRunError::Spawn(e) => f.debug_tuple("Spawn").field(e).finish(),
            GpuRunError::ExitCode(c) => f.debug_tuple("ExitCode").field(c).finish(),
            GpuRunError::TimedOut(d) => f.debug_tuple("TimedOut").field(d).finish(),
        }
    }
}

/// Runs a subprocess and returns its stdout, bounded by a wall-clock deadline.
///
/// The child's standard output is drained by a small reader thread (so a
/// chatty child cannot fill the pipe buffer and hang), and the caller polls
/// `try_wait()` until `timeout` elapses. If the child does not finish in time
/// it is killed and the call returns [`GpuRunError::TimedOut`]. The caller is
/// on the GTK main loop, so nothing in this path may block indefinitely: on
/// timeout we deliberately do not `wait()` (against a D-state child a kill
/// may not take — and `wait()` would hang — which is exactly the bug this
/// exists to prevent) and we do not join the reader thread (its read ends
/// with EOF when the child's pipe finally closes, after which the thread
/// exits on its own).
///
/// Standard error is discarded (the tool's diagnostics are warnings; the JSON
/// we need comes on stdout).
pub fn run_with_timeout(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, GpuRunError> {
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(GpuRunError::Spawn)?;
    let mut pipe = child
        .stdout
        .take()
        .expect("stdout was set to Stdio::piped() above");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    // Deliberately no `wait()` and no `reader.join()`: see
                    // the function docs. `child` is dropped here (the zombie
                    // is reaped by init once the kill takes), and the reader
                    // thread detaches, exiting on EOF.
                    return Err(GpuRunError::TimedOut(timeout));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(GpuRunError::Spawn(e)),
        }
    };

    let stdout = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(GpuRunError::ExitCode(status.code().unwrap_or(-1)));
    }
    Ok(stdout)
}

/// Reads the first card (`card0`) from the live `rocm-smi`. Returns `None`
/// (and logs at debug) when `rocm-smi` is missing, exits non-zero, or reports
/// a document we cannot parse — a normal, expected condition on hosts without
/// an AMD GPU. A `rocm-smi` that wedges past `ROCSMI_TIMEOUT` is killed and
/// skipped the same way. Used both by the per-sample `push_sample` and by the
/// startup probe (`gpu_available`).
pub fn read_gpu_card() -> Option<GpuSample> {
    let mut rocm = std::process::Command::new("rocm-smi");
    rocm.args(["--showuse", "--showmemuse", "--showtemp", "--json"]);
    let stdout = match run_with_timeout(&mut rocm, ROCSMI_TIMEOUT) {
        Ok(stdout) => stdout,
        Err(GpuRunError::Spawn(e)) => {
            // rocm-smi not on PATH is the common case on non-AMD hosts.
            debug!("Can't run rocm-smi: {e:?}");
            return None;
        }
        Err(GpuRunError::ExitCode(code)) => {
            debug!("rocm-smi exited with code {code}");
            return None;
        }
        Err(GpuRunError::TimedOut(timeout)) => {
            warn!("rocm-smi did not finish within {timeout:?} — killed it; skipping this sample");
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&stdout);
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
    use std::time::{Duration, Instant};

    // ---- Bounded subprocess --------------------------------------------------

    /// A child that finishes in time passes its stdout through unchanged.
    #[test]
    fn test_run_with_timeout_success() {
        let mut echo = std::process::Command::new("echo");
        echo.arg("hello-world");
        let out = run_with_timeout(&mut echo, Duration::from_secs(2))
            .expect("echo must finish within the deadline");
        assert_eq!(String::from_utf8_lossy(&out), "hello-world\n");
    }

    /// A non-zero exit is reported with its code (stdout is discarded).
    #[test]
    fn test_run_with_timeout_nonzero_exit() {
        let mut false_cmd = std::process::Command::new("false");
        let err = run_with_timeout(&mut false_cmd, Duration::from_secs(2))
            .expect_err("`false` must be reported as a failure, not a hang");
        assert!(
            matches!(err, GpuRunError::ExitCode(1)),
            "expected ExitCode(1), got {err:?}"
        );
    }

    /// A child that cannot finish in time is killed and reported — and the
    /// call returns promptly, which is the whole point (the caller is the
    /// GTK main loop).
    #[test]
    fn test_run_with_timeout_times_out_bounded() {
        let started = Instant::now();
        let mut sleep_cmd = std::process::Command::new("sleep");
        sleep_cmd.arg("30");
        let err = run_with_timeout(&mut sleep_cmd, Duration::from_millis(300))
            .expect_err("`sleep 30` must not finish in 300 ms");
        let elapsed = started.elapsed();
        assert!(
            matches!(err, GpuRunError::TimedOut(_)),
            "expected TimedOut, got {err:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the timed-out call returned after {elapsed:?}"
        );
    }

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
}
