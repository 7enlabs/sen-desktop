// Real Tauri command handlers for CLI bridge settings (Phase 2, tin 9530
// quyết 2). Thin wrappers only: they convert wire args and delegate to
// commands/cli_settings.rs, which owns the model value, the base URL,
// and the file. No logic of its own lives here.
//
// The settings hold the MODEL id and, since R2B-05, the optional BASE
// URL of the provider endpoint. There is no key command and no key
// field anywhere: the key travels only through the process environment as
// ZEN_API_KEY, which the CLI reads itself. These commands can never
// return, store, or log a key.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

use super::cli_settings::CliSettings;
use super::model_router::{
    key_env_present, resolve_attempt_order, validate_router_config, ProviderKind, Route,
    RouterConfig,
};
use super::registry::{validate_contract, Registry};
use super::session::ContractArgs;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettingResult {
    pub model: String,
}

/// Resolve the settings dir: the app config dir when available. When it
/// is not available, the default model applies (settings never fail a
/// read).
fn settings_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok()
}

/// sen_settings_get_model: the stored model, or the CLI's free default
/// when nothing is stored.
#[tauri::command]
pub fn sen_settings_get_model(
    app: AppHandle,
    contract: ContractArgs,
) -> Result<ModelSettingResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let model = match settings_dir(&app) {
        Some(dir) => CliSettings::load(&dir).model,
        None => CliSettings::default().model,
    };
    Ok(ModelSettingResult { model })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelArgs {
    pub model: String,
}

/// sen_settings_set_model: store a model id for later turns. The model
/// applies to sessions created after the change; a running session keeps
/// the model it started with. Refuses a blank model. The stored base
/// URL, if any, is preserved: setting the model must not wipe it.
/// The load-change-save runs under `CliSettings::update`'s
/// process-wide lock (P2-6), so a concurrent set_base_url / set_router
/// cannot lose this change, and vice versa.
#[tauri::command]
pub fn sen_settings_set_model(
    app: AppHandle,
    state: State<'_, Mutex<Registry>>,
    args: SetModelArgs,
    contract: ContractArgs,
) -> Result<ModelSettingResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let model =
        CliSettings::validate_model(&args.model).map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    let dir = settings_dir(&app)
        .ok_or_else(|| "INVALID_CONTRACT: app config dir is not available, FAIL".to_string())?;
    let model = CliSettings::update(&dir, |settings| {
        settings.model = model.clone();
        Ok(model.clone())
    })
    .map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    // Sessions created from now on use the new model.
    if let Ok(mut registry) = state.lock() {
        registry.set_settings_dir(dir);
    }
    Ok(ModelSettingResult { model })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseUrlSettingResult {
    pub base_url: Option<String>,
}

/// sen_settings_get_base_url: the stored base URL, or None when nothing
/// is stored (R2B-05). A session created with no stored base URL passes
/// no --base-url flag and the CLI uses its own default endpoint.
#[tauri::command]
pub fn sen_settings_get_base_url(
    app: AppHandle,
    contract: ContractArgs,
) -> Result<BaseUrlSettingResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let base_url = match settings_dir(&app) {
        Some(dir) => CliSettings::load(&dir).base_url,
        None => None,
    };
    Ok(BaseUrlSettingResult { base_url })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBaseUrlArgs {
    pub base_url: String,
}

/// sen_settings_set_base_url: store a provider base URL for later
/// turns (R2B-05). The URL applies to sessions created after the
/// change; a running session keeps the base URL it started with. A
/// blank value clears the stored base URL (later sessions pass no
/// --base-url flag). The stored model is preserved: setting the base
/// URL must not wipe it. The load-change-save runs under
/// `CliSettings::update`'s process-wide lock (P2-6), so a concurrent
/// set_model / set_router cannot lose this change, and vice versa.
#[tauri::command]
pub fn sen_settings_set_base_url(
    app: AppHandle,
    state: State<'_, Mutex<Registry>>,
    args: SetBaseUrlArgs,
    contract: ContractArgs,
) -> Result<BaseUrlSettingResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let base_url = CliSettings::validate_base_url(&args.base_url)
        .map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    let dir = settings_dir(&app)
        .ok_or_else(|| "INVALID_CONTRACT: app config dir is not available, FAIL".to_string())?;
    let base_url = CliSettings::update(&dir, |settings| {
        settings.base_url = base_url.clone();
        Ok(base_url.clone())
    })
    .map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    // Sessions created from now on use the new base URL.
    if let Ok(mut registry) = state.lock() {
        registry.set_settings_dir(dir);
    }
    Ok(BaseUrlSettingResult { base_url })
}

/// P0-3: the model-router settings (routes, preference, fallback,
/// BYOK approval) as the frontend sees them. `byok_key_present` is a
/// boolean only: true when a BYOK route's named env var is set and
/// non-empty. No key value ever travels in either direction.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterSettingResult {
    pub routes: Vec<Route>,
    pub preferred_route: Option<String>,
    pub fallback_enabled: bool,
    pub byok_approved: bool,
    pub byok_key_present: bool,
    pub active_route_id: String,
}

fn router_result(config: &RouterConfig) -> RouterSettingResult {
    let byok_key_present = config.routes.iter().any(|route| {
        route.provider == ProviderKind::Byok
            && route
                .key_env
                .as_ref()
                .map(|env| key_env_present(env))
                .unwrap_or(false)
    });
    let active_route_id = resolve_attempt_order(config)
        .into_iter()
        .next()
        .map(|route| route.id)
        .unwrap_or_else(|| "spark".to_string());
    RouterSettingResult {
        routes: config.routes.clone(),
        preferred_route: config.preferred_route.clone(),
        fallback_enabled: config.fallback_enabled,
        byok_approved: config.byok_approved,
        byok_key_present,
        active_route_id,
    }
}

/// sen_settings_get_router: the stored router, or the default chain
/// when nothing is stored. Never fails for a missing file: the
/// default chain applies.
#[tauri::command]
pub fn sen_settings_get_router(
    app: AppHandle,
    contract: ContractArgs,
) -> Result<RouterSettingResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let config = match settings_dir(&app) {
        Some(dir) => CliSettings::load(&dir).router.unwrap_or_default(),
        None => RouterConfig::default(),
    };
    Ok(router_result(&config))
}

/// Partial router update: only the fields present in `args` change.
/// A supplied `routes` replaces the whole chain. The merged config
/// is validated before anything is stored: a paid route or a BYOK
/// route without approval is refused, never stored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRouterArgs {
    pub preferred_route: Option<Option<String>>,
    pub fallback_enabled: Option<bool>,
    pub byok_approved: Option<bool>,
    pub routes: Option<Vec<Route>>,
}

/// sen_settings_set_router: store a router change for later turns.
/// A running session keeps the route it started with; the new chain
/// applies to sessions created after the change (and to fallback
/// swaps, which read the stored chain). Refuses an invalid merged
/// config. The load-change-save runs under `CliSettings::update`'s
/// process-wide lock (P2-6), so a concurrent set_model /
/// set_base_url cannot lose this change, and vice versa. A refused
/// merged config saves nothing: the stored settings stay untouched.
#[tauri::command]
pub fn sen_settings_set_router(
    app: AppHandle,
    state: State<'_, Mutex<Registry>>,
    args: SetRouterArgs,
    contract: ContractArgs,
) -> Result<RouterSettingResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let dir = settings_dir(&app)
        .ok_or_else(|| "INVALID_CONTRACT: app config dir is not available, FAIL".to_string())?;
    let SetRouterArgs {
        preferred_route,
        fallback_enabled,
        byok_approved,
        routes,
    } = args;
    let config = CliSettings::update(&dir, |settings| {
        let mut config = settings.router.clone().unwrap_or_default();
        if let Some(preferred) = preferred_route {
            config.preferred_route = preferred.filter(|p| !p.trim().is_empty());
        }
        if let Some(fallback) = fallback_enabled {
            config.fallback_enabled = fallback;
        }
        if let Some(approved) = byok_approved {
            config.byok_approved = approved;
        }
        if let Some(routes) = routes {
            config.routes = routes;
        }
        let errors = validate_router_config(&config);
        if !errors.is_empty() {
            // Bare error: the outer map_err adds the single
            // "INVALID_CONTRACT: " prefix, exactly as before.
            return Err(errors.join("; "));
        }
        settings.router = Some(config.clone());
        Ok(config)
    })
    .map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    if let Ok(mut registry) = state.lock() {
        registry.set_settings_dir(dir);
    }
    Ok(router_result(&config))
}
