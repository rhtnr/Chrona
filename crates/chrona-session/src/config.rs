//! TOML-backed app config: the default lift angle, last-used input device,
//! and per-device ppm calibration (spec M3).

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_lift_deg() -> f64 {
    52.0
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigStore {
    #[serde(default = "default_lift_deg")]
    pub default_lift_deg: f64,
    #[serde(default)]
    pub last_device: Option<String>,
    #[serde(default)]
    pub device_ppm: BTreeMap<String, f64>,
}

impl Default for ConfigStore {
    fn default() -> Self {
        ConfigStore {
            default_lift_deg: default_lift_deg(),
            last_device: None,
            device_ppm: BTreeMap::new(),
        }
    }
}

/// `<platform config dir>/chrona/config.toml`, or `None` when the platform
/// has no known config directory (spec M3: config persistence).
fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "chrona")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

impl ConfigStore {
    /// Loads from the platform config dir; a missing file yields defaults.
    pub fn load_default() -> ConfigStore {
        match default_config_path() {
            Some(path) => ConfigStore::load_from(&path),
            None => ConfigStore::default(),
        }
    }

    /// Loads from an explicit path (tests). A missing file, or one that
    /// fails to parse, silently falls back to defaults — never an error,
    /// same "absent/unparseable is not a failure" rule as the sidecar.
    pub fn load_from(path: &Path) -> ConfigStore {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves to the platform config dir (creating it if needed).
    pub fn save(&self) -> Result<()> {
        let path = default_config_path().context("no platform config directory available")?;
        self.save_to(&path)
    }

    /// Saves to an explicit path (tests), creating parent directories.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = ConfigStore::load_from(&path); // missing → defaults
        assert_eq!(c.default_lift_deg, 52.0);
        c.device_ppm.insert("Built-in Microphone".into(), -37.2);
        c.last_device = Some("Built-in Microphone".into());
        c.save_to(&path).unwrap();
        let c2 = ConfigStore::load_from(&path);
        assert_eq!(c2.device_ppm["Built-in Microphone"], -37.2);
        assert_eq!(c2.last_device.as_deref(), Some("Built-in Microphone"));
    }
}
