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
    /// Persisted UI theme, `"dark"` | `"light"` (M4a spec §1); `None` or any
    /// other value falls back to dark — see `chrona_app::theme::theme_from_config`.
    #[serde(default)]
    pub theme: Option<String>,
    /// The user's watch list, freeform names (M4a: watch list). Normalized
    /// (trimmed, deduped, empties dropped) by [`ConfigStore::normalize`]
    /// after every load.
    #[serde(default)]
    pub watches: Vec<String>,
    /// The most recently selected entry from `watches`, or `None`.
    /// `normalize` clears this if it no longer names an entry in `watches`.
    #[serde(default)]
    pub last_watch: Option<String>,
}

impl Default for ConfigStore {
    fn default() -> Self {
        ConfigStore {
            default_lift_deg: default_lift_deg(),
            last_device: None,
            device_ppm: BTreeMap::new(),
            theme: None,
            watches: Vec::new(),
            last_watch: None,
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
        let mut store: ConfigStore = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        store.normalize();
        store
    }

    /// Trims whitespace and drops empty strings from `watches`, dedupes it
    /// (preserving first-occurrence order), and clears `last_watch` if it
    /// no longer names an entry in the normalized list. Called after every
    /// load so a hand-edited config file can't leave `watches`/`last_watch`
    /// in a state the rest of the app has to defend against.
    pub fn normalize(&mut self) {
        let mut deduped: Vec<String> = Vec::with_capacity(self.watches.len());
        for raw in self.watches.drain(..) {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !deduped.iter().any(|w| w == trimmed) {
                deduped.push(trimmed.to_string());
            }
        }
        self.watches = deduped;
        if let Some(lw) = &self.last_watch
            && !self.watches.iter().any(|w| w == lw)
        {
            self.last_watch = None;
        }
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
        assert_eq!(c.theme, None);
        c.device_ppm.insert("Built-in Microphone".into(), -37.2);
        c.last_device = Some("Built-in Microphone".into());
        c.theme = Some("light".into());
        c.save_to(&path).unwrap();
        let c2 = ConfigStore::load_from(&path);
        assert_eq!(c2.device_ppm["Built-in Microphone"], -37.2);
        assert_eq!(c2.theme.as_deref(), Some("light"));
        assert_eq!(c2.last_device.as_deref(), Some("Built-in Microphone"));
    }

    #[test]
    fn normalize_trims_dedupes_and_validates_last_watch() {
        let mut c = ConfigStore {
            watches: vec![
                " a ".to_string(),
                "a".to_string(),
                "".to_string(),
                "b".to_string(),
            ],
            last_watch: Some("zz".to_string()),
            ..Default::default()
        };
        c.normalize();
        assert_eq!(c.watches, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(c.last_watch, None);
    }

    #[test]
    fn watch_fields_roundtrip_through_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = ConfigStore::load_from(&path); // missing → defaults
        assert!(c.watches.is_empty());
        assert_eq!(c.last_watch, None);

        c.watches = vec!["Seiko 5".to_string(), "Omega Seamaster".to_string()];
        c.last_watch = Some("Seiko 5".to_string());
        c.save_to(&path).unwrap();

        let c2 = ConfigStore::load_from(&path);
        assert_eq!(
            c2.watches,
            vec!["Seiko 5".to_string(), "Omega Seamaster".to_string()]
        );
        assert_eq!(c2.last_watch.as_deref(), Some("Seiko 5"));
    }
}
