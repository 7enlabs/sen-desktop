// Real Tauri command handlers for the Telegram bridge (P0-1).
// Thin wrappers only: they convert wire args and delegate to
// commands/telegram_client.rs, which owns the Bot API client, the
// update parsing and the poll offset. No Telegram logic lives here.
//
// The poll loop is caller-driven (see telegram_client.rs): the chat
// layer calls sen_telegram_poll_once again after each round, with a
// 1s backoff after an error. There is no background thread in the
// backend, so there is no start/stop command and no stop flag to race
// a 30s long-poll round. The poller (client + offset) lives in
// Tauri-managed state (Mutex<Option<TelegramPoller>>): it is built
// lazily from the environment on first use and then reused, so the
// offset survives across rounds and the HTTP client is not rebuilt
// per round. The offset is also durable: it is written to the offset
// file (set by main.rs setup, next to the memory files) before the
// in-memory offset moves, so a restart resumes instead of replaying
// up to 100 already-handled updates. When no offset path is set (app
// data dir unavailable) the offset is memory-only, as before.
//
// The token never crosses IPC: no command takes it, no command returns
// it, and the status result only says whether one is configured. Error
// strings from the client are redacted before they leave the client
// (see telegram_client.rs redact_error), so no token reaches the
// webview through a failed poll or send either.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

use super::registry::validate_contract;
use super::session::ContractArgs;
use super::telegram_client::{
    FileOffsetStore, ReqwestHttp, TelegramClient, TelegramPoller, TelegramUpdate,
};

pub const ERR_TELEGRAM_NOT_CONFIGURED: &str = "TELEGRAM_NOT_CONFIGURED";

/// Managed Telegram state: the poller once built (client + offset),
/// None until the first use builds it from the environment, plus the
/// path of the durable offset file (set by main.rs setup).
pub struct TelegramState {
    pub poller: Option<TelegramPoller<ReqwestHttp>>,
    pub offset_path: Option<PathBuf>,
}

impl TelegramState {
    pub fn new() -> TelegramState {
        TelegramState {
            poller: None,
            offset_path: None,
        }
    }

    /// Where the poll offset is persisted (app data dir + the memory
    /// dir + "telegram-offset"). Called once from main.rs setup.
    pub fn set_offset_path(&mut self, path: PathBuf) {
        self.offset_path = Some(path);
    }
}

/// Lock the managed state and make sure a poller exists, building it
/// from the environment when this is the first use. Err when no token
/// is configured: the bridge is not set up, which is a normal state
/// (the desktop chat still works).
fn poller_or_fail<'a>(
    state: State<'a, Mutex<TelegramState>>,
) -> Result<std::sync::MutexGuard<'a, TelegramState>, String> {
    let mut guard = state.inner().lock().map_err(|e| e.to_string())?;
    if guard.poller.is_none() {
        let client = TelegramClient::from_env()?.ok_or_else(|| {
            format!(
                "{}: no {} in the environment, FAIL",
                ERR_TELEGRAM_NOT_CONFIGURED,
                super::telegram_client::ENV_BOT_TOKEN
            )
        })?;
        guard.poller = Some(match guard.offset_path.clone() {
            Some(path) => TelegramPoller::with_offset_store(client, FileOffsetStore::new(path)),
            None => TelegramPoller::new(client),
        });
    }
    Ok(guard)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramStatusResult {
    pub configured: bool,
    pub allowed_chat_id_set: bool,
    pub allow_any_chat: bool,
}

/// sen_telegram_status: whether a token is configured (env or cached
/// client), whether an allowlist chat id is set, and whether the
/// explicit any-chat opt-in is on. Never returns the token.
#[tauri::command]
pub fn sen_telegram_status(
    state: State<'_, Mutex<TelegramState>>,
    contract: ContractArgs,
) -> Result<TelegramStatusResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let cached: Option<(bool, bool)> = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        guard.poller.as_ref().map(|poller| {
            (
                poller.client_allowed_chat_id_set(),
                poller.client_allow_any_chat(),
            )
        })
    };
    match cached {
        Some((allowed, allow_any)) => Ok(TelegramStatusResult {
            configured: true,
            allowed_chat_id_set: allowed,
            allow_any_chat: allow_any,
        }),
        None => match TelegramClient::<ReqwestHttp>::from_env() {
            Ok(Some(client)) => Ok(TelegramStatusResult {
                configured: true,
                allowed_chat_id_set: client.allowed_chat_id().is_some(),
                allow_any_chat: client.allow_any_chat(),
            }),
            Ok(None) => Ok(TelegramStatusResult {
                configured: false,
                allowed_chat_id_set: false,
                allow_any_chat: false,
            }),
            Err(e) => Err(e),
        },
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramUpdateEvent {
    pub update_id: i64,
    pub chat_id: i64,
    pub text: String,
    pub from_username: Option<String>,
}

impl From<TelegramUpdate> for TelegramUpdateEvent {
    fn from(update: TelegramUpdate) -> TelegramUpdateEvent {
        TelegramUpdateEvent {
            update_id: update.update_id,
            chat_id: update.chat_id,
            text: update.text,
            from_username: update.from_username,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramPollResult {
    pub updates: Vec<TelegramUpdateEvent>,
}

/// sen_telegram_poll_once: one getUpdates round (long-poll, up to
/// 30s), returning the new allowed text updates. The offset is kept
/// in the managed poller, so the caller just calls again. An error
/// leaves the offset untouched: the next round re-fetches the same
/// updates instead of losing them.
#[tauri::command]
pub fn sen_telegram_poll_once(
    state: State<'_, Mutex<TelegramState>>,
    contract: ContractArgs,
) -> Result<TelegramPollResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let mut guard = poller_or_fail(state)?;
    let poller = guard.poller.as_mut().ok_or_else(|| {
        format!(
            "{}: poller missing after build, FAIL",
            ERR_TELEGRAM_NOT_CONFIGURED
        )
    })?;
    let mut updates = Vec::new();
    poller.poll_once(&mut |update| updates.push(TelegramUpdateEvent::from(update)))?;
    Ok(TelegramPollResult { updates })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTelegramArgs {
    pub chat_id: i64,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramSendResult {
    pub sent: bool,
}

/// sen_telegram_send: send one reply to a Telegram chat. Long text is
/// split into 4096-UTF-16-unit messages by the client, never truncated.
#[tauri::command]
pub fn sen_telegram_send(
    state: State<'_, Mutex<TelegramState>>,
    args: SendTelegramArgs,
    contract: ContractArgs,
) -> Result<TelegramSendResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    if args.text.trim().is_empty() {
        return Err("INVALID_CONTRACT: text must be non-empty, FAIL".to_string());
    }
    let mut guard = poller_or_fail(state)?;
    let poller = guard.poller.as_mut().ok_or_else(|| {
        format!(
            "{}: poller missing after build, FAIL",
            ERR_TELEGRAM_NOT_CONFIGURED
        )
    })?;
    poller.client_send_message(args.chat_id, &args.text)?;
    Ok(TelegramSendResult { sent: true })
}
