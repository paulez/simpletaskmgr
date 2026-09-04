//! Per-physical-disk I/O throughput and utilization, derived from
//! `/proc/diskstats`.
//!
//! `/proc/diskstats` reports *cumulative* per-block-device counters — sectors
//! read, sectors written, and the milliseconds the device has spent handling
//! I/O. None of them is a rate. To show what a user cares about ("how fast is
//! this disk moving data *right now*, and how saturated is it"), we take the
//! delta between two snapshots and divide by the elapsed time — the same
//! "delta since the last read" principle as [`crate::io_tracker::IoTracker`]
//! (which does it per `pid`, this does it per block-device name) and
//! [`crate::cpu_tracker::CpuTracker`].
//!
//! This reads the kernel interface directly, so — unlike `iostat` — it has no
//! dependency on the `sysstat` package being installed. Only *physical* disks
//! (NVMe, SCSI, IDE) are reported; partitions and virtual devices (loop, ram,
//! zram, md, dm, sr, …) are filtered out.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;

use log::debug;

/// The byte size of the sector unit `/proc/diskstats` reports sectors in.
/// The kernel counts I/O in 512-byte sectors regardless of the device's native
/// sector size, so the byte conversion is always `sectors * 512`.
const SECTOR_BYTES: u64 = 512;

/// One physical disk's I/O reading for the interval since the previous sample.
/// The `Option`s are `None` on the very first sample for a device (a rate
/// needs two samples) and whenever a counter regressed (device reset /
/// re-numbering) — in which case the whole row is *unknown* rather than a
/// spurious zero, so the UI can show a blank instead of a fake `0`.
#[derive(Clone, Debug, PartialEq)]
pub struct DiskSample {
    /// The block-device name (e.g. `nvme0n1`, `sda`).
    pub name: String,
    /// Read throughput in bytes/second, or `None` when not measurable.
    pub read_bps: Option<f64>,
    /// Write throughput in bytes/second, or `None` when not measurable.
    pub write_bps: Option<f64>,
    /// Device utilization: the percentage of the interval the device spent
    /// handling I/O (0–100), or `None` when not measurable.
    pub util_pct: Option<f64>,
}

/// The cumulative `/proc/diskstats` counters we track, captured once for a
/// device. Monotonic per device, which is what makes interval math possible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Counters {
    /// Sectors read (512-byte units) since boot.
    read_sectors: u64,
    /// Sectors written (512-byte units) since boot.
    write_sectors: u64,
    /// Milliseconds the device has spent handling I/O, since boot.
    io_ms: u64,
}

/// Returns `true` when `name` is a physical disk we should report — an NVMe,
/// SCSI, or IDE *whole* disk — and `false` for partitions and virtual devices.
///
/// The kernel names partitions a couple of different ways:
/// * SCSI / IDE / USB (`sda`, `hda`, `vda`, …): partition `N` is `<disk><N>`
///   (`sda1`, `sda2`, …). A whole disk therefore never ends in a digit.
/// * NVMe / MMC (`nvme0n1`, `mmcblk0`, …): partition `N` is `<device>p<N>`
///   (`nvme0n1p1`, `mmcblk0p2`, …). A whole disk has no `p` segment.
pub fn is_physical_disk(name: &str) -> bool {
    // Virtual / pseudo devices — never report these.
    const EXCLUDE_PREFIX: [&str; 9] = [
        "loop", "ram", "zram", "dm-", "md", "sr", "nbd", "fd", "ubiblock",
    ];
    if EXCLUDE_PREFIX.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    // NVMe / MMC whole disks mark partitions with a `p<N>` segment; a bare
    // `<device>` (no `p`) is the whole disk.
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        return !name.contains('p');
    }
    // SCSI / IDE / USB whole disks mark partitions with a trailing digit
    // (`sda1`); a whole disk never ends in a digit, so this is unambiguous.
    !name.bytes().last().is_some_and(|b| b.is_ascii_digit())
}

/// Parses one `/proc/diskstats` line into a device name and the counters we
/// track.
///
/// Field layout (space-separated, 0-based): `major(0) minor(1) name(2)
/// reads(3) reads_merged(4) sectors_read(5) ms_reading(6) writes(7)
/// writes_merged(8) sectors_written(9) ms_writing(10) io_in_flight(11)
/// ms_spent_io(12) …`. We keep only `name`, `sectors_read`,
/// `sectors_written`, and `ms_spent_io`.
///
/// Returns `None` for a line with too few fields or any unparseable counter
/// (a truncated or malformed line must not yield a bogus zero).
///
/// Pure: no I/O, so it is easy to test against fixed input.
pub fn parse_line(line: &str) -> Option<(String, Counters)> {
    let mut f = line.split_whitespace();
    f.next()?; // major
    f.next()?; // minor
    let name = f.next()?.to_string();
    f.next()?; // reads completed
    f.next()?; // reads merged
    let read_sectors: u64 = f.next()?.parse().ok()?;
    f.next()?; // ms spent reading
    f.next()?; // writes completed
    f.next()?; // writes merged
    let write_sectors: u64 = f.next()?.parse().ok()?;
    f.next()?; // ms spent writing
    f.next()?; // I/Os in flight
    let io_ms: u64 = f.next()?.parse().ok()?;
    Some((
        name,
        Counters {
            read_sectors,
            write_sectors,
            io_ms,
        },
    ))
}

/// Parses a whole `/proc/diskstats` document: one `(name, counters)` pair per
/// well-formed line, in file order.
///
/// Malformed lines are skipped (so a truncated read on one device never hides
/// the devices that follow it).
pub fn parse_diskstats(text: &str) -> Vec<(String, Counters)> {
    text.lines().filter_map(parse_line).collect()
}

/// Computes the per-interval `(read_bps, write_bps, util_pct)` between two
/// `Counters` snapshots elapsed `dt` seconds apart:
///
/// * `read_bps`  = `Δ read_sectors * 512 / dt`
/// * `write_bps` = `Δ write_sectors * 512 / dt`
/// * `util_pct`  = `100 * Δ io_ms / (dt * 1000)`, clamped to `0–100`
///
/// A counter that *decreased* (device reset / re-numbering) or a non-positive
/// `dt` make the interval unmeasurable, so all three come back `None` — never
/// a borrow-checked underflow, never a division by ≤ 0, and never a spurious
/// `0` for a reading we did not actually see.
fn calculate_rates(
    cur: &Counters,
    last: &Counters,
    dt: f64,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    if dt <= 0.0 {
        return (None, None, None);
    }
    // A counter that regressed (device reset / re-numbering) makes the whole
    // interval unknown, so all three are `None` — never a borrow-checked
    // underflow.
    let read_delta = cur.read_sectors.checked_sub(last.read_sectors);
    let write_delta = cur.write_sectors.checked_sub(last.write_sectors);
    let io_delta = cur.io_ms.checked_sub(last.io_ms);
    let (Some(read_delta), Some(write_delta), Some(io_delta)) = (read_delta, write_delta, io_delta)
    else {
        return (None, None, None);
    };
    let read_bps = read_delta as f64 * SECTOR_BYTES as f64 / dt;
    let write_bps = write_delta as f64 * SECTOR_BYTES as f64 / dt;
    let util_pct = (100.0 * io_delta as f64 / (dt * 1000.0)).clamp(0.0, 100.0);
    (Some(read_bps), Some(write_bps), Some(util_pct))
}

/// Tracks per-device [`Counters`] baselines so each sampling pass can report
/// the interval's read/write throughput and utilization, the same delta
/// technique as [`crate::io_tracker::IoTracker`].
///
/// Sampling is driven by [`DiskStatus::snapshot`], which reads the live
/// `/proc/diskstats` and records the counters; [`DiskStatus::refresh`] is the
/// pure, testable core that turns one `Vec<(name, counters)>` into samples.
#[derive(Clone)]
pub struct DiskStatus {
    /// Last-seen counters and the clock time they were recorded, per device
    /// name.
    baselines: HashMap<String, (Counters, f64)>,
    /// Monotonic clock reference for the elapsed-time math (immune to
    /// wall-clock steps).
    start: Instant,
}

impl Default for DiskStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskStatus {
    /// Creates a tracker with no baselines and a monotonic clock anchored to
    /// now; the first `refresh`/`snapshot` establishes each device's baseline.
    pub fn new() -> Self {
        Self {
            baselines: HashMap::new(),
            start: Instant::now(),
        }
    }
    /// Records `stats`' counters (taken at monotonic time `now`) and returns
    /// this interval's per-disk samples, most recently measured first.
    ///
    /// A device seen for the first time records a baseline and reports
    /// `None` rates (a rate needs two samples). A device whose counters
    /// regressed reports `None` rates and refreshes its baseline. A device
    /// that has disappeared is dropped from the baselines so it does not
    /// linger and re-report later.
    pub fn refresh(&mut self, stats: &[(String, Counters)], now: f64) -> Vec<DiskSample> {
        let mut samples = Vec::with_capacity(stats.len());
        let mut seen = HashSet::with_capacity(stats.len());
        for (name, cur) in stats {
            seen.insert(name.clone());
            match self.baselines.entry(name.clone()) {
                Entry::Occupied(mut occ) => {
                    let (last, last_ts) = *occ.get();
                    let (read_bps, write_bps, util_pct) =
                        calculate_rates(cur, &last, now - last_ts);
                    samples.push(DiskSample {
                        name: name.clone(),
                        read_bps,
                        write_bps,
                        util_pct,
                    });
                    *occ.get_mut() = (*cur, now);
                }
                Entry::Vacant(vac) => {
                    samples.push(DiskSample {
                        name: name.clone(),
                        read_bps: None,
                        write_bps: None,
                        util_pct: None,
                    });
                    vac.insert((*cur, now));
                }
            }
        }
        // Drop baselines for devices that are no longer present.
        self.baselines.retain(|name, _| seen.contains(name));
        samples
    }

    /// Reads the live `/proc/diskstats` and returns this interval's per-disk
    /// samples (only physical disks; virtual devices and partitions filtered
    /// out), updating the stored baselines along the way.
    ///
    /// Returns an empty `Vec` (and logs at debug) if the file is unreadable
    /// — a normal, expected condition on some minimal containers.
    pub fn snapshot(&mut self) -> Vec<DiskSample> {
        let now = self.start.elapsed().as_secs_f64();
        let text = match std::fs::read_to_string("/proc/diskstats") {
            Ok(t) => t,
            Err(e) => {
                debug!("Can't read /proc/diskstats: {e:?}");
                return Vec::new();
            }
        };
        let stats = parse_diskstats(&text)
            .into_iter()
            .filter(|(name, _)| is_physical_disk(name))
            .collect::<Vec<_>>();
        self.refresh(&stats, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn counters(read: u64, write: u64, io: u64) -> Counters {
        Counters {
            read_sectors: read,
            write_sectors: write,
            io_ms: io,
        }
    }

    // ---- is_physical_disk ---------------------------------------------

    /// Accepts physical whole disks and rejects partitions and virtual devices.
    #[rstest]
    #[case::nvme_disk("nvme0n1", true)]
    #[case::nvme_disk_2("nvme1n1", true)]
    #[case::nvme_partition("nvme0n1p1", false)]
    #[case::nvme_partition_2("nvme1n1p2", false)]
    #[case::nvme_partition_10("nvme0n1p10", false)]
    #[case::mmcblk_disk("mmcblk0", true)]
    #[case::mmcblk_partition("mmcblk0p1", false)]
    #[case::sda("sda", true)]
    #[case::sdb("sdb", true)]
    #[case::sda_partition("sda1", false)]
    #[case::sda_partition_2("sda2", false)]
    #[case::vda("vda", true)]
    #[case::vda_partition("vda2", false)]
    #[case::hda("hda", true)]
    #[case::hda_partition("hda3", false)]
    #[case::loop_dev("loop0", false)]
    #[case::ram("ram0", false)]
    #[case::zram("zram0", false)]
    #[case::dm("dm-0", false)]
    #[case::md("md127", false)]
    #[case::sr("sr0", false)]
    #[case::nbd("nbd0", false)]
    #[case::fd("fd0", false)]
    fn test_is_physical_disk(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(is_physical_disk(name), expected, "{name}");
    }

    // ---- parse_line / parse_diskstats ----------------------------------

    /// A representative 20-field NVMe `/proc/diskstats` line parses to the
    /// device name + the three counters we track, ignoring the rest.
    #[test]
    fn test_parse_line_nvme() {
        let line = "259       0 nvme0n1 792542 13932 83995404 498540 3443944 673115 152139339 6295286 0 898360 6825271 0 0 0 26244 31445 0";
        let (name, c) = parse_line(line).expect("parses");
        assert_eq!(name, "nvme0n1");
        assert_eq!(c.read_sectors, 83995404);
        assert_eq!(c.write_sectors, 152139339);
        assert_eq!(c.io_ms, 898360);
    }

    /// A 16-field line (older kernels without the discard/flush fields) still
    /// parses — we only require the first 13 fields.
    #[test]
    fn test_parse_line_sixteen_fields() {
        let line = "8 0 sda 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16";
        let (name, c) = parse_line(line).expect("parses");
        assert_eq!(name, "sda");
        assert_eq!(c.read_sectors, 3);
        assert_eq!(c.write_sectors, 7);
        assert_eq!(c.io_ms, 10);
    }

    /// A too-short, empty, whitespace-only, or unparseable line is `None` — a
    /// truncated read must not yield a bogus zero for the counters.
    #[rstest]
    #[case::too_few("259 0 sda 100 0 1234")]
    #[case::empty("")]
    #[case::whitespace("   ")]
    #[case::bad_counter("259 0 sda 1 2 notanumber 6 7 8 9 10 11 12")]
    fn test_parse_line_malformed(#[case] line: &str) {
        assert_eq!(parse_line(line), None, "line {line:?}");
    }

    /// `parse_diskstats` keeps one pair per well-formed line, in order.
    #[test]
    fn test_parse_diskstats_keeps_all_lines() {
        let text = "259 0 nvme0n1 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18\n\
                   8 0 sda 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18\n\
                   7 0 loop0 9 9 9 9 9 9 9 9 9 9 9\n";
        let got = parse_diskstats(text);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].0, "nvme0n1");
        assert_eq!(got[1].0, "sda");
        assert_eq!(got[2].0, "loop0");
    }

    /// `parse_diskstats` skips a malformed line in the middle without dropping
    /// the well-formed lines that follow it.
    #[test]
    fn test_parse_diskstats_skips_bad_line() {
        let text = "8 0 sda 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18\n\
                   short line\n\
                   259 0 nvme0n1 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18\n";
        let got = parse_diskstats(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "sda");
        assert_eq!(got[1].0, "nvme0n1");
    }

    // ---- calculate_rates --------------------------------------------------

    /// Steady I/O: sector-deltas / elapsed → bytes/s, and io-ms-delta /
    /// elapsed → percent.
    #[test]
    fn test_calculate_rates_steady() {
        let last = counters(1000, 2000, 100);
        // 1.0 s later: read +1000 sectors (=512000 B) → 512000 B/s;
        // write +4000 sectors (=2048000 B) → 2048000 B/s;
        // io +500 ms over 1000 ms → 50%.
        let cur = counters(2000, 6000, 600);
        assert_eq!(
            calculate_rates(&cur, &last, 1.0),
            (Some(512000.0), Some(2048000.0), Some(50.0))
        );
    }

    /// An idle interval (no counter movement) is `Some(0, 0, 0)` — a real
    /// "nothing moved" reading, not unknown.
    #[test]
    fn test_calculate_rates_idle_is_zero() {
        let c = counters(1000, 2000, 100);
        assert_eq!(
            calculate_rates(&c, &c, 1.0),
            (Some(0.0), Some(0.0), Some(0.0))
        );
    }

    /// A counter that decreased (device reset / re-numbering) makes the whole
    /// interval unknown — all three `None`, never a borrow-checked underflow.
    #[rstest]
    #[case::read_regress(counters(1000, 2000, 100), counters(2000, 2000, 100))]
    #[case::write_regress(counters(1000, 1000, 100), counters(1000, 2000, 100))]
    #[case::io_regress(counters(1000, 2000, 50), counters(1000, 2000, 100))]
    fn test_calculate_rates_regress(#[case] cur: Counters, #[case] last: Counters) {
        assert_eq!(
            calculate_rates(&cur, &last, 1.0),
            (None, None, None),
            "a regressed counter must make the whole interval unknown"
        );
    }

    /// A non-positive elapsed time is unmeasurable (a divide-by-≤0 guard).
    #[rstest]
    #[case::zero(0.0)]
    #[case::negative(-1.0)]
    fn test_calculate_rates_non_positive_dt(#[case] dt: f64) {
        assert_eq!(
            calculate_rates(&counters(2, 4, 8), &counters(1, 2, 4), dt),
            (None, None, None)
        );
    }

    /// Utilization is clamped to 0–100 even when the raw math overshoots
    /// (a single-interval `Δ io_ms` can exceed the wall interval on
    /// multi-queue devices).
    #[test]
    fn test_calculate_rates_clamps_util() {
        let last = counters(0, 0, 0);
        let cur = counters(100, 200, 5000);
        assert_eq!(
            calculate_rates(&cur, &last, 1.0),
            (Some(51200.0), Some(102400.0), Some(100.0))
        );
    }

    // ---- DiskStatus.refresh (the delta core) ----------------------------

    /// A device seen for the first time records a baseline and reports unknown
    /// rates (a rate needs two samples).
    #[test]
    fn test_refresh_first_sample_unknown() {
        let mut ds = DiskStatus::default();
        let stats = vec![("nvme0n1".to_string(), counters(100, 200, 10))];
        let s = ds.refresh(&stats, 0.0);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "nvme0n1");
        assert!(s[0].read_bps.is_none());
        assert!(s[0].write_bps.is_none());
        assert!(s[0].util_pct.is_none());
    }

    /// A second sample measures against the first: sector deltas / elapsed →
    /// bytes/s, io-ms delta / elapsed → percent.
    #[test]
    fn test_refresh_second_sample_measures() {
        let mut ds = DiskStatus::default();
        let first = vec![("nvme0n1".to_string(), counters(1000, 2000, 100))];
        ds.refresh(&first, 0.0);
        // 1.0 s later: read 1000→2000 sectors (+512000 B → 512000 B/s);
        // write 2000→6000 sectors (+2048000 B → 2048000 B/s); io 100→600 ms
        // (Δ 500 / 1000 → 50%).
        let second = vec![("nvme0n1".to_string(), counters(2000, 6000, 600))];
        let s = ds.refresh(&second, 1.0);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].read_bps, Some(512000.0));
        assert_eq!(s[0].write_bps, Some(2048000.0));
        assert_eq!(s[0].util_pct, Some(50.0));
    }

    /// A device that disappears between samples is dropped from the baselines
    /// (a hot-unplug or re-number must not linger and re-report later).
    #[test]
    fn test_refresh_drops_disappeared_device() {
        let mut ds = DiskStatus::default();
        ds.refresh(&[("nvme0n1".to_string(), counters(1, 1, 1))], 0.0);
        assert_eq!(ds.baselines.len(), 1);
        ds.refresh(&[], 1.0);
        assert_eq!(
            ds.baselines.len(),
            0,
            "a vanished device drops its baseline"
        );
    }

    /// Each of several devices reports against its *own* baseline — a delta
    /// never bleeds across devices.
    #[test]
    fn test_refresh_multiple_devices_independent() {
        let mut ds = DiskStatus::default();
        ds.refresh(
            &[
                ("nvme0n1".to_string(), counters(0, 0, 0)),
                ("sda".to_string(), counters(10, 20, 5)),
            ],
            0.0,
        );
        let s = ds.refresh(
            &[
                ("nvme0n1".to_string(), counters(10, 20, 50)),
                ("sda".to_string(), counters(10, 20, 5)),
            ],
            1.0,
        );
        let nvme = s
            .iter()
            .find(|x| x.name == "nvme0n1")
            .expect("nvme0n1 present");
        assert_eq!(nvme.read_bps, Some(5120.0), "nvme read +10 sectors * 512");
        assert_eq!(
            nvme.write_bps,
            Some(10240.0),
            "nvme write +20 sectors * 512"
        );
        assert_eq!(nvme.util_pct, Some(5.0), "nvme io +50 ms over 1 s → 5%");
        let sda = s.iter().find(|x| x.name == "sda").expect("sda present");
        assert_eq!(sda.read_bps, Some(0.0), "sda idle → 0, not nvme's delta");
        assert_eq!(sda.write_bps, Some(0.0));
        assert_eq!(sda.util_pct, Some(0.0));
    }

    // ---- Live I/O (smoke test) ------------------------------------------

    /// On a real Linux host `/proc/diskstats` is present: the second snapshot
    /// (which now has a baseline) yields sane readings for whatever physical
    /// disks exist — throughput ≥ 0, utilization in 0–100. On a host/container
    /// with no physical disk the snapshot is empty — both are acceptable.
    #[test]
    fn test_snapshot_sane_when_present() {
        let mut ds = DiskStatus::default();
        let first = ds.snapshot();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = ds.snapshot();
        assert_eq!(second.len(), first.len(), "same set of devices both times");
        for s in &second {
            assert!(!s.name.is_empty());
            if let Some(b) = s.read_bps {
                assert!(b >= 0.0);
            }
            if let Some(w) = s.write_bps {
                assert!(w >= 0.0);
            }
            if let Some(u) = s.util_pct {
                assert!((0.0..=100.0).contains(&u), "util in 0–100");
            }
        }
    }

    /// `snapshot` always filters to physical disks — it must never report a
    /// partition or a virtual device, whatever the host happens to expose.
    #[test]
    fn test_snapshot_only_physical_disks() {
        let mut ds = DiskStatus::default();
        let _ = ds.snapshot();
        for s in ds.snapshot() {
            assert!(
                is_physical_disk(&s.name),
                "snapshot must only report physical disks, got {}",
                s.name
            );
        }
    }
}
