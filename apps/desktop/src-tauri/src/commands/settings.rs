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
    let mut settings = CliSettings::load(&dir);
    settings.model = model.clone();
    settings
        .save(&dir)
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
/// URL must not wipe it.
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
    let mut settings = CliSettings::load(&dir);
    settings.base_url = base_url.clone();
    settings
        .save(&dir)
        .map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    // Sessions created from now on use the new base URL.
    if let Ok(mut registry) = state.lock() {
        registry.set_settings_dir(dir);
    }
    Ok(BaseUrlSettingResult { base_url })
}
