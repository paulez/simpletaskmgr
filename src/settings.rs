use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::RefreshInterval;

/// Application settings that the user can change and that persist across
/// restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    /// When `true`, the process list shows every process owned by any user.
    pub show_all: bool,
    /// The polling interval used by the refresh timer.
    pub refresh: RefreshInterval,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            show_all: false,
            refresh: RefreshInterval::Normal,
        }
    }
}

/// Returns the path where user settings are stored, by convention
/// `$XDG_CONFIG_HOME/simpletaskmgr/settings.toml`
/// (or `~/.config/simpletaskmgr/settings.toml` when that variable is unset).
pub fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("simpletaskmgr")
        .join("settings.toml")
}

impl UserSettings {
    /// Loads settings from `path`, falling back to [`UserSettings::default`]
    /// when the file is missing, unreadable, or contains invalid TOML.
    /// Never panics: a broken config file must not prevent the app from
    /// starting.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(contents) => match Self::parse(&contents) {
                Ok(settings) => settings,
                Err(err) => {
                    log::warn!(
                        "Could not parse settings from {}: {err:?}; using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) => {
                if path.exists() {
                    log::warn!(
                        "Could not read settings from {}: {err}; using defaults",
                        path.display()
                    );
                }
                Self::default()
            }
        }
    }

    /// Parses a TOML document into settings (falls back to defaults).
    pub fn parse(toml: &str) -> Result<Self> {
        let settings = toml::from_str::<Self>(toml).context("invalid TOML in settings file")?;
        Ok(settings)
    }

    /// Serializes settings to TOML.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("failed to serialize settings")
    }

    /// Writes settings to `path`, creating parent directories as needed.
    /// The write is atomic: the content goes to a temporary file in the same
    /// directory first, which is then renamed into place, so a crash mid-write
    /// cannot leave a truncated settings file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create settings dir {}", parent.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, self.to_toml()?).with_context(|| {
            format!("failed to write temporary settings file {}", tmp.display())
        })?;
        fs::rename(&tmp, path).with_context(|| {
            format!(
                "failed to move settings file into place at {}",
                path.display()
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simpletaskmgr-settings-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_settings_default() {
        let s = UserSettings::default();
        assert!(!s.show_all);
        assert_eq!(s.refresh, RefreshInterval::Normal);
    }

    #[test]
    fn test_load_missing_file_returns_defaults() {
        let dir = temp_dir("missing");
        let path = dir.join("settings.toml");
        let s = UserSettings::load(&path);
        assert_eq!(s, UserSettings::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_load_round_trip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("settings.toml");
        let original = UserSettings {
            show_all: true,
            refresh: RefreshInterval::Slow,
        };
        original.save(&path).expect("save succeeds");
        assert_eq!(UserSettings::load(&path), original);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_corrupt_file_returns_defaults() {
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.toml");
        let mut f = fs::File::create(&path).expect("create file");
        f.write_all(b"{{{ definitely not toml").unwrap();
        assert_eq!(UserSettings::load(&path), UserSettings::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_partial_file_fills_missing_with_defaults() {
        let dir = temp_dir("partial");
        let path = dir.join("settings.toml");
        fs::write(&path, r#"show_all = true"#).unwrap();
        let s = UserSettings::load(&path);
        assert!(s.show_all);
        assert_eq!(
            s.refresh,
            RefreshInterval::Normal,
            "missing key must default"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unknown_key_is_ignored() {
        let s =
            UserSettings::parse("show_all = true\nrefresh = \"normal\"\nunknown = 1\n").unwrap();
        assert!(s.show_all);
        assert_eq!(s.refresh, RefreshInterval::Normal);
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let dir = temp_dir("parent");
        let path = dir.join("nested/deeper/settings.toml");
        UserSettings::default()
            .save(&path)
            .expect("save with nested dirs succeeds");
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_settings_path_is_under_config_dir() {
        let path = settings_path();
        let Some(config_dir) = dirs::config_dir() else {
            return; // no config dir on this platform; nothing to assert
        };
        let path_str = path.to_string_lossy().into_owned();
        let config_str = config_dir.to_string_lossy().into_owned();
        assert!(
            path_str.starts_with(&config_str),
            "settings must live under {}",
            config_str
        );
        assert!(path_str.ends_with("simpletaskmgr/settings.toml"));
    }
}
