use std::time::Duration;

/// Application configuration constants
pub struct Config;

impl Config {
    /// The interval at which the process list should be refreshed (in milliseconds)
    /// This determines how often the UI updates to show new processes and CPU changes
    pub const REFRESH_INTERVAL_MS: u64 = 1500;

    /// The default refresh interval as a Duration for convenience
    pub fn refresh_interval() -> Duration {
        Duration::from_millis(Self::REFRESH_INTERVAL_MS)
    }
}
