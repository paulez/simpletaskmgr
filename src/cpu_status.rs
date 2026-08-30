//! Live CPU status readouts: current frequency and temperature, read from the
//! Linux `sysfs` interfaces. Frequency comes from the `cpufreq` driver on core
//! 0; temperature from the CPU's `hwmon` sensor. Both degrade gracefully to
//! `None` when the source is absent (e.g. running in a container without the
//! driver), so the UI can show a blank placeholder instead of failing.

use std::path::Path;

use log::debug;

/// The `cpufreq` directory of core 0, the representative "current CPU
/// frequency" source (matches what most system monitors show).
const CPU0_CPUFREQ: &str = "/sys/devices/system/cpu/cpu0/cpufreq";

/// `scaling_cur_freq` reports the current frequency in kilohertz.
const CUR_FREQ_KHZ: &str = "scaling_cur_freq";
/// `scaling_max_freq` reports the ceiling the governor can reach, in kHz — the
/// top of the frequency-axis domain for the graph.
const MAX_FREQ_KHZ: &str = "scaling_max_freq";

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
}
