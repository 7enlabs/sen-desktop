// Real Tauri command handlers for sessions (CLI bridge round).
// Thin wrappers only: they convert wire args and delegate to
// commands/registry.rs, which owns the statechart, the contract check and
// the record, and to commands/cli_spawn.rs, which owns the CLI turn.
// No session logic lives here.
// The backend never calls the Pi SDK and never holds a key. The model
// runs in the backend through the CLI core + stream-json bridge: steer /
// follow_up run the `zen` CLI for the session (resolved per decision 1,
// model from local settings per decision 2), and CLI events flow to the
// frontend as Tauri events (SEN_SESSION_EVENT) while the turn runs.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

use super::registry::{ActionContract, Registry};
use super::stream_json::SenEvent;

/// Tauri event carrying one Sen event to the frontend while a turn runs.
/// The frontend listens with `listenSessionEvents` (see
/// apps/desktop/src/ipc/session-events.ts) and filters by sessionId.
pub const SEN_SESSION_EVENT: &str = "sen-session-event";

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

/// sen_session_create: backend record + one CLI bridge handle for the
/// session, state Running. No model call happens here.
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

/// Run one turn for a session: validate + take the bridge (step 1), run
/// the CLI turn with NO registry lock held so abort can reach the child
/// (step 2), then record the turn (step 3). Events stream to the frontend
/// through the `emit` callback while the turn runs; a failed emit never
/// fails the turn (the event is already recorded in the turn result).
/// When step 1 reports Duplicate (BUG-10, #25: this exact steer already
/// completed on the session), the recorded result is returned at once:
/// no model call runs, no log entry is pushed, no event is emitted.
/// When step 1 reports InFlight (BUG-10 extra, tin 9645/9650: the same
/// steer is already running on the session), the steer is refused with
/// DUPLICATE_STEER: there is no recorded result to replay yet, so no
/// second model call starts. When step 2 fails, the in-flight key is
/// released before the error returns, so a retry with the same key
/// still executes.
fn run_steer_turn(
    app: &AppHandle,
    state: State<'_, Mutex<Registry>>,
    session_id: &str,
    text: &str,
    kind: &str,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    let start = {
        let mut registry = state.lock().map_err(|e| e.to_string())?;
        registry
            .begin_steer(session_id, &contract.to_registry(), kind, text)
            .map_err(|e| e.to_wire())?
    };
    let bridge = match start {
        super::registry::SteerStart::Duplicate { state, .. } => {
            return Ok(result(session_id, state));
        }
        super::registry::SteerStart::InFlight { .. } => {
            return Err(super::registry::CommandError::new(
                super::registry::ERR_DUPLICATE_STEER,
                "Steer already running on this session, refused",
            )
            .to_wire());
        }
        super::registry::SteerStart::Run(bridge) => bridge,
    };
    // BUG-11: the contract timeout_ms is the turn deadline, capped at
    // the 120s turn timeout inside the bridge (min(timeout_ms, 120s)
    // per tin 9566). Capture it before the turn runs lock-free below.
    let timeout = Duration::from_millis(contract.timeout_ms);
    let app_emit = app.clone();
    let mut emit = |event: SenEvent| {
        // Best effort: a listener that is gone must not fail the turn.
        let _ = app_emit.emit(SEN_SESSION_EVENT, &event);
    };
    let turn = match bridge.run_turn_with_timeout(kind, text, timeout, &mut emit) {
        Ok(turn) => turn,
        Err(e) => {
            // The turn never ran to a result: release the in-flight key
            // (BUG-10 extra) so a retry with the same key still
            // executes. A lock failure here must not mask the turn error.
            if let Ok(mut registry) = state.lock() {
                registry.release_steer(session_id, &contract.to_registry(), kind, text);
            }
            return Err(format!("{}: {}", e.code, e.message));
        }
    };
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .complete_steer(session_id, &contract.to_registry(), kind, text, &turn)
        .map_err(|e| e.to_wire())?;
    Ok(result(session_id, next))
}

/// sen_session_steer: gates on a Running session, then runs one CLI turn
/// for it. The backend ack means the model call ran to a result record;
/// a failure surfaces as a typed error and nothing is recorded. A result
/// record with is_error=true (R2B-02) is also not recorded as a completed
/// steer: the error is visible in the event stream, and a retry with the
/// same key runs the model again instead of replaying the error.
#[tauri::command]
pub fn sen_session_steer(
    app: AppHandle,
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    text: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    run_steer_turn(&app, state, &session_id, &text, "steer", contract)
}

/// sen_session_follow_up: same gate and turn as steer, kind follow_up
/// (later turns resume the CLI session with -r).
#[tauri::command]
pub fn sen_session_follow_up(
    app: AppHandle,
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    text: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    run_steer_turn(&app, state, &session_id, &text, "follow_up", contract)
}

/// sen_session_abort: escalates the running CLI turn first (SIGINT, 5s
/// grace, SIGKILL), then Running or AwaitingApproval -> Cancelled, per
/// the point-854 table. Error and terminal states refuse with a typed
/// error.
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
