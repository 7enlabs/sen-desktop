// Local app settings for the CLI bridge (Phase 2, tin 9530 quyết 2).
// PURE RUST, no Tauri dependency: compiles and tests standalone.
//
// What lives here: the MODEL id and, since R2B-05, the optional BASE
// URL of the provider endpoint. The model is passed to the CLI on
// every turn with the --model flag; when no model is stored, the CLI's
// own free default applies (DEFAULT_MODEL below, the CLI's
// DEFAULT_MODEL in zen-cli settings.rs:6 — a free model, per the
// free-only rule). The base URL is passed with --base-url when set;
// when it is not set, no --base-url flag is passed and the CLI uses
// its own default endpoint.
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
    /// Optional provider base URL (R2B-05). None means no --base-url
    /// flag: the CLI uses its own default endpoint.
    pub base_url: Option<String>,
}

impl Default for CliSettings {
    fn default() -> Self {
        CliSettings {
            model: DEFAULT_MODEL.to_string(),
            base_url: None,
        }
    }
}

impl CliSettings {
    /// Settings file path for a config dir.
    pub fn file_path(config_dir: &Path) -> PathBuf {
        config_dir.join(SETTINGS_FILE_NAME)
    }

    /// Load settings from a config dir. A missing file, an unreadable
    /// file, or invalid JSON all fall back to the defaults: settings
    /// must never fail a session. A stored blank model also falls back
    /// to the default model. A missing, null, or blank "base_url" loads
    /// as None (R2B-05), so a settings file written before base_url
    /// existed loads normally with no base URL.
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
        let model = value
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let base_url = value
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        CliSettings { model, base_url }
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

    /// Validate a base URL (R2B-05): a blank value means "no base URL"
    /// and validates to None (clearing it), never to an error — the
    /// caller stores None and the bridge then passes no --base-url
    /// flag. A non-blank value is trimmed and kept as-is; the CLI
    /// itself rejects a malformed URL at spawn time.
    pub fn validate_base_url(base_url: &str) -> Result<Option<String>, String> {
        let trimmed = base_url.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_string()))
    }

    /// Save settings to a config dir (creating it when needed). The
    /// model id is always written; "base_url" is written only when
    /// Some, so a file with no base URL stays a plain {"model": ...}
    /// file. The file never contains a key.
    pub fn save(&self, config_dir: &Path) -> Result<(), String> {
        let model = Self::validate_model(&self.model)?;
        let base_url = match &self.base_url {
            Some(url) => Self::validate_base_url(url)?,
            None => None,
        };
        if let Err(e) = std::fs::create_dir_all(config_dir) {
            return Err(format!(
                "Cannot create settings dir {}: {}",
                config_dir.display(),
                e
            ));
        }
        let mut value = serde_json::json!({ "model": model });
        if let Some(url) = base_url {
            value["base_url"] = serde_json::Value::String(url);
        }
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
            base_url: None,
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

    // R2B-05: base_url round-trips through save/load, and the file
    // carries no key.
    #[test]
    fn base_url_round_trips() {
        let dir = tmpdir("baseurl");
        let settings = CliSettings {
            model: "some/model:free".to_string(),
            base_url: Some("https://example.invalid/v1".to_string()),
        };
        settings.save(&dir).unwrap();
        assert_eq!(CliSettings::load(&dir), settings);
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(text.contains("https://example.invalid/v1"));
        assert!(text.contains("base_url"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // R2B-05: a settings file written before base_url existed (no
    // "base_url" key) loads normally with base_url None; a null or
    // blank base_url also loads as None.
    #[test]
    fn load_file_without_base_url_returns_none() {
        let dir = tmpdir("oldfile");
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"some/model:free\"}",
        )
        .unwrap();
        assert_eq!(
            CliSettings::load(&dir),
            CliSettings {
                model: "some/model:free".to_string(),
                base_url: None,
            }
        );
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"some/model:free\",\"base_url\":null}",
        )
        .unwrap();
        assert_eq!(CliSettings::load(&dir).base_url, None);
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"some/model:free\",\"base_url\":\"   \"}",
        )
        .unwrap();
        assert_eq!(CliSettings::load(&dir).base_url, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // R2B-05: validate_base_url trims, and a blank value means "no
    // base URL" (None), never an error.
    #[test]
    fn validate_base_url_trims_and_blank_means_none() {
        assert_eq!(CliSettings::validate_base_url("").unwrap(), None);
        assert_eq!(CliSettings::validate_base_url("   ").unwrap(), None);
        assert_eq!(
            CliSettings::validate_base_url("  https://example.invalid/v1  ").unwrap(),
            Some("https://example.invalid/v1".to_string())
        );
    }

    // R2B-05: saving a blank base_url stores None (no base_url key in
    // the file), so clearing it is a round-trip, not a corrupt file.
    #[test]
    fn save_blank_base_url_stores_none() {
        let dir = tmpdir("blankbase");
        let settings = CliSettings {
            model: "some/model:free".to_string(),
            base_url: Some("   ".to_string()),
        };
        settings.save(&dir).unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.base_url, None);
        assert_eq!(loaded.model, "some/model:free");
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(!text.contains("base_url"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
