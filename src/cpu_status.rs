//! Live CPU status readouts: current frequency and temperature, read from the
//! Linux `sysfs` interfaces. Frequency comes from the `cpufreq` driver on core
//! 0; temperature from the CPU's `hwmon` sensor. Both degrade gracefully to
//! `None` when the source is absent (e.g. running in a container without the
//! driver), so the UI can show a blank placeholder instead of failing.

use std::path::{Path, PathBuf};

use log::debug;

/// The `cpufreq` directory of core 0, the representative "current CPU
/// frequency" source (matches what most system monitors show).
const CPU0_CPUFREQ: &str = "/sys/devices/system/cpu/cpu0/cpufreq";

/// `scaling_cur_freq` reports the current frequency in kilohertz.
const CUR_FREQ_KHZ: &str = "scaling_cur_freq";
/// `scaling_max_freq` reports the ceiling the governor can reach, in kHz — the
/// top of the frequency-axis domain for the graph.
const MAX_FREQ_KHZ: &str = "scaling_max_freq";

/// Root of the hardware-monitor class; each `hwmonN` child carries a `name`
/// and one or more `tempN_input` readings in millidegrees.
const HWMON_DIR: &str = "/sys/class/hwmon";

/// Parses a `cpufreq` frequency file containing a single kilohertz value into
/// an integer kHz count. Returns `None` if it isn't a positive integer.
///
/// Pure: no I/O, so it is easy to test against fixed input.
pub fn parse_freq_khz(text: &str) -> Option<u64> {
    // `cpufreq` files are a bare decimal (kHz). Trim the trailing newline that
    // `read_to_string` leaves and any surrounding whitespace before parsing.
    let trimmed = text.trim().parse::<u64>().ok()?;
    if trimmed == 0 {
        return None;
    }
    Some(trimmed)
}

/// Reads a `cpufreq` kHz file at `path` and returns its value in kilohertz.
///
/// Returns `None` (and logs at debug) when the file is missing, unreadable, or
/// malformed — a normal, expected condition on hosts without the driver.
pub fn read_khz_file(path: &Path) -> Option<u64> {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse_freq_khz(&text) {
            Some(khz) => Some(khz),
            None => {
                debug!(
                    "{} is not a positive kHz value: {:?}",
                    path.display(),
                    text.trim()
                );
                None
            }
        },
        Err(e) => {
            debug!("Can't read {}: {e:?}", path.display());
            None
        }
    }
}

/// The current frequency of core 0 in **MHz**, or `None` when the `cpufreq`
/// interface is unavailable.
pub fn read_cpu0_freq_mhz() -> Option<f64> {
    let path = Path::new(CPU0_CPUFREQ).join(CUR_FREQ_KHZ);
    read_khz_file(&path).map(|khz| khz as f64 / 1000.0)
}

/// The maximum frequency of core 0 in **MHz**, or `None` when the `cpufreq`
/// interface is unavailable. Used as the top of the frequency-axis domain.
pub fn read_max_freq_mhz() -> Option<f64> {
    let path = Path::new(CPU0_CPUFREQ).join(MAX_FREQ_KHZ);
    read_khz_file(&path).map(|khz| khz as f64 / 1000.0)
}

/// Formats a frequency in MHz for display, switching to GHz at 1000 and up:
/// `"980 MHz"`, `"3.45 GHz"`.
pub fn format_freq(mhz: f64) -> String {
    if mhz >= 1000.0 {
        format!("{:.2} GHz", mhz / 1000.0)
    } else {
        format!("{:.0} MHz", mhz)
    }
}

/// Returns `true` when `name` identifies a CPU temperature sensor.
///
/// CPU sensors are named after their architecture driver (`coretemp`, `k10temp`,
/// `zenpower`, `cpu`). Non-CPU sensors (GPU, NVMe, motherboard chips) are
/// deliberately excluded so a CPU task manager never reports a disk or GPU
/// temperature.
pub fn is_cpu_sensor(name: &str) -> bool {
    matches!(name, "cpu" | "coretemp" | "k10temp" | "zenpower")
}

/// The hottest reading, in °C, across a `hwmon` sensor's `tempN_input` files
/// (each holds a millidegree value). Returns the maximum, dividing by 1000.
///
/// Returns `None` if no `tempN_input` file parses to a finite value.
///
/// Reads are performed from a snapshot of the directory listing so this is
/// safe against concurrent `hwmon` changes; only the hottest value is kept
/// since that is the value a user cares about (the "CPU is this hot" number).
pub fn hottest_temp_c(sensor_dir: &Path) -> Option<f64> {
    let Ok(entries) = std::fs::read_dir(sensor_dir) else {
        return None;
    };
    let mut hottest: Option<f64> = None;
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        // Match `tempN_input` only — skip `tempN_label`, `tempN_crit`, etc.
        let Some(index) = file_name.strip_prefix("temp") else {
            continue;
        };
        let Some(index) = index.strip_suffix("_input") else {
            continue;
        };
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Some(millideg) = read_f64_file(&entry.path()) {
            // Millidegrees → degrees. Skip non-finite values.
            if millideg.is_finite() {
                hottest = Some(hottest.map_or(millideg / 1000.0, |h| h.max(millideg / 1000.0)));
            }
        }
    }
    hottest
}

/// Reads a signed decimal file into an `f64`, e.g. `temp1_input` (millidegrees).
pub fn read_f64_file(path: &Path) -> Option<f64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
}

/// Scans `base` (an `hwmon` root directory) for the first sensor whose
/// `name` is a CPU sensor, returning the directory that holds its
/// `tempN_input` files.
fn pick_cpu_hwmon(base: &Path) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return None;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        // `hwmon` roots only contain directories named `hwmonN`; skip files.
        let Ok(ft) = dir.symlink_metadata() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        // A sensor whose `name` can't be read is skipped, not fatal: one
        // unreadable sensor must not abort the scan and hide a CPU sensor
        // that comes later (the old `.ok()?` bailed on the first error and
        // discarded the real `k10temp`/`coretemp` sensor behind it, so CPU
        // temperature silently read as `None`).
        let Ok(name) = std::fs::read_to_string(dir.join("name")) else {
            continue;
        };
        if is_cpu_sensor(name.trim()) {
            return Some(dir);
        }
    }
    None
}

/// The current CPU temperature in **°C**, or `None` when no CPU `hwmon`
/// sensor is available (common in VMs and containers).
pub fn read_cpu_temp_c() -> Option<f64> {
    let sensor = pick_cpu_hwmon(Path::new(HWMON_DIR))?;
    hottest_temp_c(&sensor)
}

/// Formats a Celsius value for display: `"72.3 °C"`.
pub fn format_temp(celsius: f64) -> String {
    format!("{:.1} °C", celsius)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_file(tag: &str, contents: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("simpletaskmgr-cpu-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("freq");
        let mut f = std::fs::File::create(&path).expect("create file");
        f.write_all(contents.as_bytes()).expect("write file");
        path
    }

    #[test]
    fn test_parse_freq_khz_happy() {
        assert_eq!(parse_freq_khz("4050149\n"), Some(4050149));
        assert_eq!(parse_freq_khz("1000"), Some(1000));
        assert_eq!(parse_freq_khz("  42  "), Some(42));
    }

    #[test]
    fn test_parse_freq_khz_rejects_invalid() {
        assert_eq!(parse_freq_khz(""), None, "empty file has no value");
        assert_eq!(parse_freq_khz("0"), None, "zero is not a real frequency");
        assert_eq!(parse_freq_khz("4.5"), None, "kHz files are integers");
        assert_eq!(parse_freq_khz("abc"), None, "non-numeric is ignored");
        assert_eq!(parse_freq_khz("-100"), None, "negative is ignored");
    }

    #[test]
    fn test_read_khz_file_returns_value() {
        let p = temp_file("read_ok", "2450000\n");
        assert_eq!(read_khz_file(&p), Some(2450000));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn test_read_khz_file_missing_returns_none() {
        let p = std::env::temp_dir().join("no-such-cpu-freq-file-xyz");
        assert_eq!(read_khz_file(&p), None);
    }

    #[test]
    fn test_read_khz_file_malformed_returns_none() {
        let p = temp_file("read_bad", "not-a-number");
        assert_eq!(read_khz_file(&p), None);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn test_format_freq_units() {
        assert_eq!(format_freq(980.0), "980 MHz");
        assert_eq!(format_freq(1000.0), "1.00 GHz");
        assert_eq!(format_freq(3450.149), "3.45 GHz");
        assert_eq!(format_freq(0.0), "0 MHz");
    }

    /// On a real Linux host the `cpufreq` interface for core 0 is usually
    /// present; when it is, the current frequency must be a positive MHz value.
    /// On a host/container without the driver the read is `None` — both are
    /// acceptable, but a present value must be sane.
    #[test]
    fn test_read_cpu0_freq_mhz_sane_when_present() {
        if let Some(mhz) = read_cpu0_freq_mhz() {
            assert!(mhz > 0.0, "a present frequency must be positive");
            assert!(
                mhz < 1_000_000.0,
                "a frequency in the MHz hundreds of millions is a parsing bug"
            );
        }
    }

    /// Creates a unique empty `hwmon` base directory and returns its path.
    /// Tests build `hwmonN` sensor dirs inside it and remove the base at the
    /// end, so parallel tests never share a tree.
    fn hwmon_base(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "simpletaskmgr-hwmon-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create hwmon base dir");
        base
    }

    /// Creates one `hwmonN` sensor dir under `base` with the given `name` and
    /// `tempN_input` files (`n` -> millidegree value).
    fn mk_sensor(base: &Path, index: u32, name: &str, temps: &[(&str, &str)]) {
        let dir = base.join(format!("hwmon{index}"));
        std::fs::create_dir_all(&dir).expect("create sensor dir");
        std::fs::write(dir.join("name"), name).expect("write name");
        for (n, value) in temps {
            std::fs::write(dir.join(format!("temp{n}_input")), value).expect("write input");
        }
    }

    #[test]
    fn test_is_cpu_sensor_recognizes_cpu_drivers() {
        assert!(is_cpu_sensor("cpu"), "generic cpu sensor");
        assert!(is_cpu_sensor("coretemp"), "Intel coretemp");
        assert!(is_cpu_sensor("k10temp"), "AMD k10temp");
        assert!(is_cpu_sensor("zenpower"), "AMD zenpower");
    }

    #[test]
    fn test_is_cpu_sensor_excludes_non_cpu() {
        assert!(!is_cpu_sensor("nvme"), "disk is not a CPU");
        assert!(!is_cpu_sensor("amdgpu"), "GPU is not a CPU");
        assert!(
            !is_cpu_sensor("pch_thermal"),
            "motherboard chip is not a CPU"
        );
    }

    #[test]
    fn test_hottest_temp_c_picks_max_and_converts_to_celsius() {
        let base = hwmon_base("hottest");
        // Two readings (Tctl 75.4 °C, Tccd1 62.5 °C) in millidegrees.
        mk_sensor(&base, 3, "k10temp", &[("1", "75375"), ("3", "62500")]);
        let dir = base.join("hwmon3");
        assert_eq!(
            hottest_temp_c(&dir),
            Some(75.375),
            "hottest of two inputs, /1000 to °C"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_hottest_temp_c_single_input() {
        let base = hwmon_base("single");
        mk_sensor(&base, 0, "coretemp", &[("1", "38500")]);
        assert_eq!(hottest_temp_c(&base.join("hwmon0")), Some(38.5));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_hottest_temp_c_only_counts_input_files() {
        let base = hwmon_base("inputonly");
        mk_sensor(&base, 1, "k10temp", &[("1", "40000")]);
        // A `temp1_label` and `temp1_crit` must not be mistaken for a reading.
        std::fs::write(base.join("hwmon1/temp1_label"), "CPU").expect("write label");
        std::fs::write(base.join("hwmon1/temp1_crit"), "110000").expect("write crit");
        assert_eq!(
            hottest_temp_c(&base.join("hwmon1")),
            Some(40.0),
            "only tempN_input is the reading"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_hottest_temp_c_missing_dir_returns_none() {
        let p = std::env::temp_dir().join("no-such-hwmon-sensor-xyz");
        assert_eq!(hottest_temp_c(&p), None);
    }

    #[test]
    fn test_pick_cpu_hwmon_selects_cpu_sensor_over_others() {
        let base = hwmon_base("pick");
        // hwmon0 = nvme, hwmon1 = k10temp (CPU), hwmon2 = amdgpu.
        mk_sensor(&base, 0, "nvme", &[]);
        mk_sensor(&base, 1, "k10temp", &[("1", "60000")]);
        mk_sensor(&base, 2, "amdgpu", &[]);

        let picked = pick_cpu_hwmon(&base);
        assert_eq!(
            picked,
            Some(base.join("hwmon1")),
            "must pick the hwmon whose name says CPU"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_pick_cpu_hwmon_none_when_no_cpu_sensor() {
        let base = hwmon_base("none");
        mk_sensor(&base, 0, "nvme", &[]);
        assert_eq!(pick_cpu_hwmon(&base), None, "no CPU sensor found");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_pick_cpu_hwmon_skips_sensor_with_missing_name() {
        let base = hwmon_base("skipmissingname");
        // A sensor whose `name` file is missing (read error) sits *before* the
        // CPU sensor. The scan must skip it and still find the k10temp sensor
        // that follows — the old `.ok()?` aborted the whole scan here.
        let broken = base.join("hwmon0");
        std::fs::create_dir_all(&broken).expect("create broken sensor dir");
        // Note: no `name` file written.
        mk_sensor(&base, 1, "k10temp", &[("1", "60000")]);

        assert_eq!(
            pick_cpu_hwmon(&base),
            Some(base.join("hwmon1")),
            "a sensor with an unreadable name must not hide a later CPU sensor"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_format_temp() {
        assert_eq!(format_temp(72.3456), "72.3 °C");
        assert_eq!(format_temp(75.375), "75.4 °C");
        assert_eq!(format_temp(0.0), "0.0 °C");
    }

    /// On a real Linux host a CPU `hwmon` sensor is usually present; when it
    /// is, the recorded temperature must be a positive and physically-sane
    /// Celsius value. On a host/container without one the read is `None`.
    #[test]
    fn test_read_cpu_temp_c_sane_when_present() {
        if let Some(c) = read_cpu_temp_c() {
            assert!(c > 0.0, "a present CPU temp must be positive");
            assert!(c < 150.0, "a CPU temp above 150 °C indicates a parsing bug");
        }
    }
}
