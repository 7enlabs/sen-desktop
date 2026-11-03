// Real Tauri command handler for tool calls (bridge round).
// Thin wrapper only: it converts wire args and delegates to
// commands/registry.rs, which validates the contract, asserts the 6-step
// gate order, and then fails loudly because the gate execute step and the
// permission layer are not filled in yet. No tool runs for real in this
// phase, and this handler never claims otherwise.
use std::sync::Mutex;
use tauri::State;

use super::registry::Registry;
use super::session::ContractArgs;

/// sen_tool_call: contract + gate order are real checks; execution is not
/// filled in, so the call always ends in TOOL_EXECUTION_NOT_WIRED.
/// A success here would be a lie, so the handler returns the typed error.
#[tauri::command]
pub fn sen_tool_call(
    state: State<'_, Mutex<Registry>>,
    tool_name: String,
    contract: ContractArgs,
) -> Result<(), String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    registry
        .tool_call(&contract.to_registry(), &tool_name)
        .map_err(|e| e.to_wire())
}
