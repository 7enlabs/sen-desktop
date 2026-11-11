// Local app settings for the CLI bridge (Phase 2, tin 9530 quyết 2).
// PURE RUST, no Tauri dependency: compiles and tests standalone.
//
// What lives here: the MODEL id only. The model is passed to the CLI on
// every turn with the --model flag; when no model is stored, the CLI's own
// free default applies (DEFAULT_MODEL below, the CLI's DEFAULT_MODEL in
// zen-cli settings.rs:6 — a free model, per the free-only rule).
//
// What NEVER lives here: the API key. The key travels only through the
// process environment as ZEN_API_KEY, which the CLI reads itself
// (zen-cli settings.rs:290, then OPENAI_API_KEY, then apiKeyEnv). The
// bridge never reads, stores, logs, or passes the key on argv. There is
// no key field in this struct, no key in the settings file, and no key in
// any log line this module can produce.
use std::path::{Path, PathBuf};

/// The CLI's own free default model (zen-cli settings.rs:6).
pub const DEFAULT_MODEL: &str = "google/gemini-4-argon:free";

/// Settings file name inside the app config dir.
pub const SETTINGS_FILE_NAME: &str = "cli-settings.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliSettings {
    pub model: String,
}

impl Default for CliSettings {
    fn default() -> Self {
        CliSettings {
            model: DEFAULT_MODEL.to_string(),
        }
    }
}

impl CliSettings {
    /// Settings file path for a config dir.
    pub fn file_path(config_dir: &Path) -> PathBuf {
        config_dir.join(SETTINGS_FILE_NAME)
    }

    /// Load settings from a config dir. A missing file, an unreadable
    /// file, or invalid JSON all fall back to the default model: settings
    /// must never fail a session. A stored blank model also falls back.
    pub fn load(config_dir: &Path) -> CliSettings {
        let path = Self::file_path(config_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return CliSettings::default(),
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return CliSettings::default(),
        };
        match value
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            Some(model) => CliSettings {
                model: model.to_string(),
            },
            None => CliSettings::default(),
        }
    }

    /// Validate a model id: non-empty after trimming. The bridge never
    /// invents a model and never falls back to a first-available list.
    pub fn validate_model(model: &str) -> Result<String, String> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Err("Model must be non-empty, FAIL".to_string());
        }
        Ok(trimmed.to_string())
    }

    /// Save settings to a config dir (creating it when needed). Only the
    /// model id is written; the file never contains a key.
    pub fn save(&self, config_dir: &Path) -> Result<(), String> {
        let model = Self::validate_model(&self.model)?;
        if let Err(e) = std::fs::create_dir_all(config_dir) {
            return Err(format!(
                "Cannot create settings dir {}: {}",
                config_dir.display(),
                e
            ));
        }
        let value = serde_json::json!({ "model": model });
        let text = match serde_json::to_string_pretty(&value) {
            Ok(text) => text,
            Err(e) => return Err(format!("Cannot serialize settings: {}", e)),
        };
        std::fs::write(Self::file_path(config_dir), text).map_err(|e| {
            format!(
                "Cannot write settings file {}: {}",
                Self::file_path(config_dir).display(),
                e
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sen_settings_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_model_is_the_cli_free_default() {
        assert_eq!(DEFAULT_MODEL, "google/gemini-4-argon:free");
        assert_eq!(CliSettings::default().model, DEFAULT_MODEL);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tmpdir("missing");
        assert_eq!(CliSettings::load(&dir), CliSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips_and_file_has_no_key() {
        let dir = tmpdir("roundtrip");
        let settings = CliSettings {
            model: "some/model:free".to_string(),
        };
        settings.save(&dir).unwrap();
        assert_eq!(CliSettings::load(&dir), settings);
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(text.contains("some/model:free"));
        assert!(!text.to_lowercase().contains("key"));
        assert!(!text.to_lowercase().contains("token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_corrupt_or_blank_model_returns_default() {
        let dir = tmpdir("corrupt");
        std::fs::write(CliSettings::file_path(&dir), "{not json").unwrap();
        assert_eq!(CliSettings::load(&dir), CliSettings::default());
        std::fs::write(CliSettings::file_path(&dir), "{\"model\":\"  \"}").unwrap();
        assert_eq!(CliSettings::load(&dir), CliSettings::default());
        std::fs::write(CliSettings::file_path(&dir), "{\"other\":1}").unwrap();
        assert_eq!(CliSettings::load(&dir), CliSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_model_refuses_blank() {
        assert!(CliSettings::validate_model("").is_err());
        assert!(CliSettings::validate_model("   ").is_err());
        assert_eq!(
            CliSettings::validate_model("  m:free  ").unwrap(),
            "m:free".to_string()
        );
    }
}
