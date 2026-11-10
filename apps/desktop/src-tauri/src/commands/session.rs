// Real Tauri command handlers for sessions (bridge round).
// Thin wrappers only: they convert wire args and delegate to
// commands/registry.rs, which owns the statechart, the contract check and
// the record. No session logic lives here.
// The backend never calls the Pi SDK and never holds a key. The model
// runs in the backend through the CLI core + stream-json bridge (a later
// phase); until then steer/follow_up are refused by the registry with
// MODEL_NOT_WIRED. A backend record with no model runtime is the
// designed intermediate state in this phase.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

use super::registry::{ActionContract, Registry};

/// Wire contract, camelCase on the wire to match
/// apps/desktop/src/ipc/types.ts exactly. The registry struct uses
/// snake_case internally; this conversion is the only mapping here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractArgs {
    pub id: String,
    pub capability: String,
    pub capability_version: String,
    pub actor: String,
    pub purpose: String,
    pub phase: String,
    pub tool_kind: String,
    pub side_effect: String,
    pub risk: String,
    pub idempotency_key: String,
    pub timeout_ms: u64,
    pub approval_required: bool,
    pub audit_required: bool,
}

impl ContractArgs {
    pub fn to_registry(&self) -> ActionContract {
        ActionContract {
            id: self.id.clone(),
            capability: self.capability.clone(),
            capability_version: self.capability_version.clone(),
            actor: self.actor.clone(),
            purpose: self.purpose.clone(),
            phase: self.phase.clone(),
            tool_kind: self.tool_kind.clone(),
            side_effect: self.side_effect.clone(),
            risk: self.risk.clone(),
            idempotency_key: self.idempotency_key.clone(),
            timeout_ms: self.timeout_ms,
            approval_required: self.approval_required,
            audit_required: self.audit_required,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub session_id: String,
    pub state: String,
}

fn result(session_id: &str, state: super::registry::SessionState) -> SessionResult {
    SessionResult {
        session_id: session_id.to_string(),
        state: state.as_str().to_string(),
    }
}

/// sen_session_create: backend record only, state Running. No SDK call.
#[tauri::command]
pub fn sen_session_create(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .create(&session_id, &contract.to_registry())
        .map_err(|e| e.to_wire())?;
    Ok(result(&session_id, next))
}

/// sen_session_steer: gates on a Running session. The model call runs in
/// the backend once the CLI bridge lands; until then the registry refuses
/// with MODEL_NOT_WIRED and no intent is recorded.
#[tauri::command]
pub fn sen_session_steer(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    text: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .steer(&session_id, &contract.to_registry(), "steer", &text)
        .map_err(|e| e.to_wire())?;
    Ok(result(&session_id, next))
}

/// sen_session_follow_up: same gate as steer; refused with
/// MODEL_NOT_WIRED until the CLI bridge lands.
#[tauri::command]
pub fn sen_session_follow_up(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    text: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .steer(&session_id, &contract.to_registry(), "follow_up", &text)
        .map_err(|e| e.to_wire())?;
    Ok(result(&session_id, next))
}

/// sen_session_abort: Running or AwaitingApproval -> Cancelled, per the
/// point-854 table. Error and terminal states refuse with a typed error.
#[tauri::command]
pub fn sen_session_abort(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .abort(&session_id, &contract.to_registry())
        .map_err(|e| e.to_wire())?;
    Ok(result(&session_id, next))
}
