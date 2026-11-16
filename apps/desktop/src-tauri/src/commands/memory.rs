// Real Tauri command handlers for file memory Phase 1 (P0-1).
// Thin wrappers only: they convert wire args and delegate to
// commands/memory_store.rs, which owns the files, the items and the
// context block. No memory logic lives here.
//
// The store lives in Tauri-managed state (Mutex<Option<MemoryStore>>):
// Some once main.rs setup sets the memory dir from the app data dir.
// A command with no store configured fails loudly with
// MEMORY_NOT_CONFIGURED, never with a silent empty answer.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::State;

use super::memory_store::{Channel, ForgetTarget, LogKind, MemorySnapshot, MemoryStore, Scope};
use super::registry::validate_contract;
use super::session::ContractArgs;

pub const ERR_MEMORY_NOT_CONFIGURED: &str = "MEMORY_NOT_CONFIGURED";

/// Lock the managed store, or fail when it was never configured.
fn store<'a>(
    state: State<'a, Mutex<Option<MemoryStore>>>,
) -> Result<std::sync::MutexGuard<'a, Option<MemoryStore>>, String> {
    state.inner().lock().map_err(|e| e.to_string())
}

fn scope_of(value: &str) -> Result<Scope, String> {
    match value {
        "memory" => Ok(Scope::Memory),
        "user" => Ok(Scope::User),
        _ => Err(format!(
            "INVALID_CONTRACT: scope must be memory or user, got {:?}, FAIL",
            value
        )),
    }
}

fn kind_of(value: &str) -> Result<LogKind, String> {
    match value {
        "user" => Ok(LogKind::User),
        "assistant" => Ok(LogKind::Assistant),
        "memory" => Ok(LogKind::Memory),
        _ => Err(format!(
            "INVALID_CONTRACT: log kind must be user, assistant or memory, got {:?}, FAIL",
            value
        )),
    }
}

fn channel_of(value: &str) -> Result<Channel, String> {
    match value {
        "desktop" => Ok(Channel::Desktop),
        "telegram" => Ok(Channel::Telegram),
        "local" => Ok(Channel::Local),
        _ => Err(format!(
            "INVALID_CONTRACT: channel must be desktop, telegram or local, got {:?}, FAIL",
            value
        )),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReadResult {
    pub memory: Vec<String>,
    pub user: Vec<String>,
    pub soul: String,
    pub today: Vec<String>,
}

impl From<MemorySnapshot> for MemoryReadResult {
    fn from(snapshot: MemorySnapshot) -> MemoryReadResult {
        MemoryReadResult {
            memory: snapshot.memory,
            user: snapshot.user,
            soul: snapshot.soul,
            today: snapshot.today,
        }
    }
}

/// sen_memory_read: both item lists, the persona text, today's log.
#[tauri::command]
pub fn sen_memory_read(
    state: State<'_, Mutex<Option<MemoryStore>>>,
    contract: ContractArgs,
) -> Result<MemoryReadResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let guard = store(state)?;
    let memory = guard
        .as_ref()
        .ok_or_else(|| ERR_MEMORY_NOT_CONFIGURED.to_string())?;
    Ok(memory.read().into())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMemoryArgs {
    pub scope: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItemResult {
    pub scope: String,
    pub index: usize,
    pub item: String,
}

/// sen_memory_add: append one fact. Returns its 1-based item number.
#[tauri::command]
pub fn sen_memory_add(
    state: State<'_, Mutex<Option<MemoryStore>>>,
    args: AddMemoryArgs,
    contract: ContractArgs,
) -> Result<MemoryItemResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let scope = scope_of(&args.scope)?;
    let guard = store(state)?;
    let memory = guard
        .as_ref()
        .ok_or_else(|| ERR_MEMORY_NOT_CONFIGURED.to_string())?;
    let index = memory.add(scope, &args.text)?;
    Ok(MemoryItemResult {
        scope: args.scope,
        index,
        item: args.text.trim().to_string(),
    })
}

/// Wire form of a forget target: exactly one of `number` (1-based item)
/// or `text` (substring, must match exactly one item).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ForgetTargetArgs {
    Number { number: usize },
    Text { text: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetMemoryArgs {
    pub scope: String,
    pub target: ForgetTargetArgs,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryForgetResult {
    pub scope: String,
    pub removed: String,
}

/// sen_memory_forget: delete one item, by number or by text.
#[tauri::command]
pub fn sen_memory_forget(
    state: State<'_, Mutex<Option<MemoryStore>>>,
    args: ForgetMemoryArgs,
    contract: ContractArgs,
) -> Result<MemoryForgetResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let scope = scope_of(&args.scope)?;
    let target = match args.target {
        ForgetTargetArgs::Number { number } => ForgetTarget::Number(number),
        ForgetTargetArgs::Text { text } => ForgetTarget::Text(text),
    };
    let guard = store(state)?;
    let memory = guard
        .as_ref()
        .ok_or_else(|| ERR_MEMORY_NOT_CONFIGURED.to_string())?;
    let removed = memory.forget(scope, &target)?;
    Ok(MemoryForgetResult {
        scope: args.scope,
        removed,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditMemoryArgs {
    pub scope: String,
    pub index: usize,
    pub text: String,
}

/// sen_memory_edit: replace one item's text (1-based number).
#[tauri::command]
pub fn sen_memory_edit(
    state: State<'_, Mutex<Option<MemoryStore>>>,
    args: EditMemoryArgs,
    contract: ContractArgs,
) -> Result<MemoryItemResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let scope = scope_of(&args.scope)?;
    let guard = store(state)?;
    let memory = guard
        .as_ref()
        .ok_or_else(|| ERR_MEMORY_NOT_CONFIGURED.to_string())?;
    let item = memory.edit(scope, args.index, &args.text)?;
    Ok(MemoryItemResult {
        scope: args.scope,
        index: args.index,
        item,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSoulArgs {
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulResult {
    pub soul: String,
}

/// sen_memory_set_soul: replace the whole persona text.
#[tauri::command]
pub fn sen_memory_set_soul(
    state: State<'_, Mutex<Option<MemoryStore>>>,
    args: SetSoulArgs,
    contract: ContractArgs,
) -> Result<SoulResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let guard = store(state)?;
    let memory = guard
        .as_ref()
        .ok_or_else(|| ERR_MEMORY_NOT_CONFIGURED.to_string())?;
    memory.set_soul(&args.text)?;
    Ok(SoulResult {
        soul: args.text.trim().to_string(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogMemoryArgs {
    pub kind: String,
    pub channel: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogMemoryResult {
    pub logged: bool,
}

/// sen_memory_log: append one event to today's daily log.
#[tauri::command]
pub fn sen_memory_log(
    state: State<'_, Mutex<Option<MemoryStore>>>,
    args: LogMemoryArgs,
    contract: ContractArgs,
) -> Result<LogMemoryResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let kind = kind_of(&args.kind)?;
    let channel = channel_of(&args.channel)?;
    let guard = store(state)?;
    let memory = guard
        .as_ref()
        .ok_or_else(|| ERR_MEMORY_NOT_CONFIGURED.to_string())?;
    memory.log(kind, channel, &args.text)?;
    Ok(LogMemoryResult { logged: true })
}
