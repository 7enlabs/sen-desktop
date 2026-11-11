// Real Tauri command handlers for CLI bridge settings (Phase 2, tin 9530
// quyết 2). Thin wrappers only: they convert wire args and delegate to
// commands/cli_settings.rs, which owns the model value and the file.
// No logic of its own lives here.
//
// The settings hold the MODEL id only. There is no key command and no key
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
/// the model it started with. Refuses a blank model.
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
    CliSettings {
        model: model.clone(),
    }
    .save(&dir)
    .map_err(|e| format!("INVALID_CONTRACT: {}", e))?;
    // Sessions created from now on use the new model.
    if let Ok(mut registry) = state.lock() {
        registry.set_settings_dir(dir);
    }
    Ok(ModelSettingResult { model })
}
