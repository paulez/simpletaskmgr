use simpletaskmgr::{
    config::RefreshInterval,
    cpu_status::{read_cpu0_freq_mhz, read_cpu_temp_c},
    disk_status::{is_physical_disk, DiskStatus},
    gpu_status::{gpu_available, read_gpu_card, GpuSample},
    metrics::SystemMetrics,
    process::{ProcessItem, TaskMgrProcess},
    process_list::ProcessList,
    settings::UserSettings,
    signal::Signal,
    ui::{KillStatus, State},
};

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // helpers
    // ---------------------------------------------------------------------------

    /// Init the test logger once per test process.
    ///
    /// Defaults to `Warn` so `cargo test` output stays quiet. Set
    /// `SIMPLETASKMGR_TEST_LOG=debug` (case-insensitive, any `debug`-ish value
    /// works) to see the crate's `debug!` logs while developing — the same
    /// knob as the app's `-v` flag, exposed as an env var because `cargo test`
    /// doesn't forward arbitrary flags to the test binary.
    fn init_log() {
        let level = std::env::var("SIMPLETASKMGR_TEST_LOG")
            .map(|v| v.eq_ignore_ascii_case("debug"))
            .unwrap_or(false);
        let filter = if level {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Warn
        };
        let _ = simplelog::SimpleLogger::init(
            filter,
            simplelog::ConfigBuilder::new()
                .add_filter_allow_str("simpletaskmgr")
                .build(),
        );
    }

    fn temp_settings_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "simpletaskmgr-integ-test-{}-{}-settings.toml",
            std::process::id(),
            tag
        ))
    }

    fn item(pid: i32) -> ProcessItem {
        let p = TaskMgrProcess::new(format!("name{pid}"), pid, 1000, "paul".to_string(), 1.0);
        ProcessItem::new(&p)
    }

    /// A live `State` — `ProcessList::init()` walks `/proc/[pid]`. This is
    /// the constructor the original *unit* tests used when they were live;
    /// they moved here to keep the lib test pool hermetic (no `/proc` walk),
    /// per `doc/TEST_FD_LIMIT_FIX_PLAN.md`.
    fn live_state(tag: &str) -> State {
        let path = temp_settings_path(tag);
        let _ = std::fs::remove_file(&path);
        State::with_settings_path(path)
    }

    // ---------------------------------------------------------------------------
    // SystemMetrics::push_sample — live I/O (moved from src/metrics.rs)
    // ---------------------------------------------------------------------------

    /// `push_sample` keeps at most `cap` samples. Live-reading side effects
    /// (`/proc/stat`, `/proc/meminfo`, `/sys/cpufreq`, etc.) run here, so this
    /// must live in the integration binary, not the parallel lib pool.
    #[test]
    fn test_push_sample_caps_history() {
        init_log();
        let mut m = SystemMetrics::with_cap(3);
        for _ in 0..5 {
            m.push_sample(None);
        }
        assert_eq!(m.history().len(), 3);
    }

    /// `push_sample` updates `history`. Same rationale as above.
    #[test]
    fn test_push_sample_updates_history_and_last() {
        init_log();
        let mut m = SystemMetrics::with_cap(10);
        assert!(m.history().is_empty());
        m.push_sample(None);
        assert_eq!(m.history().len(), 1);
    }

    /// `push_sample` records the live core-0 `cpufreq` frequency, when present.
    #[test]
    fn test_push_sample_carries_freq() {
        init_log();
        let mut m = SystemMetrics::with_cap(10);
        m.push_sample(None);
        let last = m.history().pop().unwrap();
        if let Some(mhz) = last.freq {
            assert!(mhz > 0.0, "recorded freq must be positive");
            assert!(mhz < 1_000_000.0, "freq must be a sane MHz value");
        }
        m.push_sample(None);
        let last2 = m.history().pop().unwrap();
        if let Some(mhz) = last2.freq {
            assert!(mhz > 0.0, "second sample's freq must be positive");
            assert!(mhz < 1_000_000.0, "second sample's freq must be sane");
        }
    }

    /// `push_sample` carries the GPU sample when provided, and holds the last
    /// value forward when `None` is passed on a subsequent tick.
    #[test]
    fn test_push_sample_carries_gpu() {
        init_log();
        let mut m = SystemMetrics::with_cap(10);

        m.push_sample(None);
        let last = m.history().pop().unwrap();
        assert_eq!(last.gpu_use, None);
        assert_eq!(last.gpu_vram, None);
        assert_eq!(last.gpu_temp, None);

        m.push_sample(Some(GpuSample {
            use_pct: 42.0,
            vram_pct: 77.0,
            temp_c: 61.0,
        }));
        let last = m.history().pop().unwrap();
        assert_eq!(last.gpu_use, Some(42.0));
        assert_eq!(last.gpu_vram, Some(77.0));
        assert_eq!(last.gpu_temp, Some(61.0));

        m.push_sample(None);
        let last = m.history().pop().unwrap();
        assert_eq!(last.gpu_use, Some(42.0), "carry-over must hold 42.0");
        assert_eq!(last.gpu_vram, Some(77.0));
        assert_eq!(last.gpu_temp, Some(61.0));
    }

    /// `push_sample` records per-disk readings from `/proc/diskstats`.
    #[test]
    fn test_push_sample_carries_disks() {
        init_log();
        let mut m = SystemMetrics::with_cap(10);
        m.push_sample(None);
        let first = m.history().pop().unwrap();
        for d in &first.disks {
            if let Some(b) = d.read_bps {
                assert!(b >= 0.0);
            }
            if let Some(w) = d.write_bps {
                assert!(w >= 0.0);
            }
            if let Some(u) = d.util_pct {
                assert!((0.0..=100.0).contains(&u));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        m.push_sample(None);
        let second = m.history().pop().unwrap();
        assert_eq!(second.disks.len(), first.disks.len(), "same set of disks");
        for d in &second.disks {
            if let Some(b) = d.read_bps {
                assert!(b >= 0.0);
            }
            if let Some(w) = d.write_bps {
                assert!(w >= 0.0);
            }
            if let Some(u) = d.util_pct {
                assert!((0.0..=100.0).contains(&u));
            }
        }
    }

    // ---------------------------------------------------------------------------
    // cpu_status — live /sys reads (moved from src/cpu_status.rs)
    // ---------------------------------------------------------------------------

    /// On a Linux host with the `cpufreq` interface, the core-0 frequency is a
    /// positive, sane MHz value. On a host without it the read is `None`.
    #[test]
    fn test_read_cpu0_freq_mhz_sane_when_present() {
        init_log();
        if let Some(mhz) = read_cpu0_freq_mhz() {
            assert!(mhz > 0.0, "a present frequency must be positive");
            assert!(
                mhz < 1_000_000.0,
                "a MHz value of hundreds of millions is a parsing bug"
            );
        }
    }

    /// On a host with a CPU `hwmon` sensor, the temperature is a positive,
    /// physically-sane Celsius value.
    #[test]
    fn test_read_cpu_temp_c_sane_when_present() {
        init_log();
        if let Some(c) = read_cpu_temp_c() {
            assert!(c > 0.0, "a present CPU temp must be positive");
            assert!(c < 150.0, "a CPU temp above 150 °C indicates a parsing bug");
        }
    }

    // ---------------------------------------------------------------------------
    // disk_status — live /proc/diskstats (moved from src/disk_status.rs)
    // ---------------------------------------------------------------------------

    /// `DiskStatus::snapshot` reports sane per-disk readings from
    /// `/proc/diskstats` (the second sample has a live baseline).
    #[test]
    fn test_snapshot_sane_when_present() {
        init_log();
        let mut ds = DiskStatus::default();
        let first = ds.snapshot();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let second = ds.snapshot();
        assert_eq!(second.len(), first.len(), "same device set both snapshots");
        for s in &second {
            assert!(!s.name.is_empty());
            if let Some(b) = s.read_bps {
                assert!(b >= 0.0);
            }
            if let Some(w) = s.write_bps {
                assert!(w >= 0.0);
            }
            if let Some(u) = s.util_pct {
                assert!((0.0..=100.0).contains(&u));
            }
        }
    }

    /// `snapshot` filters to physical disks only — no partitions, virtual devices.
    #[test]
    fn test_snapshot_only_physical_disks() {
        init_log();
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

    // ---------------------------------------------------------------------------
    // State::refresh / kill / settings — live I/O (moved from src/ui.rs)
    // ---------------------------------------------------------------------------

    // Each of these exercises a distinct `State` behaviour — primes metrics,
    // refresh appends one sample, refresh preserves selection, kill-`sleep`,
    // refresh-drops-killed, settings round-trip — in sequence so the live
    // `/proc` walk is never concurrent with itself in the integration binary.
    // (Splitting into separate `#[test]` functions would let the 16-thread
    // pool run ~6 walks at once, saturating the fd table under low `ulimit -n`.)
    #[test]
    fn test_state_lifecycle() {
        init_log();

        // (a) with_settings_path primes metrics, no selection yet.
        {
            let s = live_state("metrics");
            assert!(!s.metrics.history().is_empty());
            assert!(s.selected_pid.is_none());
        }

        // (b) refresh appends exactly one sample.
        {
            let mut s = live_state("refresh_sample");
            let before = s.metrics.history().len();
            s.refresh();
            let after = s.metrics.history().len();
            assert_eq!(after, before + 1, "one refresh appends exactly one sample");
        }

        // (c) refresh preserves the selection.
        {
            let mut s = live_state("refresh_preserve");
            let me = std::process::id() as i32;
            s.selected_pid = Some(me);
            s.refresh();
            assert_eq!(s.selected_pid, Some(me));
        }

        // (d) kill(SIGKILL) on a live `sleep` child reports Sent.
        {
            let mut s = live_state("kill_kill");
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep");
            let pid = child.id() as i32;
            s.selected_pid = Some(pid);
            match s.kill(Signal::Sigkill) {
                KillStatus::Sent => {}
                other => panic!("expected Sent, got {other:?}"),
            }
            let _ = child.wait();
        }

        // (e) refresh after kill drops the process.
        {
            let mut s = live_state("kill_drops");
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep");
            let pid = child.id() as i32;
            s.process_list.processes.push(item(pid));
            s.selected_pid = Some(pid);
            match s.kill(Signal::Sigkill) {
                KillStatus::Sent => {}
                other => panic!("expected Sent, got {other:?}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            let _ = child.wait();
            s.refresh();
            assert!(
                s.process_list.processes.iter().all(|p| p.pid != pid),
                "killed process must be dropped after refresh"
            );
        }

        // (f) saved settings round-trip and seed the list.
        {
            let path = temp_settings_path("load_all");
            UserSettings {
                show_all: true,
                refresh: RefreshInterval::Slow,
            }
            .save(&path)
            .unwrap();
            let s = State::with_settings_path(path.clone());
            let _ = std::fs::remove_file(&path);
            assert!(s.settings.show_all);
            assert_eq!(s.settings.refresh, RefreshInterval::Slow);
            assert!(
                s.process_list.show_all,
                "show_all setting must seed the process list on startup"
            );
        }

        // (g) `refresh_process_list` (the same routine `State::with_settings_path`
        // and `State::refresh` internally use) returns a non-empty, unique-pid
        // set of rows with each name, pid, and username populated. Consolidated
        // here so it shares the single sequential full-`/proc` walk in this test
        // instead of being a second, concurrent enumerator.
        {
            let mut process_list = ProcessList::new();
            let all = process_list
                .refresh_process_list()
                .expect("Failed to get process list");
            assert!(
                !all.is_empty(),
                "refresh_process_list must return at least one row on a live host"
            );
            let mut pids = std::collections::HashSet::new();
            for p in all.iter() {
                assert!(!p.name.is_empty());
                assert!(p.pid > 0);
                assert!(!p.username.is_empty());
                assert!(pids.insert(p.pid), "Duplicate PID found: {}", p.pid);
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Existing integration tests (unchanged, consolidated to avoid parallel
    // live-walk contention)
    // ---------------------------------------------------------------------------

    // The live `rocm-smi` smoke test — one spawn in the integration binary is
    // safe (only one at a time), unlike in the parallel lib pool.
    #[test]
    fn test_read_gpu_card_sane_when_present() {
        init_log();
        let available = gpu_available();
        if let Some(s) = read_gpu_card() {
            assert!(available, "a `Some` reading implies the GPU is available");
            assert!((0.0..=100.0).contains(&s.use_pct), "use_pct in 0–100");
            assert!((0.0..=100.0).contains(&s.vram_pct), "vram_pct in 0–100");
            assert!(s.temp_c > 0.0, "a present GPU temp must be positive");
            assert!(
                s.temp_c < 120.0,
                "a GPU temp above 120 °C indicates a parsing bug"
            );
        }
    }
}
