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
// Tauri-managed state: it is built lazily from the environment on
// first use and then reused, so the offset survives across rounds and
// the HTTP client is not rebuilt per round. The offset is also
// durable: it is written to the offset file (set by main.rs setup,
// next to the memory files) before the in-memory offset moves, so a
// restart resumes instead of replaying up to 100 already-handled
// updates. When no offset path is set (app data dir unavailable) the
// offset is memory-only, as before.
//
// Lock discipline (Rust P1-3): a poll round NEVER holds the state
// lock. sen_telegram_poll_once checks the poller out of the state,
// drops the lock, polls for up to 30s, then checks the poller back
// in — so sen_telegram_send and sen_telegram_status, which only need
// the lock briefly, never queue behind a long-poll. Send goes one
// step further: it clones the client out of the poller (or builds a
// fresh one from the environment while a round has the poller checked
// out) and sends with no lock held at all. Poll rounds are still
// serialized with each other by a dedicated lock, so two overlapping
// polls cannot fetch the same batch twice.
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
/// path of the durable offset file (set by main.rs setup). Each piece
/// lives behind its own lock, so no lock is ever held across a
/// network round (Rust P1-3): the poller lock is held only to check
/// the poller out/in, the poll lock serializes rounds with each
/// other, and the offset path is read once when the poller is built.
pub struct TelegramState {
    poller: Mutex<Option<TelegramPoller<ReqwestHttp>>>,
    offset_path: Mutex<Option<PathBuf>>,
    /// Serializes poll rounds with each other (held for the whole
    /// long-poll round). Send and status never take this lock, so a
    /// reply is never queued behind a 30s poll.
    poll_lock: Mutex<()>,
}

impl TelegramState {
    pub fn new() -> TelegramState {
        TelegramState {
            poller: Mutex::new(None),
            offset_path: Mutex::new(None),
            poll_lock: Mutex::new(()),
        }
    }

    /// Where the poll offset is persisted (app data dir + the memory
    /// dir + "telegram-offset"). Called once from main.rs setup.
    pub fn set_offset_path(&self, path: PathBuf) {
        if let Ok(mut slot) = self.offset_path.lock() {
            *slot = Some(path);
        }
    }
}

impl Default for TelegramState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the poller from the environment. Ok(None) when no token is
/// configured: the bridge is not set up, which is a normal state
/// (the desktop chat still works). Err only when the allowlist value
/// is set but is not a number.
fn build_poller_from_env(
    offset_path: Option<PathBuf>,
) -> Result<Option<TelegramPoller<ReqwestHttp>>, String> {
    let client = match TelegramClient::from_env()? {
        Some(client) => client,
        None => return Ok(None),
    };
    Ok(Some(match offset_path {
        Some(path) => TelegramPoller::with_offset_store(client, FileOffsetStore::new(path)),
        None => TelegramPoller::new(client),
    }))
}

/// Check the poller out of the managed state, building it first when
/// this is the first use. The state lock is held only for the
/// build/take and released before the caller polls: the long-poll
/// round (up to 30s) must never hold it (Rust P1-3), or send and
/// status — which need the same lock — queue behind it.
fn take_poller(state: &TelegramState) -> Result<TelegramPoller<ReqwestHttp>, String> {
    let offset_path = state.offset_path.lock().map_err(|e| e.to_string())?.clone();
    let mut slot = state.poller.lock().map_err(|e| e.to_string())?;
    if slot.is_none() {
        *slot = build_poller_from_env(offset_path)?;
    }
    slot.take().ok_or_else(|| {
        format!(
            "{}: no {} in the environment, FAIL",
            ERR_TELEGRAM_NOT_CONFIGURED,
            super::telegram_client::ENV_BOT_TOKEN
        )
    })
}

/// Check a poller back in after its round. Overwrites unconditionally:
/// nothing else stores a poller in the meantime (send builds a
/// throwaway client, never a poller — see client_for_send), so the
/// returning poller always holds the authoritative offset and seen
/// set. The poller goes back even when the round failed: the offset
/// is untouched on error and the next round re-fetches.
fn return_poller(state: &TelegramState, poller: TelegramPoller<ReqwestHttp>) {
    // A poisoned lock can only mean an earlier panic while holding
    // it; dropping the poller then is safe — the next round rebuilds
    // from the durable offset file instead of replaying from 0.
    if let Ok(mut slot) = state.poller.lock() {
        *slot = Some(poller);
    }
}

/// A client to send with, cloned out of the managed state without
/// holding the state lock through the network call. When a poll round
/// has the poller checked out, the client is built fresh from the
/// environment instead of waiting for the round (Rust P1-3): a reply
/// must not queue behind a 30s long-poll. The fresh client is a
/// throwaway — it is never stored — so it cannot disturb the checked
/// out poller's offset. Err when no token is configured.
fn client_for_send(state: &TelegramState) -> Result<TelegramClient<ReqwestHttp>, String> {
    let cached = {
        let slot = state.poller.lock().map_err(|e| e.to_string())?;
        slot.as_ref().map(|poller| poller.clone_client())
    };
    match cached {
        Some(client) => Ok(client),
        None => TelegramClient::from_env()?.ok_or_else(|| {
            format!(
                "{}: no {} in the environment, FAIL",
                ERR_TELEGRAM_NOT_CONFIGURED,
                super::telegram_client::ENV_BOT_TOKEN
            )
        }),
    }
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
    state: State<'_, TelegramState>,
    contract: ContractArgs,
) -> Result<TelegramStatusResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let cached: Option<(bool, bool)> = {
        let slot = state.inner().poller.lock().map_err(|e| e.to_string())?;
        slot.as_ref().map(|poller| {
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
/// updates instead of losing them. The round checks the poller out
/// of the state and never holds the state lock while polling (Rust
/// P1-3), so send and status stay usable during a round; rounds are
/// serialized with each other by the poll lock instead.
#[tauri::command]
pub fn sen_telegram_poll_once(
    state: State<'_, TelegramState>,
    contract: ContractArgs,
) -> Result<TelegramPollResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    // Serialize rounds with each other (as the state lock used to):
    // two overlapping polls must not fetch the same batch twice.
    // Send and status never take this lock. The guard lives for the
    // whole round — it is dropped only when the function returns.
    let _poll_guard = state.inner().poll_lock.lock().map_err(|e| e.to_string())?;
    let mut poller = take_poller(state.inner())?;
    let mut updates = Vec::new();
    let result = poller.poll_once(&mut |update| updates.push(TelegramUpdateEvent::from(update)));
    // The poller always goes back, even when the round failed.
    return_poller(state.inner(), poller);
    result?;
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
/// The send clones the client out and holds no lock during the network
/// call, so a reply never queues behind a poll round in flight (Rust
/// P1-3).
#[tauri::command]
pub fn sen_telegram_send(
    state: State<'_, TelegramState>,
    args: SendTelegramArgs,
    contract: ContractArgs,
) -> Result<TelegramSendResult, String> {
    validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    if args.text.trim().is_empty() {
        return Err("INVALID_CONTRACT: text must be non-empty, FAIL".to_string());
    }
    let client = client_for_send(state.inner())?;
    client.send_message(args.chat_id, &args.text)?;
    Ok(TelegramSendResult { sent: true })
}
