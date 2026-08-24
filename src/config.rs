use std::time::Duration;

/// The refresh interval at which the process list and metrics are updated.
///
/// The user can select one of three fixed presets from the settings popover;
/// the choice is persisted and applied at startup.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshInterval {
    /// Refresh every 0.5 seconds
    Fast,
    /// Refresh every 1.5 seconds (the default)
    #[default]
    Normal,
    /// Refresh every 3 seconds
    Slow,
}

impl RefreshInterval {
    /// The duration corresponding to this preset.
    pub fn as_duration(self) -> Duration {
        match self {
            RefreshInterval::Fast => Duration::from_millis(500),
            RefreshInterval::Normal => Duration::from_millis(1500),
            RefreshInterval::Slow => Duration::from_millis(3000),
        }
    }

    /// All presets in display order (fastest first).
    pub const ALL: [Self; 3] = [Self::Fast, Self::Normal, Self::Slow];

    /// The human-readable label shown in the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            RefreshInterval::Fast => "0.5s",
            RefreshInterval::Normal => "1.5s",
            RefreshInterval::Slow => "3s",
        }
    }
}
