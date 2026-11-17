// Telegram Bot API client + poll loop (P0-1). PURE RUST, no Tauri
// dependency: compiles and tests standalone in the scratch harness.
//
// The token lives here and nowhere else in the app: it comes from the
// environment (SEN_TELEGRAM_BOT_TOKEN), is stored in this struct, and
// never crosses IPC into the webview, never appears in a log line, and
// never appears in a Debug dump (the Debug impl redacts it). It does
// travel in the request path (https://api.telegram.org/bot<TOKEN>/...),
// which is Telegram's own auth scheme; the client never logs request
// URLs, and every error string leaving this module is redacted
// (see redact_error), because reqwest's error Display embeds the
// request URL — token included — and that string travels over IPC into
// the transcript and the webview console.
//
// The HTTP transport is generic (TelegramHttp) so tests inject a fake:
// the production transport is ReqwestHttp (reqwest blocking, rustls).
// The poll loop is caller-driven: the chat layer calls poll_once
// again after each round, with a 1s backoff after an error. There is
// no background thread in the backend and no stop flag: stopping is
// just not calling again. An error in one round never kills the loop,
// because a transient network blip must not take the bridge down.
//
// Allowlist: when SEN_TELEGRAM_ALLOWED_CHAT_ID is set, only that chat's
// messages are emitted, and only that chat may receive replies
// (Rust P1-4: send enforces the same allowlist as poll, through
// `send_message`, so no caller can bypass it). The offset still
// advances past other chats' updates (they are skipped, not
// re-fetched forever): the skip happens in poll_once AFTER the offset
// moves, never before.
use std::time::Duration;

/// Long-poll wait per getUpdates round (seconds). The client read
/// timeout is longer than this, so a round that waits the full 30s is
/// not cut off by our own timeout.
pub const POLL_TIMEOUT_SECS: u64 = 30;

/// Read timeout for one HTTP round (seconds): the long-poll wait plus
/// margin.
pub const HTTP_TIMEOUT_SECS: u64 = 35;

/// The poll loop is caller-driven: poll_once is the unit under test,
/// and the caller (the desktop chat layer) calls it again after each
/// round, with a 1s backoff after an error. There is no background
/// thread in the backend: the chat layer owns the loop, so stopping is
/// just not calling again, and no stop flag can race a 30s long-poll
/// round.

/// Telegram's per-message text limit, in UTF-16 code units — that is
/// how Telegram counts it. A longer reply is sent as several messages,
/// never truncated silently. Chunking by Rust chars would be wrong:
/// 4096 emoji are 4096 chars but 8192 UTF-16 units, over the limit.
pub const SEND_MESSAGE_MAX_UTF16_UNITS: usize = 4096;

/// Default Bot API base URL. The only base URL: no custom endpoint is
/// supported in Phase 1.
pub const DEFAULT_BASE_URL: &str = "https://api.telegram.org";

/// Env var holding the bot token. Unset/blank means the bridge is not
/// configured (the desktop chat still works).
pub const ENV_BOT_TOKEN: &str = "SEN_TELEGRAM_BOT_TOKEN";

/// Env var holding the optional allowed chat id. Unset means NO chat
/// may use the assistant (fail closed): the operator must set either
/// this or SEN_TELEGRAM_ALLOW_ANY_CHAT before the bridge serves
/// anyone. An unset allowlist used to mean "any chat may talk to it";
/// that default-open behaviour is gone (review R1, 16/11).
pub const ENV_ALLOWED_CHAT_ID: &str = "SEN_TELEGRAM_ALLOWED_CHAT_ID";

/// Env var recording the operator's explicit acceptance of an open
/// bridge: set to "1" or "true" to serve any chat that finds the bot.
/// This is the recorded opt-in the fail-closed default requires; a
/// missing or any other value keeps the bridge closed to every chat.
pub const ENV_ALLOW_ANY_CHAT: &str = "SEN_TELEGRAM_ALLOW_ANY_CHAT";

/// Minimal HTTP surface the client needs. GET returns the response
/// body; POST sends a JSON body and returns the response body.
/// A non-2xx status is an Err carrying the status, never a fake body.
pub trait TelegramHttp: Send + Sync {
    fn get(&self, url: &str) -> Result<String, String>;
    fn post_json(&self, url: &str, body: &str) -> Result<String, String>;
}

/// Production transport: reqwest blocking, rustls, one client per
/// TelegramClient (reqwest clients are cheap to clone and share a pool).
#[derive(Debug, Clone)]
pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
}

impl ReqwestHttp {
    pub fn new() -> Result<ReqwestHttp, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("Cannot build Telegram HTTP client: {}", e))?;
        Ok(ReqwestHttp { client })
    }
}

impl TelegramHttp for ReqwestHttp {
    fn get(&self, url: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| format!("Telegram GET failed: {}", e))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|e| format!("Telegram GET body unreadable: {}", e))?;
        if !status.is_success() {
            return Err(format!("Telegram GET HTTP {}: {}", status, body));
        }
        Ok(body)
    }

    fn post_json(&self, url: &str, body: &str) -> Result<String, String> {
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| format!("Telegram POST failed: {}", e))?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|e| format!("Telegram POST body unreadable: {}", e))?;
        if !status.is_success() {
            return Err(format!("Telegram POST HTTP {}: {}", status, text));
        }
        Ok(text)
    }
}

/// One incoming text message: the only update kind the bridge routes.
/// Every other update kind (photo, sticker, edit, ...) is skipped at
/// parse time, never delivered as a fake text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub chat_id: i64,
    pub text: String,
    pub from_username: Option<String>,
}

/// What one getUpdates response carries: the text updates worth
/// routing, plus the highest update id the API returned — including
/// skipped items (photo, edit, ...). The poller advances its offset
/// past that id, so a skipped update is never fetched again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUpdates {
    pub updates: Vec<TelegramUpdate>,
    pub max_update_id: i64,
}

/// Parse one getUpdates response body. `ok: false` is a real failure
/// (the error carries Telegram's description). A `result` that is not
/// an array is a real failure too. Individual items without a text
/// message, or without a usable chat id, are skipped: a photo update
/// is normal traffic, not an error, and one odd update must not kill
/// the poll loop. Skipped items still count toward max_update_id, so
/// the poller never re-fetches them.
pub fn parse_updates(json: &str) -> Result<ParsedUpdates, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Telegram updates are not JSON: {}", e))?;
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("Telegram getUpdates not ok: {}", description));
    }
    let result = value
        .get("result")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Telegram getUpdates result is not an array, FAIL".to_string())?;
    let mut updates = Vec::new();
    let mut max_update_id: i64 = -1;
    for item in result {
        let Some(update_id) = item.get("update_id").and_then(|v| v.as_i64()) else {
            continue;
        };
        max_update_id = max_update_id.max(update_id);
        let Some(message) = item.get("message") else {
            continue;
        };
        let Some(text) = message.get("text").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(chat_id) = message
            .get("chat")
            .and_then(|c| c.get("id"))
            .and_then(|v| v.as_i64())
        else {
            continue;
        };
        let from_username = message
            .get("from")
            .and_then(|f| f.get("username"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        updates.push(TelegramUpdate {
            update_id,
            chat_id,
            text: text.to_string(),
            from_username,
        });
    }
    Ok(ParsedUpdates {
        updates,
        max_update_id,
    })
}

/// Split text into chunks of at most `max_units` UTF-16 code units
/// each — the unit Telegram counts toward its 4096 limit. A char is
/// 1 unit (BMP) or 2 units (astral, e.g. emoji), never more, so no
/// single char can overflow a chunk on its own. An empty text yields
/// one empty chunk (sending an empty reply is still one send).
fn split_utf16_units(text: &str, max_units: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut units = 0usize;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if units + width > max_units && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += width;
    }
    chunks.push(current);
    chunks
}

/// The Telegram client: token + transport + optional allowlist. The
/// token is never logged and never Debug-printed (see the manual Debug
/// impl below), and never leaks through an error string either (see
/// redact_error).
pub struct TelegramClient<H: TelegramHttp> {
    http: H,
    token: String,
    base_url: String,
    allowed_chat_id: Option<i64>,
    allow_any_chat: bool,
}

impl<H: TelegramHttp> std::fmt::Debug for TelegramClient<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramClient")
            .field("base_url", &self.base_url)
            .field("token", &"***")
            .field("allowed_chat_id", &self.allowed_chat_id)
            .field("allow_any_chat", &self.allow_any_chat)
            .finish()
    }
}

impl<H: TelegramHttp> Clone for TelegramClient<H>
where
    H: Clone,
{
    fn clone(&self) -> Self {
        TelegramClient {
            http: self.http.clone(),
            token: self.token.clone(),
            base_url: self.base_url.clone(),
            allowed_chat_id: self.allowed_chat_id,
            allow_any_chat: self.allow_any_chat,
        }
    }
}

impl<H: TelegramHttp> TelegramClient<H> {
    pub fn with_http(http: H, token: &str, allowed_chat_id: Option<i64>) -> TelegramClient<H> {
        TelegramClient {
            http,
            token: token.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            allowed_chat_id,
            allow_any_chat: false,
        }
    }

    /// Test-only: point the client at a fake base URL (the fake
    /// transport answers any URL, but the recorded URL proves the path).
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.to_string();
        self
    }

    /// Record the operator's explicit opt-in to serve any chat
    /// (SEN_TELEGRAM_ALLOW_ANY_CHAT). Without it and without an
    /// allowlist, the bridge serves nobody (fail closed).
    pub fn with_allow_any_chat(mut self, allow: bool) -> Self {
        self.allow_any_chat = allow;
        self
    }

    pub fn allowed_chat_id(&self) -> Option<i64> {
        self.allowed_chat_id
    }

    pub fn allow_any_chat(&self) -> bool {
        self.allow_any_chat
    }

    /// True when this chat may use the assistant. Fail closed: no
    /// allowlist and no explicit any-chat opt-in means NOBODY may,
    /// so a bot whose operator never set an allowlist cannot be
    /// found and driven by a stranger.
    pub fn is_allowed(&self, chat_id: i64) -> bool {
        match self.allowed_chat_id {
            Some(allowed) => allowed == chat_id,
            None => self.allow_any_chat,
        }
    }

    /// Strip the token out of an error string before it can travel
    /// anywhere. reqwest's error Display embeds the request URL,
    /// token included (`.../bot<TOKEN>/...`), and that string goes
    /// over IPC into the transcript and the webview console — so the
    /// raw error must never leave this module. The URL shape stays
    /// (useful for diagnosis); only the token is replaced.
    fn redact_error(&self, error: &str) -> String {
        if self.token.is_empty() || !error.contains(self.token.as_str()) {
            return error.to_string();
        }
        error.replace(self.token.as_str(), "***")
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base_url, self.token, method)
    }

    /// One getUpdates round at the given offset (long-poll, waits up
    /// to POLL_TIMEOUT_SECS). Returns the parsed response: the text
    /// updates plus the highest update id seen, unfiltered. The poller
    /// advances the offset past that id and applies the allowlist
    /// itself, so no update is ever fetched twice.
    pub fn get_updates(&self, offset: i64) -> Result<ParsedUpdates, String> {
        let url = format!(
            "{}?offset={}&timeout={}",
            self.api_url("getUpdates"),
            offset,
            POLL_TIMEOUT_SECS
        );
        let body = self.http.get(&url).map_err(|e| self.redact_error(&e))?;
        parse_updates(&body)
    }

    /// Send one reply, split into 4096-UTF-16-unit messages when
    /// longer. The allowlist is enforced here, like poll enforces it
    /// in `poll_once` (Rust P1-4): a chat that `is_allowed` refuses
    /// is an Err and no HTTP call is made, so a compromised webview or
    /// direct `sen_telegram_send` caller cannot drive the bot to an
    /// arbitrary chat. Returns Err when any chunk fails (earlier
    /// chunks may already have been sent; the caller reports that, it
    /// does not retry). Error strings are redacted: the transport
    /// error may embed the request URL, token included.
    pub fn send_message(&self, chat_id: i64, text: &str) -> Result<(), String> {
        if !self.is_allowed(chat_id) {
            return Err(format!(
                "TELEGRAM_CHAT_NOT_ALLOWED: chat {} is not allowed, FAIL",
                chat_id
            ));
        }
        let url = self.api_url("sendMessage");
        let chunks = split_utf16_units(text, SEND_MESSAGE_MAX_UTF16_UNITS);
        for chunk in chunks {
            let body = serde_json::json!({
                "chat_id": chat_id,
                "text": chunk,
            })
            .to_string();
            let resp = self
                .http
                .post_json(&url, &body)
                .map_err(|e| self.redact_error(&e))?;
            let value: serde_json::Value = serde_json::from_str(&resp)
                .map_err(|e| format!("Telegram sendMessage response is not JSON: {}", e))?;
            if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                let description = value
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(format!("Telegram sendMessage not ok: {}", description));
            }
        }
        Ok(())
    }
}

impl TelegramClient<ReqwestHttp> {
    /// Build the production client from an explicit token. A blank
    /// token is refused: an empty bot path would be a confusing 404.
    pub fn new(token: &str, allowed_chat_id: Option<i64>) -> Result<Self, String> {
        if token.trim().is_empty() {
            return Err("Telegram bot token must be non-empty, FAIL".to_string());
        }
        Ok(TelegramClient::with_http(
            ReqwestHttp::new()?,
            token.trim(),
            allowed_chat_id,
        ))
    }

    /// Build from the environment. Ok(None) when no token is set: the
    /// bridge is not configured, which is a normal state (desktop chat
    /// still works). Err only when the allowlist value is set but is
    /// not a number. The allowlist is fail closed: with neither
    /// SEN_TELEGRAM_ALLOWED_CHAT_ID nor SEN_TELEGRAM_ALLOW_ANY_CHAT=1
    /// set, the client serves no chat.
    pub fn from_env() -> Result<Option<Self>, String> {
        let token = std::env::var(ENV_BOT_TOKEN).unwrap_or_default();
        if token.trim().is_empty() {
            return Ok(None);
        }
        let allowed = match std::env::var(ENV_ALLOWED_CHAT_ID) {
            Ok(value) if !value.trim().is_empty() => {
                Some(value.trim().parse::<i64>().map_err(|_| {
                    format!(
                        "Env {} is not a chat id number: {:?}, FAIL",
                        ENV_ALLOWED_CHAT_ID, value
                    )
                })?)
            }
            _ => None,
        };
        let allow_any = matches!(
            std::env::var(ENV_ALLOW_ANY_CHAT)
                .unwrap_or_default()
                .trim()
                .to_lowercase()
                .as_str(),
            "1" | "true" | "yes"
        );
        Ok(Some(
            TelegramClient::new(&token, allowed)?.with_allow_any_chat(allow_any),
        ))
    }
}

/// Durable poll offset, one number in one file. The poller advances
/// its offset past every update the API returns, including skipped
/// ones — but an offset that only lives in memory is lost on restart,
/// and the next start re-fetches up to 100 already-handled updates:
/// duplicate replies, duplicate `/nhớ`, a replayed `/quên <n>` deleting
/// the wrong item. The offset file (under the app data dir, next to
/// the memory files) is written BEFORE the in-memory offset moves, so
/// a failed write leaves the offset untouched and the next round
/// re-fetches instead of silently skipping. A missing or unreadable
/// file means "start at 0" (fresh install, or a deleted state file):
/// replays are annoying but never destructive, because the in-process
/// dedupe below also guards a re-fetch.
pub struct FileOffsetStore {
    path: std::path::PathBuf,
}

impl FileOffsetStore {
    pub fn new(path: std::path::PathBuf) -> FileOffsetStore {
        FileOffsetStore { path }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn load(&self) -> i64 {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .unwrap_or(0)
    }

    pub fn save(&self, offset: i64) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Cannot create Telegram offset dir {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }
        std::fs::write(&self.path, format!("{}\n", offset)).map_err(|e| {
            format!(
                "Cannot write Telegram offset {}: {}",
                self.path.display(),
                e
            )
        })
    }
}

/// The poll loop: owns a client and the next offset. poll_once is the
/// unit under test and the unit the caller drives. The offset is
/// durable when an offset store is attached (see FileOffsetStore);
/// without one it is memory-only, exactly as before.
pub struct TelegramPoller<H: TelegramHttp> {
    client: TelegramClient<H>,
    offset: i64,
    offset_store: Option<FileOffsetStore>,
    /// Update ids already emitted in this process. A round whose
    /// offset write failed re-fetches the same updates on retry;
    /// the set stops them being emitted twice. Ids below the current
    /// offset can never come back, so they are pruned on every
    /// advance and the set stays tiny.
    seen_update_ids: std::collections::HashSet<i64>,
}

impl<H: TelegramHttp> TelegramPoller<H> {
    pub fn new(client: TelegramClient<H>) -> TelegramPoller<H> {
        TelegramPoller {
            client,
            offset: 0,
            offset_store: None,
            seen_update_ids: std::collections::HashSet::new(),
        }
    }

    /// Build with a durable offset: the starting offset is read from
    /// the store (0 when the file is missing or unreadable).
    pub fn with_offset_store(
        client: TelegramClient<H>,
        store: FileOffsetStore,
    ) -> TelegramPoller<H> {
        let offset = store.load();
        TelegramPoller {
            client,
            offset,
            offset_store: Some(store),
            seen_update_ids: std::collections::HashSet::new(),
        }
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }

    /// Whether the poller's client has an allowlist chat id set.
    pub fn client_allowed_chat_id_set(&self) -> bool {
        self.client.allowed_chat_id().is_some()
    }

    /// Whether the poller's client was built with the explicit
    /// any-chat opt-in (SEN_TELEGRAM_ALLOW_ANY_CHAT).
    pub fn client_allow_any_chat(&self) -> bool {
        self.client.allow_any_chat()
    }

    /// Clone the poller's client. Send and status use the clone with
    /// no lock held, so a reply never queues behind a poll round in
    /// flight (Rust P1-3): the client is cheap to clone (the reqwest
    /// client shares its connection pool) and carries the same token
    /// and allowlist as the poller's own.
    pub fn clone_client(&self) -> TelegramClient<H>
    where
        H: Clone,
    {
        self.client.clone()
    }

    /// Send one reply through the poller's client. The allowlist is
    /// enforced by `TelegramClient::send_message` (Rust P1-4), so this
    /// is the path `sen_telegram_send` takes and it cannot bypass it.
    pub fn client_send_message(&self, chat_id: i64, text: &str) -> Result<(), String> {
        self.client.send_message(chat_id, text)
    }

    /// One getUpdates round. The offset advances past the highest
    /// update id the API returned — including photo/edit updates and
    /// ones the allowlist skips — so no update is ever fetched again.
    /// Only allowed text updates reach `emit`, and an update id is
    /// never emitted twice in this process. The new offset is written
    /// to the store before the in-memory offset moves: a failed write
    /// fails the round and the offset stays, so the next round
    /// re-fetches (and the dedupe set swallows the repeats).
    /// Returns the number of updates emitted.
    pub fn poll_once(&mut self, emit: &mut dyn FnMut(TelegramUpdate)) -> Result<usize, String> {
        let parsed = self.client.get_updates(self.offset)?;
        if parsed.max_update_id >= 0 {
            let new_offset = self.offset.max(parsed.max_update_id + 1);
            if new_offset != self.offset {
                if let Some(store) = &self.offset_store {
                    store.save(new_offset)?;
                }
                self.offset = new_offset;
            }
            self.seen_update_ids.retain(|id| *id >= self.offset);
        }
        let mut emitted = 0;
        for update in parsed.updates {
            if !self.seen_update_ids.insert(update.update_id) {
                continue;
            }
            if self.client.is_allowed(update.chat_id) {
                emit(update);
                emitted += 1;
            }
        }
        Ok(emitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Fake transport: canned GET bodies in order, and a record of
    /// every call (URL + POST body) for assertions. State is shared
    /// through Arc, so a clone made for the client under test still
    /// records into the same log the test reads.
    #[derive(Clone)]
    struct FakeHttp {
        gets: Arc<Mutex<Vec<String>>>,
        calls: Arc<Mutex<Vec<(String, Option<String>)>>>,
        fail_get: Arc<Mutex<Option<String>>>,
        fail_post: Arc<Mutex<Option<String>>>,
    }

    impl FakeHttp {
        fn new(gets: Vec<String>) -> FakeHttp {
            FakeHttp {
                gets: Arc::new(Mutex::new(gets)),
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_get: Arc::new(Mutex::new(None)),
                fail_post: Arc::new(Mutex::new(None)),
            }
        }

        fn with_get_error(error: &str) -> FakeHttp {
            let fake = FakeHttp::new(vec![]);
            *fake.fail_get.lock().unwrap() = Some(error.to_string());
            fake
        }

        fn with_post_error(error: &str) -> FakeHttp {
            let fake = FakeHttp::new(vec![]);
            *fake.fail_post.lock().unwrap() = Some(error.to_string());
            fake
        }

        fn calls(&self) -> Vec<(String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl TelegramHttp for FakeHttp {
        fn get(&self, url: &str) -> Result<String, String> {
            self.calls.lock().unwrap().push((url.to_string(), None));
            if let Some(error) = self.fail_get.lock().unwrap().clone() {
                return Err(error);
            }
            let mut gets = self.gets.lock().unwrap();
            if gets.is_empty() {
                return Ok(r#"{"ok":true,"result":[]}"#.to_string());
            }
            Ok(gets.remove(0))
        }

        fn post_json(&self, url: &str, body: &str) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((url.to_string(), Some(body.to_string())));
            if let Some(error) = self.fail_post.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(r#"{"ok":true,"result":{"message_id":1}}"#.to_string())
        }
    }

    fn updates_json() -> String {
        r#"{"ok":true,"result":[
          {"update_id":10,"message":{"message_id":1,"from":{"id":7,"username":"zen"},"chat":{"id":100},"text":"hello"}},
          {"update_id":11,"message":{"message_id":2,"from":{"id":7},"chat":{"id":100},"text":"second"}},
          {"update_id":12,"message":{"message_id":3,"from":{"id":7},"chat":{"id":100},"photo":[{"file_id":"x"}]}},
          {"update_id":13,"edited_message":{"message_id":4,"from":{"id":7},"chat":{"id":100},"text":"edited"}}
        ]}"#
        .to_string()
    }

    #[test]
    fn parse_updates_keeps_text_only_and_skips_photo_and_edit() {
        let parsed = parse_updates(&updates_json()).unwrap();
        let updates = parsed.updates;
        assert_eq!(updates.len(), 2);
        assert_eq!(
            updates[0],
            TelegramUpdate {
                update_id: 10,
                chat_id: 100,
                text: "hello".to_string(),
                from_username: Some("zen".to_string()),
            }
        );
        assert_eq!(updates[1].update_id, 11);
        assert_eq!(updates[1].from_username, None);
        // The skipped photo (12) and edit (13) still count toward the
        // max id, so the poller never re-fetches them.
        assert_eq!(parsed.max_update_id, 13);
    }

    #[test]
    fn parse_updates_fails_on_not_ok_and_bad_json() {
        assert!(
            parse_updates(r#"{"ok":false,"description":"Unauthorized"}"#)
                .unwrap_err()
                .contains("Unauthorized")
        );
        assert!(parse_updates("not json").is_err());
        assert!(parse_updates(r#"{"ok":true,"result":{}}"#).is_err());
        let empty = parse_updates(r#"{"ok":true,"result":[]}"#).unwrap();
        assert_eq!(empty.updates, Vec::<TelegramUpdate>::new());
        assert_eq!(empty.max_update_id, -1);
    }

    #[test]
    fn get_updates_calls_the_right_url_with_offset_and_timeout() {
        let http = FakeHttp::new(vec![updates_json()]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", None);
        let parsed = client.get_updates(42).unwrap();
        assert_eq!(parsed.updates.len(), 2);
        assert_eq!(parsed.max_update_id, 13);
        let calls = http.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].0.contains("/botTOKEN123/getUpdates"),
            "url: {}",
            calls[0].0
        );
        assert!(calls[0].0.contains("offset=42"), "url: {}", calls[0].0);
        assert!(calls[0].0.contains("timeout=30"), "url: {}", calls[0].0);
    }

    #[test]
    fn send_message_posts_chat_id_and_text_and_ok_means_sent() {
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100));
        client.send_message(100, "hi there").unwrap();
        let calls = http.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].0.contains("/botTOKEN123/sendMessage"),
            "url: {}",
            calls[0].0
        );
        let body = calls[0].1.as_ref().unwrap();
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(value["chat_id"], 100);
        assert_eq!(value["text"], "hi there");
    }

    #[test]
    fn send_message_splits_a_long_reply_into_chunks() {
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100));
        let long = "y".repeat(SEND_MESSAGE_MAX_UTF16_UNITS + 10);
        client.send_message(100, &long).unwrap();
        let calls = http.calls();
        assert_eq!(calls.len(), 2, "a 4106-unit reply needs 2 messages");
        let first: serde_json::Value = serde_json::from_str(calls[0].1.as_ref().unwrap()).unwrap();
        assert_eq!(
            first["text"].as_str().unwrap().chars().count(),
            SEND_MESSAGE_MAX_UTF16_UNITS
        );
    }

    #[test]
    fn send_message_splits_emoji_by_utf16_units_not_chars() {
        // 4096 emoji are 4096 Rust chars but 8192 UTF-16 units:
        // Telegram counts units, so this must be 2 messages of 2048
        // emoji each — char-based chunking would send 1 message of
        // 8192 units, over the limit.
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100));
        let emoji = "😀".repeat(SEND_MESSAGE_MAX_UTF16_UNITS);
        client.send_message(100, &emoji).unwrap();
        let calls = http.calls();
        assert_eq!(calls.len(), 2, "4096 emoji = 8192 units, needs 2");
        for call in &calls {
            let value: serde_json::Value = serde_json::from_str(call.1.as_ref().unwrap()).unwrap();
            let text = value["text"].as_str().unwrap();
            assert_eq!(text.chars().count(), 2048);
            assert_eq!(text.encode_utf16().count(), 4096);
        }
        // Mixed: 4095 ASCII + 1 emoji = 4097 units, also 2 messages.
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100));
        let mixed = format!("{}😀", "y".repeat(4095));
        client.send_message(100, &mixed).unwrap();
        assert_eq!(http.calls().len(), 2);
    }

    #[test]
    fn clone_client_shares_token_and_config_and_sends() {
        // Rust P1-3: send clones the client out of the poller and
        // sends with no lock held. The clone must carry the same
        // token and allowlist, and send through the same transport.
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100))
            .with_allow_any_chat(true);
        let poller = TelegramPoller::new(client);
        let cloned = poller.clone_client();
        assert_eq!(cloned.allowed_chat_id(), Some(100));
        assert!(cloned.allow_any_chat());
        cloned.send_message(100, "hi").unwrap();
        let calls = http.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].0.contains("/botTOKEN123/sendMessage"),
            "url: {}",
            calls[0].0
        );
        // The clone redacts the token in Debug, like the original.
        assert!(!format!("{:?}", cloned).contains("TOKEN123"));
    }

    #[test]
    fn poll_once_advances_the_offset_and_emits_in_order() {
        let http = FakeHttp::new(vec![
            updates_json(),
            r#"{"ok":true,"result":[]}"#.to_string(),
        ]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(100));
        let mut poller = TelegramPoller::new(client);
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        assert_eq!(poller.poll_once(&mut |u| seen.push(u)).unwrap(), 2);
        assert_eq!(poller.offset(), 14);
        assert_eq!(seen[0].text, "hello");
        assert_eq!(seen[1].text, "second");
        // Second round: empty, offset stays.
        assert_eq!(poller.poll_once(&mut |_| {}).unwrap(), 0);
        assert_eq!(poller.offset(), 14);
    }

    #[test]
    fn poll_once_advances_past_skipped_chats_and_emits_only_allowed() {
        let http = FakeHttp::new(vec![updates_json()]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(999));
        let mut poller = TelegramPoller::new(client);
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        // Both text updates are from chat 100, not the allowed 999:
        // nothing is emitted, but the offset still moves past them.
        assert_eq!(poller.poll_once(&mut |u| seen.push(u)).unwrap(), 0);
        assert!(seen.is_empty());
        assert_eq!(poller.offset(), 14, "offset must move past skipped updates");
    }

    #[test]
    fn poll_once_error_does_not_move_the_offset() {
        let http = FakeHttp::with_get_error("network down");
        let client = TelegramClient::with_http(http, "TOKEN123", None);
        let mut poller = TelegramPoller::new(client);
        assert!(poller.poll_once(&mut |_| {}).is_err());
        assert_eq!(poller.offset(), 0);
    }

    #[test]
    fn debug_never_prints_the_token() {
        let client = TelegramClient::with_http(FakeHttp::new(vec![]), "SECRET_TOKEN_XYZ", None);
        let dumped = format!("{:?}", client);
        assert!(!dumped.contains("SECRET_TOKEN_XYZ"), "debug: {}", dumped);
        assert!(dumped.contains("***"));
    }

    #[test]
    fn new_refuses_a_blank_token() {
        assert!(TelegramClient::<ReqwestHttp>::new("   ", None).is_err());
    }

    #[test]
    fn is_allowed_fails_closed_without_allowlist_or_opt_in() {
        // No allowlist, no opt-in: nobody may use the assistant.
        let closed = TelegramClient::with_http(FakeHttp::new(vec![]), "TOKEN123", None);
        assert!(!closed.is_allowed(100));
        assert!(!closed.is_allowed(999));
        // The explicit operator opt-in opens it to every chat.
        let open = TelegramClient::with_http(FakeHttp::new(vec![]), "TOKEN123", None)
            .with_allow_any_chat(true);
        assert!(open.is_allowed(100));
        assert!(open.is_allowed(999));
        // An allowlist still wins over the opt-in: only that chat.
        let listed = TelegramClient::with_http(FakeHttp::new(vec![]), "TOKEN123", Some(100))
            .with_allow_any_chat(true);
        assert!(listed.is_allowed(100));
        assert!(!listed.is_allowed(999));
    }

    #[test]
    fn send_message_rejects_a_chat_outside_the_allowlist() {
        // Rust P1-4: sen_telegram_send used to bypass the allowlist
        // entirely. A send to any chat but the listed one is refused
        // before any HTTP call is made.
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100));
        let err = client.send_message(999, "hi").unwrap_err();
        assert!(err.contains("TELEGRAM_CHAT_NOT_ALLOWED"), "error: {}", err);
        assert!(http.calls().is_empty(), "no HTTP call for a refused send");
        // The listed chat still sends.
        client.send_message(100, "hi").unwrap();
        assert_eq!(http.calls().len(), 1);
    }

    #[test]
    fn send_message_rejects_every_chat_when_fail_closed() {
        // No allowlist and no opt-in: the bridge serves nobody, and
        // that covers sends too, not just poll.
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", None);
        let err = client.send_message(100, "hi").unwrap_err();
        assert!(err.contains("TELEGRAM_CHAT_NOT_ALLOWED"), "error: {}", err);
        assert!(http.calls().is_empty(), "no HTTP call for a refused send");
    }

    #[test]
    fn send_message_allows_any_chat_only_with_the_explicit_opt_in() {
        let http = FakeHttp::new(vec![]);
        let client =
            TelegramClient::with_http(http.clone(), "TOKEN123", None).with_allow_any_chat(true);
        client.send_message(999, "hi").unwrap();
        assert_eq!(http.calls().len(), 1);
    }

    #[test]
    fn client_send_message_rejects_a_chat_outside_the_allowlist() {
        // Rust P1-4, poller path: this is the call sen_telegram_send
        // makes, so it must refuse a non-listed chat the same way.
        let http = FakeHttp::new(vec![]);
        let client = TelegramClient::with_http(http.clone(), "TOKEN123", Some(100));
        let poller = TelegramPoller::new(client);
        let err = poller.client_send_message(999, "hi").unwrap_err();
        assert!(err.contains("TELEGRAM_CHAT_NOT_ALLOWED"), "error: {}", err);
        assert!(http.calls().is_empty(), "no HTTP call for a refused send");
        poller.client_send_message(100, "hi").unwrap();
        assert_eq!(http.calls().len(), 1);
    }

    #[test]
    fn poll_once_emits_nothing_when_the_bridge_is_fail_closed() {
        let http = FakeHttp::new(vec![updates_json()]);
        let client = TelegramClient::with_http(http, "TOKEN123", None);
        let mut poller = TelegramPoller::new(client);
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        // Nothing emitted, but the offset still moves past the updates.
        assert_eq!(poller.poll_once(&mut |u| seen.push(u)).unwrap(), 0);
        assert!(seen.is_empty());
        assert_eq!(poller.offset(), 14);
    }

    #[test]
    fn transport_errors_never_carry_the_token() {
        // reqwest's error Display embeds the request URL, token
        // included. The client must redact it before the string can
        // travel over IPC into the transcript / webview console.
        let token = "SECRET_TOKEN_XYZ";
        let get_error = format!(
            "Telegram GET failed: error sending request for url (https://api.telegram.org/bot{}/getUpdates?offset=0&timeout=30): network down",
            token
        );
        let http = FakeHttp::with_get_error(&get_error);
        let client = TelegramClient::with_http(http, token, Some(100));
        let err = client.get_updates(0).unwrap_err();
        assert!(!err.contains(token), "error leaks token: {}", err);
        assert!(err.contains("***"), "token must be redacted: {}", err);

        let post_error = format!(
            "Telegram POST failed: error sending request for url (https://api.telegram.org/bot{}/sendMessage): network down",
            token
        );
        let http = FakeHttp::with_post_error(&post_error);
        let client = TelegramClient::with_http(http, token, Some(100));
        let err = client.send_message(100, "hi").unwrap_err();
        assert!(!err.contains(token), "error leaks token: {}", err);
        assert!(err.contains("***"), "token must be redacted: {}", err);
    }

    #[test]
    fn offset_store_round_trips_and_poller_resumes_after_restart() {
        let dir = std::env::temp_dir().join(format!("sen_offset_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("telegram-offset");
        let store = FileOffsetStore::new(path.clone());
        assert_eq!(store.load(), 0, "missing file starts at 0");
        store.save(42).unwrap();
        assert_eq!(FileOffsetStore::new(path.clone()).load(), 42);

        // A poller built on the stored offset starts there, advances,
        // and persists the new offset for the next process.
        let http = FakeHttp::new(vec![updates_json()]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(100));
        let mut poller =
            TelegramPoller::with_offset_store(client, FileOffsetStore::new(path.clone()));
        assert_eq!(poller.offset(), 42);
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        // updates_json carries ids 10-13, all below the stored offset
        // 42: the API would never return them, but the fake does — the
        // offset must still advance past them, never backwards.
        assert_eq!(poller.poll_once(&mut |u| seen.push(u)).unwrap(), 2);
        assert_eq!(poller.offset(), 42);
        assert_eq!(FileOffsetStore::new(path.clone()).load(), 42);

        // Fresh store at 0: the poller advances to 14 and writes it.
        let dir2 = std::env::temp_dir().join(format!("sen_offset2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        let path2 = dir2.join("telegram-offset");
        let http = FakeHttp::new(vec![updates_json()]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(100));
        let mut poller =
            TelegramPoller::with_offset_store(client, FileOffsetStore::new(path2.clone()));
        assert_eq!(poller.offset(), 0);
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        assert_eq!(poller.poll_once(&mut |u| seen.push(u)).unwrap(), 2);
        assert_eq!(poller.offset(), 14);
        assert_eq!(FileOffsetStore::new(path2.clone()).load(), 14);
        // A new poller on the same file resumes at 14, not 0.
        let http = FakeHttp::new(vec![r#"{"ok":true,"result":[]}"#.to_string()]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(100));
        let mut resumed =
            TelegramPoller::with_offset_store(client, FileOffsetStore::new(path2.clone()));
        assert_eq!(resumed.offset(), 14);
        assert_eq!(resumed.poll_once(&mut |_| {}).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn poll_once_dedupes_a_duplicate_update_id_in_one_round() {
        let dup_json = r#"{"ok":true,"result":[
          {"update_id":10,"message":{"message_id":1,"from":{"id":7},"chat":{"id":100},"text":"hello"}},
          {"update_id":10,"message":{"message_id":1,"from":{"id":7},"chat":{"id":100},"text":"hello"}}
        ]}"#;
        let http = FakeHttp::new(vec![dup_json.to_string()]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(100));
        let mut poller = TelegramPoller::new(client);
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        assert_eq!(poller.poll_once(&mut |u| seen.push(u)).unwrap(), 1);
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn offset_write_failure_fails_the_round_and_keeps_the_offset() {
        // Store path under a regular file: creating the parent dir
        // fails, so the offset can never be written.
        let dir = std::env::temp_dir().join(format!("sen_offset3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "not a dir").unwrap();
        let path = blocker.join("telegram-offset");
        let http = FakeHttp::new(vec![updates_json()]);
        let client = TelegramClient::with_http(http, "TOKEN123", Some(100));
        let mut poller = TelegramPoller::with_offset_store(client, FileOffsetStore::new(path));
        let mut seen: Vec<TelegramUpdate> = Vec::new();
        assert!(poller.poll_once(&mut |u| seen.push(u)).is_err());
        assert!(seen.is_empty(), "nothing emitted on a failed round");
        assert_eq!(poller.offset(), 0, "offset stays: next round re-fetches");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
