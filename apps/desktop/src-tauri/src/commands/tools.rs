// Real Tauri command handlers for tool calls (bridge round).
// Thin wrappers only: they convert wire args and delegate to
// commands/registry.rs, which validates the contract, asserts the 6-step
// gate order, runs the P0-2 approval gate (auto / ask-first / never +
// default-deny policy), and records every evaluation in the append-only
// activity log. No tool runs for real in this phase: when the gate
// passes, sen_tool_call still ends in TOOL_EXECUTION_NOT_WIRED, and
// that is honest — permission granted is not execution.
// sen_tool_approve grants one approval (one tool + one idempotency
// key, by a named granter); sen_tool_revoke removes one approval
// (one tool + one idempotency key, true when a live grant was
// removed); sen_tool_activity_log reads the log back.
// No key, token, or credential travels through any of these commands.
use serde::Deserialize;
use std::sync::Mutex;
use tauri::State;

use super::approval_gate::{ActivityEntry, Approval};
use super::registry::Registry;
use super::session::ContractArgs;

/// sen_tool_call: contract + gate order are real checks, the P0-2
/// approval gate is a real decision, and the evaluation is recorded.
/// A success here would be a lie (execution is not wired yet), so a
/// gate pass still returns the typed TOOL_EXECUTION_NOT_WIRED error.
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveToolArgs {
    pub tool_name: String,
    pub idempotency_key: String,
    pub granted_by: String,
}

/// sen_tool_approve: grant one approval for one tool + one
/// idempotency key. The contract is validated first (a bad contract
/// grants nothing). A grant for a never-tier tool is recorded but
/// never honored: the gate still refuses that tool.
#[tauri::command]
pub fn sen_tool_approve(
    state: State<'_, Mutex<Registry>>,
    contract: ContractArgs,
    args: ApproveToolArgs,
) -> Result<Approval, String> {
    let registry_contract = contract.to_registry();
    super::registry::validate_contract(&registry_contract).map_err(|e| e.to_wire())?;
    if args.tool_name.trim().is_empty()
        || args.idempotency_key.trim().is_empty()
        || args.granted_by.trim().is_empty()
    {
        return Err(
            "INVALID_CONTRACT: approve needs a non-empty tool, idempotency key, and granter, FAIL"
                .to_string(),
        );
    }
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    Ok(registry.approve_tool(&args.tool_name, &args.idempotency_key, &args.granted_by))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeToolArgs {
    pub tool_name: String,
    pub idempotency_key: String,
}

/// sen_tool_revoke: remove one approval (one tool + one
/// idempotency key). The contract is validated first (a bad
/// contract revokes nothing). Returns true when a live grant was
/// removed, false when there was none (already spent, expired, or
/// never granted). Revoking a grant never touches the activity log:
/// the log has no delete path, and the removal itself is not a
/// log entry — the next call with that tool + key is recorded as
/// awaiting-approval like any other unapproved call.
#[tauri::command]
pub fn sen_tool_revoke(
    state: State<'_, Mutex<Registry>>,
    contract: ContractArgs,
    args: RevokeToolArgs,
) -> Result<bool, String> {
    super::registry::validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    if args.tool_name.trim().is_empty() || args.idempotency_key.trim().is_empty() {
        return Err(
            "INVALID_CONTRACT: revoke needs a non-empty tool and idempotency key, FAIL".to_string(),
        );
    }
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    Ok(registry.revoke_tool(&args.tool_name, &args.idempotency_key))
}

/// sen_tool_activity_log: read back the append-only activity log
/// (clones; the log has no delete or update path). The contract is
/// validated first, like every other command.
#[tauri::command]
pub fn sen_tool_activity_log(
    state: State<'_, Mutex<Registry>>,
    contract: ContractArgs,
) -> Result<Vec<ActivityEntry>, String> {
    super::registry::validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let registry = state.lock().map_err(|e| e.to_string())?;
    Ok(registry.activity_log())
}
