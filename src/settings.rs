//! User settings kept in `$XDG_CONFIG_HOME/ferrochat/settings.json`.
//!
//! Only preferences live here (no secrets, no message data). Unknown or
//! missing fields fall back to the defaults, and a corrupt file is ignored
//! rather than fatal.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Closing the window keeps the app running in the tray (when a tray is
    /// available). On by default, like the official clients.
    pub close_to_tray: bool,
    /// Launch without a window, tray only.
    pub start_minimized: bool,
    /// No desktop notifications at all.
    pub muted: bool,
    /// Look for a new release at start and every few hours.
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            start_minimized: false,
            muted: false,
            check_updates: true,
        }
    }
}

/// File name inside the config directory.
pub const FILE: &str = "settings.json";

impl Settings {
    /// Reads `<dir>/settings.json`; defaults when missing or unreadable.
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes the file atomically (temp file + rename), creating `dir`.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        crate::config::secure_dir(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join(FILE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ferrochat-settings-{name}-{}", std::process::id()))
    }

    #[test]
    fn defaults_close_to_tray_on() {
        let s = Settings::default();
        assert!(s.close_to_tray && !s.start_minimized && !s.muted && s.check_updates);
    }

    #[test]
    fn round_trips() {
        let dir = temp("rt");
        let s = Settings {
            close_to_tray: false,
            start_minimized: true,
            muted: true,
            check_updates: false,
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_partial_and_corrupt_files_use_defaults() {
        let dir = temp("bad");
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), br#"{"muted":true}"#).unwrap();
        let s = Settings::load(&dir);
        assert!(s.muted && s.close_to_tray);
        std::fs::write(dir.join(FILE), b"{ not json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
