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
// (zen-cli settings.rs:290, then the CLI key env var, then apiKeyEnv). The
// bridge never reads, stores, logs, or passes the key on argv. There is
// no key field in this struct, no key in the settings file, and no key in
// any log line this module can produce.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::atomic_file::write_atomic;
use super::model_router::{
    default_routes, validate_router_config, Route, RouterConfig, DEFAULT_BASE_URL,
};

/// The CLI's own free default model (zen-cli settings.rs:6).
pub const DEFAULT_MODEL: &str = "deepseek-v4.1-flash";

/// Settings file name inside the app config dir.
pub const SETTINGS_FILE_NAME: &str = "cli-settings.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliSettings {
    pub model: String,
    /// Optional provider base URL (R2B-05). None means no --base-url
    /// flag: the CLI uses its own default endpoint.
    pub base_url: Option<String>,
    /// Optional model router (Sen P0-3). None means the legacy
    /// single-route behaviour: one implicit "spark" route from the
    /// model/base_url above. Some means the router chain decides the
    /// route for new sessions (preferred route first, then the chain
    /// order); the legacy model/base_url are then only the fallback
    /// when the router has no usable route.
    pub router: Option<RouterConfig>,
}

impl Default for CliSettings {
    fn default() -> Self {
        CliSettings {
            model: DEFAULT_MODEL.to_string(),
            base_url: None,
            router: None,
        }
    }
}

/// Migration for settings files written before the provider-hiding
/// rename (22/11/2026, dev/hide-providers). The old build stored
/// route id "openrouter-free", provider "openrouter-free", model
/// "google/gemini-4-argon:free", base URL "https://openrouter.ai/api/v1",
/// and BYOK route id "byok-openrouter" with model
/// "anthropic/claude-sonnet-4" and key env "OPENROUTER_API_KEY".
///
/// Why normalize here instead of a serde alias on ProviderKind: an
/// alias would only fix the enum variant parse. It cannot rename a
/// route id, preferred_route, a model id, a base URL, or a key env
/// name. One normalization pass on the raw JSON value in load covers
/// all five fields, and load is the only entry point that reads the
/// on-disk file (the Tauri settings handlers all go through it), so
/// every consumer sees migrated values and the next save persists
/// them, permanently upgrading the file. Only exact old values are
/// rewritten; a user's own custom model / base URL / key env is left
/// untouched.
///
/// Without this, an old router fails to parse (unknown provider
/// variant) or fails validation (unknown preferred route), load drops
/// it, and the next save overwrites the file without the router —
/// the stored chain is lost for good. Worse, an old legacy
/// model/base URL would keep running against the old endpoint, and
/// an old BYOK route would keep running with the old model, base
/// URL, and key env.
fn migrate_legacy_settings(value: &mut serde_json::Value) {
    fn migrate_string_field(
        obj: &mut serde_json::Map<String, serde_json::Value>,
        key: &str,
        map: fn(&str) -> Option<&'static str>,
    ) {
        if let Some(serde_json::Value::String(current)) = obj.get_mut(key) {
            if let Some(new) = map(current) {
                *current = new.to_string();
            }
        }
    }
    fn migrate_model(model: &str) -> Option<&'static str> {
        match model {
            "google/gemini-4-argon:free" | "anthropic/claude-sonnet-4" => Some(DEFAULT_MODEL),
            _ => None,
        }
    }
    fn migrate_base_url(url: &str) -> Option<&'static str> {
        match url {
            "https://openrouter.ai/api/v1" => Some(DEFAULT_BASE_URL),
            _ => None,
        }
    }
    fn migrate_route_id(id: &str) -> Option<&'static str> {
        match id {
            "openrouter-free" => Some("free"),
            "byok-openrouter" => Some("byok"),
            _ => None,
        }
    }
    fn migrate_provider(provider: &str) -> Option<&'static str> {
        match provider {
            "openrouter-free" => Some("free"),
            _ => None,
        }
    }
    fn migrate_key_env(env: &str) -> Option<&'static str> {
        match env {
            "OPENROUTER_API_KEY" => Some("MODEL_API_KEY"),
            _ => None,
        }
    }

    let Some(obj) = value.as_object_mut() else {
        return;
    };
    migrate_string_field(obj, "model", migrate_model);
    migrate_string_field(obj, "base_url", migrate_base_url);
    let Some(router) = obj.get_mut("router").and_then(|v| v.as_object_mut()) else {
        return;
    };
    migrate_string_field(router, "preferredRoute", migrate_route_id);
    if let Some(routes) = router.get_mut("routes").and_then(|v| v.as_array_mut()) {
        for route in routes {
            let Some(route) = route.as_object_mut() else {
                continue;
            };
            migrate_string_field(route, "id", migrate_route_id);
            migrate_string_field(route, "provider", migrate_provider);
            migrate_string_field(route, "model", migrate_model);
            migrate_string_field(route, "baseUrl", migrate_base_url);
            migrate_string_field(route, "keyEnv", migrate_key_env);
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
        let mut value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return CliSettings::default(),
        };
        // Provider-hiding migration (22/11/2026): rewrite exact old
        // stored values to the new ones before parsing, so a file
        // written by the old build loads as the new chain instead of
        // being dropped. See migrate_legacy_settings.
        migrate_legacy_settings(&mut value);
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
        // P0-3: an invalid "router" object loads as None (legacy
        // single-route behaviour) instead of failing the session: a
        // bad router file must never fail session creation. A valid
        // router object is kept as-is.
        let router = value.get("router").and_then(|v| {
            let parsed: Result<RouterConfig, _> = serde_json::from_value(v.clone());
            match parsed {
                Ok(config) if validate_router_config(&config).is_empty() => Some(config),
                _ => None,
            }
        });
        CliSettings {
            model,
            base_url,
            router,
        }
    }

    /// The active route for a new session (P0-3): the first usable
    /// route of the stored router chain, or the legacy implicit
    /// "spark" route (stored model/base_url) when no router is stored
    /// or none of its routes is usable. Never fails: there is always
    /// a route, so session creation never blocks on router state.
    pub fn active_route(&self) -> Route {
        if let Some(config) = &self.router {
            if let Some(route) = super::model_router::select_route(config) {
                return route;
            }
        }
        Route {
            id: "spark".to_string(),
            provider: super::model_router::ProviderKind::Spark,
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            paid: false,
            byok_approved: false,
            key_env: None,
        }
    }

    /// The full default router chain (P0-3), for settings screens
    /// that list routes before the user stored anything.
    pub fn default_router_routes() -> Vec<Route> {
        default_routes()
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
    /// file. The file never contains a key. The write is atomic (P2-6):
    /// temp file in the same dir, fsync, rename — a crash mid-write
    /// leaves the old file or the new file whole, never a torn file
    /// that would load as the default and lose the stored router.
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
        // P0-3: the router object is written only when stored, so a
        // file without a router stays a plain {"model": ...} file.
        // The file never contains a key: a route carries only the env
        // var NAME (key_env), never a value.
        if let Some(router) = &self.router {
            if let Ok(router_value) = serde_json::to_value(router) {
                value["router"] = router_value;
            }
        }
        let text = match serde_json::to_string_pretty(&value) {
            Ok(text) => text,
            Err(e) => return Err(format!("Cannot serialize settings: {}", e)),
        };
        write_atomic(&Self::file_path(config_dir), text.as_bytes()).map_err(|e| {
            format!(
                "Cannot write settings file {}: {}",
                Self::file_path(config_dir).display(),
                e
            )
        })
    }
}

/// One process-wide lock for the settings file (P2-6). The three
/// setters each used to load, change one field, and save the whole
/// file with no lock: two concurrent setters could interleave (both
/// load the old file, each saves its own field) and the second save
/// silently wiped the first setter's change. Every read-modify-write
/// goes through `CliSettings::update`, which holds this lock for the
/// whole load-change-save sequence. Plain loads need no lock: the
/// atomic write means a load always sees a whole old or new file.
static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

impl CliSettings {
    /// Load the settings, apply `change`, and save — atomically with
    /// respect to other `update` calls in this process. When `change`
    /// returns Err, nothing is saved: a refused change leaves the file
    /// untouched. The lock is recovered when poisoned (a thread that
    /// panicked mid-update) instead of bricking settings forever.
    pub fn update<T>(
        config_dir: &Path,
        change: impl FnOnce(&mut CliSettings) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = SETTINGS_WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut settings = Self::load(config_dir);
        let out = change(&mut settings)?;
        settings.save(config_dir)?;
        Ok(out)
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
        assert_eq!(DEFAULT_MODEL, "deepseek-v4.1-flash");
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
            router: None,
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
            router: None,
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
                router: None,
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
            router: None,
        };
        settings.save(&dir).unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.base_url, None);
        assert_eq!(loaded.model, "some/model:free");
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(!text.contains("base_url"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // P0-3: a stored router round-trips; the file carries the env var
    // NAME (key_env) but never a key value.
    #[test]
    fn router_round_trips_and_file_has_no_key_value() {
        let dir = tmpdir("router");
        let config = RouterConfig {
            preferred_route: Some("ollama".to_string()),
            ..Default::default()
        };
        let settings = CliSettings {
            model: "some/model:free".to_string(),
            base_url: None,
            router: Some(config),
        };
        settings.save(&dir).unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded, settings);
        assert_eq!(loaded.active_route().id, "ollama");
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(text.contains("\"router\""));
        assert!(!text.to_lowercase().contains("api_key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // P0-3: an old file without "router" loads with router None, and
    // the active route is the legacy implicit spark route.
    #[test]
    fn load_file_without_router_uses_legacy_route() {
        let dir = tmpdir("norouter");
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"some/model:free\"}",
        )
        .unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.router, None);
        let route = loaded.active_route();
        assert_eq!(route.id, "spark");
        assert_eq!(route.model, "some/model:free");
        assert_eq!(route.cost_label(), "0 đồng");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // P0-3: an invalid stored router loads as None instead of failing
    // the session; the legacy route applies.
    #[test]
    fn load_invalid_router_falls_back_to_legacy() {
        let dir = tmpdir("badrouter");
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"some/model:free\",\"router\":{\"routes\":[]}}",
        )
        .unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.router, None);
        assert_eq!(loaded.active_route().id, "spark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Provider-hiding migration (22/11/2026): a file written by the
    // old build — route id + provider "openrouter-free", model
    // "google/gemini-4-argon:free", base URL "https://openrouter.ai/api/v1",
    // preferredRoute "openrouter-free" — loads as the new chain
    // instead of being dropped, and the next save keeps the router:
    // the stored chain must not be lost on first save.
    #[test]
    fn load_old_openrouter_file_migrates_to_free_route() {
        let dir = tmpdir("migrate-free");
        std::fs::write(
            CliSettings::file_path(&dir),
            r#"{
                "model": "google/gemini-4-argon:free",
                "base_url": "https://openrouter.ai/api/v1",
                "router": {
                    "routes": [
                        {"id": "spark", "provider": "spark", "model": "google/gemini-4-argon:free"},
                        {"id": "openrouter-free", "provider": "openrouter-free", "model": "google/gemini-4-argon:free", "baseUrl": "https://openrouter.ai/api/v1"},
                        {"id": "ollama", "provider": "ollama", "model": "llama3.1", "baseUrl": "http://127.0.0.1:11434/v1"}
                    ],
                    "preferredRoute": "openrouter-free",
                    "fallbackEnabled": true,
                    "byokApproved": false
                }
            }"#,
        )
        .unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.model, "deepseek-v4.1-flash");
        assert_eq!(
            loaded.base_url.as_deref(),
            Some("https://api.experientiallabs.ai/api/v1")
        );
        let router = loaded
            .router
            .clone()
            .expect("old router must survive migration, not be dropped");
        assert_eq!(router.preferred_route.as_deref(), Some("free"));
        assert_eq!(router.routes[1].id, "free");
        assert_eq!(router.routes[1].provider.as_str(), "free");
        assert_eq!(router.routes[1].model, "deepseek-v4.1-flash");
        assert_eq!(
            router.routes[1].base_url.as_deref(),
            Some("https://api.experientiallabs.ai/api/v1")
        );
        assert_eq!(loaded.active_route().id, "free");
        assert_eq!(loaded.active_route().model, "deepseek-v4.1-flash");
        // Saving the migrated settings keeps the router and writes
        // only new values: the old file is upgraded on disk.
        loaded.save(&dir).unwrap();
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(
            !text.contains("openrouter"),
            "migrated file must not keep old ids: {text}"
        );
        assert!(
            !text.contains("google/gemini-4-argon"),
            "migrated file must not keep old model: {text}"
        );
        let reloaded = CliSettings::load(&dir);
        assert_eq!(reloaded, loaded);
        assert!(reloaded.router.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Provider-hiding migration (22/11/2026): an old BYOK route
    // (id "byok-openrouter", model "anthropic/claude-sonnet-4",
    // base URL openrouter.ai, key env OPENROUTER_API_KEY) loads as
    // the new BYOK route — without migration it parsed and validated
    // fine and would keep running against the old provider.
    #[test]
    fn load_old_byok_route_migrates_to_new_model_and_key_env() {
        let dir = tmpdir("migrate-byok");
        std::fs::write(
            CliSettings::file_path(&dir),
            r#"{
                "model": "deepseek-v4.1-flash",
                "router": {
                    "routes": [
                        {"id": "spark", "provider": "spark", "model": "deepseek-v4.1-flash"},
                        {"id": "byok-openrouter", "provider": "byok", "model": "anthropic/claude-sonnet-4", "baseUrl": "https://openrouter.ai/api/v1", "byokApproved": true, "keyEnv": "OPENROUTER_API_KEY"}
                    ],
                    "fallbackEnabled": true,
                    "byokApproved": true
                }
            }"#,
        )
        .unwrap();
        let loaded = CliSettings::load(&dir);
        let router = loaded
            .router
            .clone()
            .expect("old BYOK router must survive migration");
        assert!(validate_router_config(&router).is_empty());
        let byok = router
            .routes
            .iter()
            .find(|r| r.id == "byok")
            .expect("byok-openrouter must migrate to id byok");
        assert_eq!(byok.model, "deepseek-v4.1-flash");
        assert_eq!(
            byok.base_url.as_deref(),
            Some("https://api.experientiallabs.ai/api/v1")
        );
        assert_eq!(byok.key_env.as_deref(), Some("MODEL_API_KEY"));
        // The migrated BYOK route is usable when its key is present
        // (pure check, no env var touched).
        assert!(crate::commands::model_router::is_route_usable(
            byok, &router, true
        ));
        assert!(!crate::commands::model_router::is_route_usable(
            byok, &router, false
        ));
        // Saving writes only new values.
        loaded.save(&dir).unwrap();
        let text = std::fs::read_to_string(CliSettings::file_path(&dir)).unwrap();
        assert!(!text.contains("openrouter"), "migrated file: {text}");
        assert!(
            !text.contains("anthropic/claude-sonnet-4"),
            "migrated file: {text}"
        );
        assert!(
            !text.contains("OPENROUTER_API_KEY"),
            "migrated file: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Provider-hiding migration (22/11/2026): only exact old values
    // are rewritten — a user's own custom model / base URL loads
    // untouched.
    #[test]
    fn migration_leaves_custom_model_and_base_url_untouched() {
        let dir = tmpdir("migrate-custom");
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"my/custom-model\",\"base_url\":\"https://custom.example/v1\"}",
        )
        .unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.model, "my/custom-model");
        assert_eq!(
            loaded.base_url.as_deref(),
            Some("https://custom.example/v1")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // P2-6: two threads updating different fields at once must not
    // lose each other's change. Without the update lock, both threads
    // load the same old file and the second save wipes the first
    // thread's field; with it, the final file carries both threads'
    // last values, deterministically.
    #[test]
    fn concurrent_updates_to_different_fields_keep_both() {
        use std::sync::{Arc, Barrier};
        let dir = tmpdir("race");
        CliSettings::default().save(&dir).unwrap();
        let dir = Arc::new(dir);
        let barrier = Arc::new(Barrier::new(2));
        let rounds = 100;
        let d1 = dir.clone();
        let b1 = barrier.clone();
        let t1 = std::thread::spawn(move || {
            b1.wait();
            for i in 0..rounds {
                CliSettings::update(&d1, |settings| {
                    settings.model = format!("model-{}", i);
                    Ok(())
                })
                .unwrap();
            }
        });
        let d2 = dir.clone();
        let b2 = barrier.clone();
        let t2 = std::thread::spawn(move || {
            b2.wait();
            for i in 0..rounds {
                CliSettings::update(&d2, |settings| {
                    settings.base_url = Some(format!("https://example.invalid/{}", i));
                    Ok(())
                })
                .unwrap();
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
        let loaded = CliSettings::load(&dir);
        assert_eq!(loaded.model, format!("model-{}", rounds - 1));
        assert_eq!(
            loaded.base_url,
            Some(format!("https://example.invalid/{}", rounds - 1))
        );
        let _ = std::fs::remove_dir_all(dir.as_path());
    }

    // P2-6: a refused change saves nothing — the file stays exactly
    // as it was, so a failed validation can never wipe stored settings.
    #[test]
    fn update_leaves_file_untouched_when_the_change_is_refused() {
        let dir = tmpdir("updaterefuse");
        let before = CliSettings {
            model: "keep/me:free".to_string(),
            base_url: Some("https://example.invalid/v1".to_string()),
            router: None,
        };
        before.save(&dir).unwrap();
        let err = CliSettings::update(&dir, |_| Err::<(), String>("nope".to_string())).unwrap_err();
        assert!(err.contains("nope"));
        assert_eq!(CliSettings::load(&dir), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
