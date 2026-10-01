//! GPU status readouts: utilization, VRAM, and temperature.
//!
//! Two interchangeable backends provide the same three metrics for the
//! representative (lowest-numbered AMD) card, consistent with core 0 being
//! the representative CPU:
//!
//! - `rocm-smi --showuse --showmemuse --showtemp --json` — spawned with a
//!   bounded wait and parsed from its JSON stdout (diagnostic warnings are
//!   sent to stderr and intentionally ignored).
//! - the `amdgpu` sysfs interface (`/sys/class/drm/cardN/device`) — plain
//!   stat files and a hwmon temperature probe, no subprocess at all.
//!
//! [`read_gpu_card`] tries the `rocm-smi` backend first and falls back to
//! sysfs when it fails. Every read degrades gracefully to `None` on a host
//! without a readable AMD GPU, so the UI can hide its tab and the sampler
//! can carry the previous reading forward.

use std::path::{Path, PathBuf};

use log::{debug, warn};

/// A single GPU card's status reading. `use_pct` and `vram_pct` are 0–100
/// percentages; `temp_c` is the edge-sensor temperature in °C.
///
/// Fields map one-to-one across backends: `use_pct` ↔ `rocm-smi`
/// "GPU use (%)" / sysfs `gpu_busy_percent`; `vram_pct` ↔ "GPU Memory
/// Allocated (VRAM%)" / `mem_info_vram_used` ÷ `mem_info_vram_total`;
/// `temp_c` ↔ "Temperature (Sensor edge) (C)" / hwmon `temp1_input`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuSample {
    /// GPU utilization percent (0–100).
    pub use_pct: f64,
    /// VRAM allocation percent (0–100).
    pub vram_pct: f64,
    /// GPU edge temperature in °C.
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

/// Reads GPU status from the `amdgpu` `sysfs` interface (no subprocess):
/// `gpu_busy_percent` for utilization, the `mem_info_vram_used/total` pair
/// for VRAM, and a hwmon `temp*_input` probe for the edge temperature.
///
/// Unlike a single-card probe, this walks `/sys/class/drm` in card-number
/// order and reports the *first* card with a complete `amdgpu` reading — so
/// on a hybrid system where the iGPU is `card0`, the discrete AMD card
/// (`card1`/`card2`/…) is found and reported. Returns `None` when the `drm`
/// class is unavailable or no card reads completely (see
/// [`read_sysfs_card`] for the all-or-nothing policy).
pub fn read_gpu_sysfs() -> Option<GpuSample> {
    read_sysfs_drm(Path::new("/sys/class/drm"))
}

/// Scans a `drm` class directory (normally `/sys/class/drm`) in card-number
/// order for the first `cardN` with a complete reading. Non-card entries
/// (`card0-DP-1`, `renderD128`, …) and cards that do not expose the `amdgpu`
/// reading files (e.g. an Intel iGPU without `gpu_busy_percent`) are skipped.
fn read_sysfs_drm(drm_root: &Path) -> Option<GpuSample> {
    let mut cards: Vec<(u32, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(drm_root).ok()?.filter_map(Result::ok) {
        let file_name = entry.file_name();
        let name = match file_name.to_str() {
            Some(n) => n,
            None => continue,
        };
        let num = match name
            .strip_prefix("card")
            .and_then(|s| s.parse::<u32>().ok())
        {
            Some(n) => n,
            None => continue,
        };
        cards.push((num, entry.path().join("device")));
    }
    cards.sort_by_key(|(n, _)| *n);
    for (_, device) in cards {
        if let Some(sample) = read_sysfs_card(&device) {
            return Some(sample);
        }
    }
    None
}

/// Reads one complete sample from a single `cardN/device` directory.
///
/// All three metrics are required: a failed read must not be confused with
/// a real `0%` reading (the same policy as the `rocm-smi` parser), so a card
/// missing any one of the values yields `None` rather than a partial sample.
fn read_sysfs_card(device: &Path) -> Option<GpuSample> {
    let use_pct = read_sysfs_value(device, "gpu_busy_percent")?;
    let vram_used = read_sysfs_value(device, "mem_info_vram_used")?;
    let vram_total = read_sysfs_value(device, "mem_info_vram_total")?;
    if vram_total <= 0.0 {
        // A zero/absent total would make the ratio meaningless (or a div-by-0).
        return None;
    }
    let temp_c = read_card_temp(device)?;
    Some(GpuSample {
        use_pct,
        vram_pct: (vram_used / vram_total) * 100.0,
        temp_c,
    })
}

/// Reads one decimal value file from a directory (a typical sysfs attribute).
fn read_sysfs_value(dir: &Path, file: &str) -> Option<f64> {
    std::fs::read_to_string(dir.join(file))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The `amdgpu` edge sensor is exposed via a hwmon attribute under the
/// device directory: `device/hwmon/hwmonN/` on recent kernels and
/// `device/hwmonN/` on older ones. Tries each hwmon directory in order and
/// returns the first one with a usable temperature.
fn read_card_temp(device: &Path) -> Option<f64> {
    let nested = device.join("hwmon");
    let roots = if nested.is_dir() {
        hwmon_subdirs(&nested)
    } else {
        hwmon_subdirs(device)
    };
    roots.into_iter().find_map(|dir| read_temp_input(&dir))
}

/// The direct subdirectories of `dir` whose names start with `hwmon`.
fn hwmon_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| s.starts_with("hwmon"))
        })
        .map(|e| e.path())
        .collect()
}

/// Reads the first usable temperature from a hwmon directory: prefers
/// `temp1_input` (for `amdgpu`, sensor `temp1` is the GPU edge) and falls
/// back to the first `temp*_input` it finds. Values are millidegrees; the
/// result is degrees Celsius.
fn read_temp_input(hwmon: &Path) -> Option<f64> {
    let to_c = |milli: f64| milli / 1000.0;
    if let Some(milli) = read_sysfs_value(hwmon, "temp1_input") {
        return Some(to_c(milli));
    }
    let Ok(rd) = std::fs::read_dir(hwmon) else {
        return None;
    };
    rd.filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| s.starts_with("temp") && s.ends_with("_input"))
        })
        .find_map(|e| read_sysfs_value(hwmon, e.file_name().to_str().unwrap()).map(to_c))
}

/// Reads the representative GPU sample, trying the `rocm-smi` backend first
/// and falling back to `amdgpu` sysfs (which needs no external binary) when
/// it fails. Returns `None` on hosts without a readable AMD GPU — a normal,
/// expected condition. A `rocm-smi` that wedges past `ROCSMI_TIMEOUT` is
/// killed and falls through to sysfs the same way as any other failure.
/// Used both by the per-sample `push_sample` and by the startup probe
/// (`gpu_available`).
pub fn read_gpu_card() -> Option<GpuSample> {
    match read_rocm_smi() {
        Some(s) => Some(s),
        None => read_gpu_sysfs(),
    }
}

/// The `rocm-smi` backend: spawns `rocm-smi` under the bounded wait, parses
/// its JSON. Returns `None` when it is missing, exits non-zero, times out
/// (it is killed), or produces a document we cannot parse.
fn read_rocm_smi() -> Option<GpuSample> {
    let mut rocm = std::process::Command::new("rocm-smi");
    rocm.args(["--showuse", "--showmemuse", "--showtemp", "--json"]);
    let stdout = match run_with_timeout(&mut rocm, ROCSMI_TIMEOUT) {
        Ok(out) => out,
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
                "GPU use {0:.0}% vram {1:.0}% temp {2:.1} C (rocm-smi)",
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

/// Whether a GPU is available *for us to sample* — i.e. at least one backend
/// (`rocm-smi` or the `amdgpu` sysfs interface) reports a readable card.
/// Called exactly once at startup by the UI: when `false` the UI hides the
/// GPU tab and the sampler skips the per-tick reads entirely.
///
/// This is the same read path as [`read_gpu_card`]; the name is what the UI
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

    // ---- sysfs backend --------------------------------------------------

    /// Builds a `card<name>/device` directory inside a fake `drm` root,
    /// seeded with the given attribute files. Returns the device path.
    fn make_sysfs_card(drm_root: &Path, card: &str, attrs: &[(&str, &str)]) -> PathBuf {
        let device = drm_root.join(card).join("device");
        std::fs::create_dir_all(&device).unwrap();
        for (name, value) in attrs {
            std::fs::write(device.join(name), *value).unwrap();
        }
        device
    }

    /// Seeds a hwmon directory under a device path: either the recent nested
    /// form (`device/hwmon/hwmonN/`) or the older flat form
    /// (`device/hwmonN/`), with an `amdgpu`-style `temp1_input`.
    fn make_hwmon(device: &Path, layout: &str, temp1_mili: &str) {
        let dir = match layout {
            "nested" => device.join("hwmon").join("hwmon2"),
            _ => device.join("hwmon0"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("name"), "amdgpu").unwrap();
        std::fs::write(dir.join("temp1_input"), temp1_mili).unwrap();
    }

    fn full_amdgpu_attrs() -> [(&'static str, &'static str); 3] {
        [
            ("gpu_busy_percent", "60\n"),
            ("mem_info_vram_used", "50000000\n"),
            ("mem_info_vram_total", "250000000\n"),
        ]
    }

    /// A complete card (nested hwmon layout) parses to the expected triple;
    /// the temperature comes back in degrees (millidegrees ÷ 1000).
    #[test]
    fn test_sysfs_full_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let device = make_sysfs_card(tmp.path(), "card0", &full_amdgpu_attrs());
        make_hwmon(&device, "nested", "61000\n");
        let s = read_sysfs_drm(tmp.path()).expect("a complete card must read");
        assert_eq!(
            s,
            GpuSample {
                use_pct: 60.0,
                vram_pct: 20.0,
                temp_c: 61.0,
            }
        );
    }

    /// The older flat hwmon layout (`device/hwmonN/`) is read the same way.
    #[test]
    fn test_sysfs_flat_hwmon_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let device = make_sysfs_card(tmp.path(), "card0", &full_amdgpu_attrs());
        make_hwmon(&device, "flat", "55500\n");
        let s = read_sysfs_drm(tmp.path()).expect("a complete card must read");
        assert_eq!(s.temp_c, 55.5);
    }

    /// A card without the `amdgpu` reading files (e.g. an Intel iGPU at
    /// `card0`) is skipped in favour of the discrete AMD card — the whole
    /// reason for scanning `cardN` instead of hardcoding `card0`.
    #[test]
    fn test_sysfs_skips_non_amd_cards() {
        let tmp = tempfile::tempdir().unwrap();
        make_sysfs_card(
            tmp.path(),
            "card0",
            &[("vendor", "0x8086\n"), ("subsystem", "0x0\n")],
        );
        let device = make_sysfs_card(tmp.path(), "card1", &full_amdgpu_attrs());
        make_hwmon(&device, "nested", "61000\n");
        let s = read_sysfs_drm(tmp.path()).expect("the amdgpu card must be found");
        assert_eq!(s.use_pct, 60.0);
    }

    /// Non-card `drm` entries (`card0-DP-1`, `renderD128`, …) must be
    /// ignored even when sitting among valid cards.
    #[test]
    fn test_sysfs_ignores_non_card_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = tmp.path().join("card0-DP-1");
        std::fs::create_dir_all(&conn).unwrap();
        std::fs::write(conn.join("device"), "junk").unwrap();
        let device = make_sysfs_card(tmp.path(), "card0", &full_amdgpu_attrs());
        make_hwmon(&device, "nested", "61000\n");
        assert!(read_sysfs_drm(tmp.path()).is_some());
    }

    /// A card missing the hwmon temperature is a partial card → `None`, not a
    /// sample with a faked temperature (all-or-nothing policy).
    #[test]
    fn test_sysfs_missing_temp_rejects_sample() {
        let tmp = tempfile::tempdir().unwrap();
        make_sysfs_card(tmp.path(), "card0", &full_amdgpu_attrs());
        assert_eq!(read_sysfs_drm(tmp.path()), None);
    }

    /// A card missing the VRAM files → `None` (a real reading must never be
    /// assembled partly; `use` alone is not a sample).
    #[test]
    fn test_sysfs_missing_vram_rejects_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let device = make_sysfs_card(tmp.path(), "card0", &[("gpu_busy_percent", "60\n")]);
        make_hwmon(&device, "nested", "61000\n");
        assert_eq!(read_sysfs_drm(tmp.path()), None);
    }

    /// A zero VRAM total is meaningless → `None`, no division by zero.
    #[test]
    fn test_sysfs_zero_vram_total_rejects_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let device = make_sysfs_card(
            tmp.path(),
            "card0",
            &[
                ("gpu_busy_percent", "60\n"),
                ("mem_info_vram_used", "50000000\n"),
                ("mem_info_vram_total", "0\n"),
            ],
        );
        make_hwmon(&device, "nested", "61000\n");
        assert_eq!(read_sysfs_drm(tmp.path()), None);
    }

    /// An empty `drm` class (or no `drm` class at all) → `None`.
    #[test]
    fn test_sysfs_empty_or_missing_drm() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_sysfs_drm(tmp.path()), None);
        let missing = tmp.path().join("does-not-exist");
        assert_eq!(read_sysfs_drm(&missing), None);
    }
}
