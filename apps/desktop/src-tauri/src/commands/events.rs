// Real Tauri command handler for event subscriptions (bridge round).
// Thin wrapper only: it converts wire args and delegates to
// commands/registry.rs, which records one subscriber on the session.
// The event stream itself will flow from the backend (Tauri events) once
// the CLI bridge lands; this record is the backend bookkeeping for it,
// not the stream.
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
