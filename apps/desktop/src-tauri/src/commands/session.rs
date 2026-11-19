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

use super::memory_store::MemoryStore;
use super::registry::{ActionContract, Registry};
use super::stream_json::SenEvent;

/// Tauri event carrying one Sen event to the frontend while a turn runs.
/// The frontend listens with `listenSessionEvents` (see
/// apps/desktop/src/ipc/session-events.ts) and filters by sessionId.
pub const SEN_SESSION_EVENT: &str = "sen-session-event";

/// Wire contract, camelCase on the wire to match
/// apps/desktop/src/ipc/types.ts exactly. The registry struct uses
/// snake_case internally; this conversion is the only mapping here.
/// session_id has a serde default ("" when an older frontend sends
/// a contract without it): the tool path rejects an empty session
/// id before any gate evaluation, so an old payload fails closed
/// instead of failing to deserialize.
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
    #[serde(default)]
    pub session_id: String,
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
            session_id: self.session_id.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub session_id: String,
    pub state: String,
    /// Unknown records seen in the last turn (R2B-04): 0 when no turn
    /// ran (create/abort), the turn's count after steer/follow_up, and
    /// the recorded turn's count on a Duplicate replay.
    pub unknown_records: u64,
    /// Version mismatches seen in the last turn (R2B-04, same rule as
    /// unknown_records; a record with no `v` field counts as one).
    pub version_mismatches: u64,
    /// P0-3: the model-router route that ran the last turn (or the
    /// session's route when no turn ran), so the frontend can show
    /// the provider/model actually used and its cost.
    pub route_id: String,
    /// P0-3: cost label of that route ("0 đồng" for free/local).
    pub cost: String,
}

fn result(
    session_id: &str,
    state: super::registry::SessionState,
    route_id: &str,
    cost: &str,
) -> SessionResult {
    SessionResult {
        session_id: session_id.to_string(),
        state: state.as_str().to_string(),
        unknown_records: 0,
        version_mismatches: 0,
        route_id: route_id.to_string(),
        cost: cost.to_string(),
    }
}

fn result_with_counts(
    session_id: &str,
    state: super::registry::SessionState,
    turn: &super::cli_spawn::TurnResult,
    route_id: &str,
    cost: &str,
) -> SessionResult {
    SessionResult {
        session_id: session_id.to_string(),
        state: state.as_str().to_string(),
        unknown_records: turn.unknown_records,
        version_mismatches: turn.version_mismatches,
        route_id: route_id.to_string(),
        cost: cost.to_string(),
    }
}

/// Cost label for a session's current route (P0-3). Takes the
/// already-locked registry: the caller holds the lock, so this never
/// locks again. The cost comes from the router chain, falling back
/// to "0 đồng" when the route is unknown (all built-in routes except
/// BYOK cost 0, and a BYOK route reports its own label when found
/// in the chain).
fn route_cost(registry: &super::registry::Registry, session_id: &str) -> (String, String) {
    let route_id = registry
        .route_id(session_id)
        .unwrap_or_else(|| "spark".to_string());
    let cost = registry
        .attempt_order()
        .into_iter()
        .find(|r| r.id == route_id)
        .map(|r| r.cost_label().to_string())
        .unwrap_or_else(|| "0 đồng".to_string());
    (route_id, cost)
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
    let (route_id, cost) = route_cost(&registry, &session_id);
    Ok(result(&session_id, next, &route_id, &cost))
}

/// Run one turn for a session: validate + take the bridge (step 1), run
/// the CLI turn with NO registry lock held so abort can reach the child
/// (step 2), then record the turn (step 3). Events stream to the frontend
/// through the `emit` callback while the turn runs; a failed emit never
/// fails the turn (the event is already recorded in the turn result).
/// When step 1 reports Duplicate (BUG-10, #25: this exact steer already
/// completed on the session), the recorded result is returned at once
/// and the cached result event is replayed (CI P2-2), so a caller
/// waiting for that event resolves instead of timing out: no model
/// call runs, no log entry is pushed.
/// When step 1 reports InFlight (BUG-10 extra, tin 9645/9650: the same
/// steer is already running on the session), the steer is refused with
/// DUPLICATE_STEER: there is no recorded result to replay yet, so no
/// second model call starts. When step 2 fails, the in-flight key is
/// released before the error returns, so a retry with the same key
/// still executes.
///
/// Memory (P0-1): the text the bridge runs is the memory context block +
/// the user's text (MemoryStore::turn_prompt), never the bare text:
/// the model always sees the current memory, including a `quên` that
/// just deleted a fact. The idempotency key still uses the bare user
/// text (begin_steer / complete_steer take `text`), so a memory edit
/// between two identical steers replays the first turn instead of
/// running a second one with the new memory — the replay rule is
/// unchanged. When no memory store is configured, the bare text runs.
fn run_steer_turn(
    app: &AppHandle,
    state: State<'_, Mutex<Registry>>,
    memory: State<'_, Mutex<Option<MemoryStore>>>,
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
        super::registry::SteerStart::Duplicate {
            state: session_state,
            turn,
        } => {
            // R2B-04: the recorded turn carries its counts, so a
            // replayed steer reports the same numbers as the turn
            // that originally ran.
            // CI P2-2: replay the cached result event before
            // returning the ack. The caller (chat.ts) arms its waiter
            // before invoking, so without this event it waits the full
            // timeout and reports "Sen không trả lời được" even though
            // the steer already completed. Best effort, like the live
            // path: a gone listener must not fail the replay. No model
            // call runs, no log entry is pushed, and the idempotency
            // record is untouched.
            let _ = app.emit(SEN_SESSION_EVENT, &turn.result_event());
            let (route_id, cost) = match state.lock() {
                Ok(registry) => route_cost(&registry, session_id),
                Err(_) => ("spark".to_string(), "0 đồng".to_string()),
            };
            return Ok(result_with_counts(
                session_id,
                session_state,
                &turn,
                &route_id,
                &cost,
            ));
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
    // P0-1: build the turn prompt (memory context block + user text)
    // before the turn runs. The memory lock is held only for the
    // prompt build, never across the turn itself.
    let prompt = {
        let guard = memory.lock().map_err(|e| e.to_string())?;
        match guard.as_ref() {
            Some(store) => store.turn_prompt(text),
            None => text.to_string(),
        }
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
    // P0-3 fallback: a turn that fails with a retryable code runs
    // again on the next route in the chain (same loop as the
    // test-only Registry::steer). A non-retryable failure returns at
    // once. The in-flight key is released only when no route is left
    // or the failure is not retryable, so a retry with the same key
    // still executes. P2-2: at most MAX_ATTEMPTS attempts per turn.
    let mut bridge = bridge;
    // P2-2: snapshot the attempt order once, at turn start, and count
    // attempts (the first run is attempt 1). The order used to be
    // re-read from disk on every retry, so a config change mid-turn
    // could drop the failed route out of the chain and restart the
    // loop at the first route forever; there was no counter to stop
    // it. The turn now walks the snapshot only and stops after
    // MAX_ATTEMPTS attempts even if the chain would keep yielding
    // routes.
    let (mut failed_route, order) = match state.lock() {
        Ok(registry) => (
            registry
                .route_id(session_id)
                .unwrap_or_else(|| "spark".to_string()),
            registry.attempt_order(),
        ),
        Err(_) => ("spark".to_string(), Vec::new()),
    };
    let mut attempts = 1;
    let turn = loop {
        match bridge.run_turn_with_timeout(kind, &prompt, timeout, &mut emit) {
            Ok(turn) => break turn,
            Err(e) => {
                if !super::model_router::is_retryable_error_code(&e.code) {
                    if let Ok(mut registry) = state.lock() {
                        registry.release_steer(session_id, &contract.to_registry(), kind, text);
                    }
                    return Err(format!("{}: {}", e.code, e.message));
                }
                if attempts >= super::model_router::MAX_ATTEMPTS {
                    if let Ok(mut registry) = state.lock() {
                        registry.release_steer(session_id, &contract.to_registry(), kind, text);
                    }
                    return Err(format!("{}: {}", e.code, e.message));
                }
                attempts += 1;
                let swapped = match state.lock() {
                    Ok(mut registry) => registry.fallback_bridge(session_id, &failed_route, &order),
                    Err(_) => None,
                };
                match swapped {
                    Some((next_bridge, next_route)) => {
                        failed_route = next_route.id.clone();
                        bridge = next_bridge;
                    }
                    None => {
                        if let Ok(mut registry) = state.lock() {
                            registry.release_steer(session_id, &contract.to_registry(), kind, text);
                        }
                        return Err(format!("{}: {}", e.code, e.message));
                    }
                }
            }
        }
    };
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .complete_steer(session_id, &contract.to_registry(), kind, text, &turn)
        .map_err(|e| e.to_wire())?;
    // R2B-04: the turn's unknown/version counts travel to the frontend
    // with the session result instead of staying inside TurnResult.
    // P0-3: the route that actually ran travels with them.
    let (route_id, cost) = route_cost(&registry, session_id);
    Ok(result_with_counts(
        session_id, next, &turn, &route_id, &cost,
    ))
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
    memory: State<'_, Mutex<Option<MemoryStore>>>,
    session_id: String,
    text: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    run_steer_turn(&app, state, memory, &session_id, &text, "steer", contract)
}

/// sen_session_follow_up: same gate and turn as steer, kind follow_up
/// (later turns resume the CLI session with -r).
#[tauri::command]
pub fn sen_session_follow_up(
    app: AppHandle,
    state: State<'_, Mutex<Registry>>,
    memory: State<'_, Mutex<Option<MemoryStore>>>,
    session_id: String,
    text: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    run_steer_turn(
        &app,
        state,
        memory,
        &session_id,
        &text,
        "follow_up",
        contract,
    )
}

/// sen_session_abort: escalates the running CLI turn first (SIGINT, 5s
/// grace, SIGKILL), then Running, AwaitingApproval, or Error ->
/// Cancelled, per the point-854 table as amended by P2-9. Terminal
/// states refuse with a typed error. Split in three (R2B-07), like run_steer_turn: begin_abort
/// validates the transition and hands out the bridge (step 1), the
/// bridge.abort() escalation runs with NO registry lock held so other
/// Tauri commands are not blocked while it polls the grace period
/// (step 2), then complete_abort re-checks the transition and records
/// Cancelled + the log entry for the AbortOutcome (step 3).
#[tauri::command]
pub fn sen_session_abort(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
) -> Result<SessionResult, String> {
    let bridge = {
        let mut registry = state.lock().map_err(|e| e.to_string())?;
        registry
            .begin_abort(&session_id, &contract.to_registry())
            .map_err(|e| e.to_wire())?
    };
    let outcome = match bridge {
        Some(bridge) => bridge.abort(),
        None => super::cli_spawn::AbortOutcome::NoChild,
    };
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let next = registry
        .complete_abort(&session_id, outcome)
        .map_err(|e| e.to_wire())?;
    let (route_id, cost) = route_cost(&registry, &session_id);
    Ok(result(&session_id, next, &route_id, &cost))
}

/// Result of sen_session_end: the end receipt, camelCase on the
/// wire like SessionResult. A separate struct on purpose (b.1):
/// SessionResult carries turn counts, route and cost, none of
/// which describe an end; this one carries the state the session
/// left, how many turns it completed, and how many unused
/// approvals the end purged.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEndResult {
    pub session_id: String,
    /// Always "Completed" on Ok.
    pub state: String,
    /// True when this call replayed an earlier end with the same
    /// idempotency key (b.5); false on the first end.
    pub already_ended: bool,
    /// "Running" or "Error": the state the session left.
    pub ended_from: String,
    /// Successful steers recorded on the session before the end.
    pub turns_completed: u64,
    /// Live approval grants of this session purged by the end
    /// (0 on a replay, which purges nothing).
    pub approvals_revoked: u64,
}

/// sen_session_end: Running or Error -> Completed, in one atomic
/// registry step (Q1-Q9). Thin wrapper like sen_session_abort,
/// but with no bridge step at all: the registry refuses while a
/// turn runs (ERR_TURN_IN_FLIGHT) instead of killing it, and
/// refuses an undecided approval (ERR_PENDING_APPROVAL) instead
/// of ending around it. No transcript file, no CLI cleanup, no
/// new session is created after the end (Q4, Q5, Q9).
#[tauri::command]
pub fn sen_session_end(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
) -> Result<SessionEndResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let receipt = registry
        .end(&session_id, &contract.to_registry())
        .map_err(|e| e.to_wire())?;
    Ok(SessionEndResult {
        session_id: receipt.session_id,
        state: receipt.state.as_str().to_string(),
        already_ended: receipt.already_ended,
        ended_from: receipt.ended_from.as_str().to_string(),
        turns_completed: receipt.turns_completed,
        approvals_revoked: receipt.approvals_revoked,
    })
}
