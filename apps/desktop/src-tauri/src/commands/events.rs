// Real Tauri command handler for event subscriptions (CLI bridge round).
// Thin wrapper only: it converts wire args and delegates to
// commands/registry.rs, which records/drops one subscriber on the
// session (BUG-09: subscribe refuses a terminal session, unsubscribe
// drops the count, abort resets it to 0).
// The event stream itself flows from the backend (Tauri events, named
// sen-session-event) while a CLI turn runs; this record is the backend
// bookkeeping for it, not the stream.
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

use super::registry::Registry;
use super::session::ContractArgs;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResult {
    pub session_id: String,
    pub subscriber_id: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeResult {
    pub session_id: String,
    pub subscriber_count: u32,
}

/// sen_events_subscribe: records one subscriber, returns its id.
#[tauri::command]
pub fn sen_events_subscribe(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
) -> Result<SubscribeResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let subscriber_id = registry
        .subscribe(&session_id, &contract.to_registry())
        .map_err(|e| e.to_wire())?;
    Ok(SubscribeResult {
        session_id,
        subscriber_id,
    })
}

/// sen_events_unsubscribe: drops one subscriber, returns the remaining
/// count. BUG-09 (#24): no unsubscribe existed, so the backend
/// subscriber count only ever grew. The frontend close path calls this
/// before abort; abort itself also resets the count to 0.
#[tauri::command]
pub fn sen_events_unsubscribe(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
) -> Result<UnsubscribeResult, String> {
    let mut registry = state.lock().map_err(|e| e.to_string())?;
    let subscriber_count = registry
        .unsubscribe(&session_id, &contract.to_registry())
        .map_err(|e| e.to_wire())?;
    Ok(UnsubscribeResult {
        session_id,
        subscriber_count,
    })
}
