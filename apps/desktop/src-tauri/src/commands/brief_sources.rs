// Gmail + Google Calendar sources for /brief (Tauri follow-up 2,
// ZC Leader tin 17093). PURE RUST, no Tauri dependency: compiles
// and tests standalone in the scratch harness.
//
// The webview has no Gmail/calendar fetch, so /brief refused
// honestly ("Chưa nối nguồn email/lịch"). This module is the real
// source, read on the Rust side: it calls the Google REST APIs
// (Gmail messages.list + messages.get, Calendar events.list) with
// reqwest blocking, and returns the plain EmailItem / CalendarEvent
// data packages/core/src/morning-brief.ts builds the brief from.
// The brief itself still runs in TS (buildMorningBrief); this
// module only fetches, never synthesizes, never acts.
//
// Auth: a Google OAuth access token from the environment
// (SEN_GMAIL_ACCESS_TOKEN), the same env-token pattern as
// SEN_TELEGRAM_BOT_TOKEN. The token travels only in the
// Authorization: Bearer header — never in a URL (unlike the
// Telegram token, which is in the path), never in a log line, and
// never in a Debug dump (the Debug impl redacts it). No token is
// stored anywhere: the wrapper reads the env var per call, and an
// unset/blank token fails honestly as BRIEF_SOURCE_NOT_CONFIGURED
// instead of fetching anything. Obtaining the token (Google OAuth
// consent) is Zen-side work: this module never starts an OAuth
// flow, never opens a browser, never asks for a credential.
//
// The HTTP transport is generic (BriefHttp) so tests inject a fake:
// the production transport is ReqwestBriefHttp (reqwest blocking,
// rustls). Base URLs are overridable (SEN_GOOGLE_API_BASE_URL, one
// prefix for both APIs in tests); production defaults are the
// Google endpoints. A failed fetch fails the whole source loudly:
// a partial brief that silently omits emails would be a made-up
// brief, so no partial result is ever returned.
//
// What the fetch reads, and why:
// - Gmail list q=newer_than:7d, maxResults 20: the brief window is
//   7 days, and the list returns newest first. Each message is then
//   fetched format=full: From/Subject headers, internalDate (epoch
//   ms, unambiguous — no RFC 2822 date parsing) for receivedAt, and
//   the text/plain part (base64url) for the body, falling back to
//   the snippet when a message has no text part. needsDecision is
//   left unset: TS buildMorningBrief derives it from
//   DECISION_MARKERS, and this module never guesses it.
// - Calendar events.list on primary, timeMin=now,
//   timeMax=now+7d, singleEvents=true, orderBy=startTime,
//   maxResults 50: the same 7-day window, recurring events
//   expanded. start/end pass through as the API returns them
//   (dateTime or all-day date), both valid ISO for the TS
//   validator.
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::collections::HashMap;

/// Env var holding the Google OAuth access token. Unset/blank means
/// the brief source is not configured (the chat still works, /brief
/// refuses honestly).
pub const ENV_GMAIL_TOKEN: &str = "SEN_GMAIL_ACCESS_TOKEN";

/// Env var holding one base-URL prefix for both Google APIs in
/// tests (e.g. a local fake server). Unset means the production
/// Google endpoints.
pub const ENV_GOOGLE_API_BASE_URL: &str = "SEN_GOOGLE_API_BASE_URL";

/// Production Gmail API base URL.
pub const DEFAULT_GMAIL_BASE_URL: &str = "https://gmail.googleapis.com";
/// Production Google Calendar API base URL.
pub const DEFAULT_CALENDAR_BASE_URL: &str = "https://www.googleapis.com";

/// Read timeout for one Google API round (seconds).
pub const BRIEF_HTTP_TIMEOUT_SECS: u64 = 30;

/// Error codes for the brief source (same shape as BridgeError).
pub const ERR_BRIEF_NOT_CONFIGURED: &str = "BRIEF_SOURCE_NOT_CONFIGURED";
pub const ERR_BRIEF_HTTP: &str = "BRIEF_SOURCE_HTTP";
pub const ERR_BRIEF_INVALID: &str = "BRIEF_SOURCE_INVALID";

/// Gmail list window: the brief window is 7 days, so only messages
/// from the last 7 days are listed.
pub const GMAIL_MAX_RESULTS: u32 = 20;
/// Calendar list cap: at most this many events in the 7-day window.
pub const CALENDAR_MAX_RESULTS: u32 = 50;
/// Body cap (chars): a message body longer than this is cut — the
/// brief matches decision markers on the head of the body, and an
/// unbounded body would bloat the IPC payload.
pub const EMAIL_BODY_MAX_CHARS: usize = 20000;

/// One email item, wire-identical to the TS EmailItem
/// (packages/core/src/morning-brief.ts): camelCase, needsDecision
/// omitted when unset (TS derives it from DECISION_MARKERS then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailItem {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub received_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_decision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<String>,
}

/// One calendar event, wire-identical to the TS CalendarEvent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The brief source payload: what the Tauri command returns and the
/// TS briefSource consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefData {
    pub emails: Vec<EmailItem>,
    pub events: Vec<CalendarEvent>,
}

/// Typed brief-source error: a code the frontend can match on, plus
/// a message that never contains the token (the token is only ever
/// in a header, never in a URL or a body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefError {
    pub code: String,
    pub message: String,
}

impl BriefError {
    pub fn new(code: &str, message: &str) -> BriefError {
        BriefError {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

/// Minimal HTTP surface the source needs: GET with a Bearer token,
///
/// A non-2xx status is an Err carrying the status, never a fake
/// body.
pub trait BriefHttp: Send + Sync {
    fn get(&self, url: &str, bearer_token: &str) -> Result<String, String>;
}

/// Production transport: reqwest blocking, rustls, one client per
/// source (reqwest clients are cheap to clone and share a pool).
#[derive(Debug, Clone)]
pub struct ReqwestBriefHttp {
    client: reqwest::blocking::Client,
}

impl ReqwestBriefHttp {
    pub fn new() -> Result<ReqwestBriefHttp, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(BRIEF_HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("Cannot build brief HTTP client: {}", e))?;
        Ok(ReqwestBriefHttp { client })
    }
}

impl BriefHttp for ReqwestBriefHttp {
    fn get(&self, url: &str, bearer_token: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {}", bearer_token))
            .send()
            .map_err(|e| format!("Brief GET failed: {}", e))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|e| format!("Brief GET body unreadable: {}", e))?;
        if !status.is_success() {
            return Err(format!("Brief GET HTTP {}: {}", status, body));
        }
        Ok(body)
    }
}

/// Where the source fetches from, and with what token. The token is
/// held here only for the duration of one fetch and is redacted in
/// Debug (same rule as the Telegram client).
pub struct BriefSourceConfig {
    pub gmail_base_url: String,
    pub calendar_base_url: String,
    pub token: String,
}

impl BriefSourceConfig {
    /// Build from the environment: SEN_GMAIL_ACCESS_TOKEN (blank
    /// means not configured) and the optional
    /// SEN_GOOGLE_API_BASE_URL test override. Production callers use
    /// this; tests build the struct directly.
    pub fn from_env() -> Result<BriefSourceConfig, BriefError> {
        let token = std::env::var(ENV_GMAIL_TOKEN).unwrap_or_default();
        if token.trim().is_empty() {
            return Err(BriefError::new(
                ERR_BRIEF_NOT_CONFIGURED,
                "Brief source is not configured: set SEN_GMAIL_ACCESS_TOKEN (Google OAuth access token) before /brief can fetch Gmail/calendar.",
            ));
        }
        let (gmail_base_url, calendar_base_url) = match std::env::var(ENV_GOOGLE_API_BASE_URL) {
            Ok(base) if !base.trim().is_empty() => {
                let base = base.trim_end_matches('/').to_string();
                (base.clone(), base)
            }
            _ => (
                DEFAULT_GMAIL_BASE_URL.to_string(),
                DEFAULT_CALENDAR_BASE_URL.to_string(),
            ),
        };
        Ok(BriefSourceConfig {
            gmail_base_url,
            calendar_base_url,
            token,
        })
    }
}

impl std::fmt::Debug for BriefSourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BriefSourceConfig")
            .field("gmail_base_url", &self.gmail_base_url)
            .field("calendar_base_url", &self.calendar_base_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The brief source: one fetch = one Gmail list + one Calendar
/// list (+ one Gmail get per listed message). No background thread,
/// no cache: the caller (the Tauri command) fetches fresh per
/// /brief call.
pub struct BriefSource<H: BriefHttp> {
    http: H,
    config: BriefSourceConfig,
}

impl<H: BriefHttp> BriefSource<H> {
    pub fn new(http: H, config: BriefSourceConfig) -> BriefSource<H> {
        BriefSource { http, config }
    }

    fn get_json(&self, url: &str) -> Result<serde_json::Value, BriefError> {
        let body = self
            .http
            .get(url, &self.config.token)
            .map_err(|e| BriefError::new(ERR_BRIEF_HTTP, &e))?;
        serde_json::from_str(&body).map_err(|e| {
            BriefError::new(
                ERR_BRIEF_INVALID,
                &format!("Brief source returned invalid JSON from {}: {}", url, e),
            )
        })
    }

    /// Fetch the brief source now: unread-window emails + the
    /// 7-day calendar window. `now` is the window start (the
    /// caller passes SystemTime::now()); tests pass a fixed time so
    /// the Calendar URLs are deterministic.
    pub fn fetch(&self, now: SystemTime) -> Result<BriefData, BriefError> {
        let emails = self.fetch_emails()?;
        let events = self.fetch_events(now)?;
        Ok(BriefData { emails, events })
    }

    fn fetch_emails(&self) -> Result<Vec<EmailItem>, BriefError> {
        let url = format!(
            "{}/gmail/v1/users/me/messages?q=newer_than%3A7d&maxResults={}",
            self.config.gmail_base_url, GMAIL_MAX_RESULTS
        );
        let list = self.get_json(&url)?;
        let messages = list
            .get("messages")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                BriefError::new(
                    ERR_BRIEF_INVALID,
                    "Gmail messages.list returned no messages array",
                )
            })?;
        let mut emails: Vec<EmailItem> = Vec::new();
        for item in messages {
            let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            emails.push(self.fetch_email(id)?);
        }
        Ok(emails)
    }

    fn fetch_email(&self, id: &str) -> Result<EmailItem, BriefError> {
        let url = format!(
            "{}/gmail/v1/users/me/messages/{}?format=full",
            self.config.gmail_base_url, id
        );
        let value = self.get_json(&url)?;
        let internal_date = value
            .get("internalDate")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| {
                BriefError::new(
                    ERR_BRIEF_INVALID,
                    &format!("Gmail message {} has no internalDate", id),
                )
            })?;
        let headers = value
            .get("payload")
            .and_then(|p| p.get("headers"))
            .and_then(|h| h.as_array());
        let header = |name: &str| -> String {
            headers
                .into_iter()
                .flatten()
                .find(|h| {
                    h.get("name")
                        .and_then(|n| n.as_str())
                        .map(|n| n.eq_ignore_ascii_case(name))
                        .unwrap_or(false)
                })
                .and_then(|h| h.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let mut body = value
            .get("payload")
            .map(extract_text_body)
            .unwrap_or_default();
        if body.trim().is_empty() {
            body = value
                .get("snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        }
        if body.chars().count() > EMAIL_BODY_MAX_CHARS {
            body = body.chars().take(EMAIL_BODY_MAX_CHARS).collect();
        }
        Ok(EmailItem {
            id: id.to_string(),
            from: header("From"),
            subject: header("Subject"),
            body,
            received_at: epoch_ms_to_iso(internal_date),
            needs_decision: None,
            due_at: None,
        })
    }

    fn fetch_events(&self, now: SystemTime) -> Result<Vec<CalendarEvent>, BriefError> {
        let now_ms = system_time_ms(now);
        let time_min = epoch_ms_to_iso(now_ms);
        let time_max = epoch_ms_to_iso(now_ms + 7 * 24 * 60 * 60 * 1000);
        let url = format!(
            "{}/calendar/v3/calendars/primary/events?timeMin={}&timeMax={}&singleEvents=true&orderBy=startTime&maxResults={}",
            self.config.calendar_base_url,
            url_encode(&time_min),
            url_encode(&time_max),
            CALENDAR_MAX_RESULTS
        );
        let value = self.get_json(&url)?;
        let items = value
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                BriefError::new(
                    ERR_BRIEF_INVALID,
                    "Calendar events.list returned no items array",
                )
            })?;
        let mut events: Vec<CalendarEvent> = Vec::new();
        for item in items {
            let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let start = item
                .get("start")
                .and_then(|s| s.get("dateTime").or_else(|| s.get("date")))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let end = item
                .get("end")
                .and_then(|s| s.get("dateTime").or_else(|| s.get("date")))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let attendees = item
                .get("attendees")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|a| a.get("email").and_then(|e| e.as_str()).map(str::to_string))
                        .collect::<Vec<String>>()
                });
            events.push(CalendarEvent {
                id: id.to_string(),
                title: item
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                start,
                end,
                attendees,
                location: item
                    .get("location")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                description: item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
        Ok(events)
    }
}

fn system_time_ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Minimal URL encoding for the timeMin/timeMax query values
/// (ISO timestamps): only the characters that must not appear raw
/// in a query are escaped. The Gmail `q` value is already encoded
/// in its format string.
fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    out.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    out
}

/// Format epoch milliseconds as an ISO 8601 UTC timestamp
/// (millis precision). Civil-from-days algorithm (Howard Hinnant):
/// no chrono dependency, and the conversion is pinned by tests
/// against `date -u`.
pub fn epoch_ms_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, minute, second, millis
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Decode base64url (RFC 4648 §5: `-`/`_` alphabet, no padding) to
/// bytes. Returns Err on a bad length or a bad character. Invalid
/// UTF-8 after decoding is lossy-converted by the caller.
pub fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let mut values: Vec<u8> = Vec::with_capacity(clean.len());
    for ch in clean.chars() {
        let v = match ch {
            'A'..='Z' => ch as u8 - b'A',
            'a'..='z' => ch as u8 - b'a' + 26,
            '0'..='9' => ch as u8 - b'0' + 52,
            '-' => 62,
            '_' => 63,
            _ => return Err(format!("invalid base64url character: {}", ch)),
        };
        values.push(v);
    }
    if values.len() % 4 == 1 {
        return Err("invalid base64url length".to_string());
    }
    let mut out: Vec<u8> = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        match chunk.len() {
            4 => {
                out.push((chunk[0] << 2) | (chunk[1] >> 4));
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
                out.push((chunk[2] << 6) | chunk[3]);
            }
            3 => {
                out.push((chunk[0] << 2) | (chunk[1] >> 4));
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
            }
            2 => {
                out.push((chunk[0] << 2) | (chunk[1] >> 4));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Extract the text/plain body from a Gmail message payload
/// (format=full): recurse into multipart parts for the first
/// text/plain part; fall back to the payload's own body.data
/// (single-part messages). An undecodable part decodes lossy
/// rather than failing the whole message.
pub fn extract_text_body(payload: &serde_json::Value) -> String {
    fn decode_data(value: &serde_json::Value) -> Option<String> {
        value
            .get("body")
            .and_then(|b| b.get("data"))
            .and_then(|d| d.as_str())
            .and_then(|s| base64url_decode(s).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
    }
    let mime = payload
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if mime == "text/plain" {
        if let Some(text) = decode_data(payload) {
            return text;
        }
    }
    if let Some(parts) = payload.get("parts").and_then(|p| p.as_array()) {
        for part in parts {
            let text = extract_text_body(part);
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    // No plain part found. Only a single-part message with no
    // declared mimeType still carries usable text here; a declared
    // non-plain type (e.g. text/html) is not body text — returning
    // its markup would pollute the brief, so it yields "" and the
    // caller falls back to the snippet.
    if mime.is_empty() {
        return decode_data(payload).unwrap_or_default();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHttp {
        responses: HashMap<String, String>,
        error: Option<String>,
        seen_urls: std::sync::Mutex<Vec<String>>,
    }

    impl FakeHttp {
        fn new(responses: HashMap<String, String>) -> FakeHttp {
            FakeHttp {
                responses,
                error: None,
                seen_urls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn failing(message: &str) -> FakeHttp {
            FakeHttp {
                responses: HashMap::new(),
                error: Some(message.to_string()),
                seen_urls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl BriefHttp for FakeHttp {
        fn get(&self, url: &str, _bearer_token: &str) -> Result<String, String> {
            self.seen_urls.lock().unwrap().push(url.to_string());
            if let Some(err) = &self.error {
                return Err(err.clone());
            }
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| format!("FakeHttp: no response for {}", url))
        }
    }

    fn test_config() -> BriefSourceConfig {
        BriefSourceConfig {
            gmail_base_url: "https://gmail.test".to_string(),
            calendar_base_url: "https://calendar.test".to_string(),
            token: "test-token".to_string(),
        }
    }

    /// Fixed "now": 2026-11-19T05:00:00.000Z (12:00 +07).
    fn test_now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(1795064400000)
    }

    fn message_json(
        id: &str,
        internal_date: &str,
        from: &str,
        subject: &str,
        body_b64: &str,
    ) -> String {
        serde_json::json!({
            "id": id,
            "internalDate": internal_date,
            "snippet": "snip",
            "payload": {
                "mimeType": "multipart/alternative",
                "headers": [
                    {"name": "From", "value": from},
                    {"name": "Subject", "value": subject},
                    {"name": "Date", "value": "Thu, 19 Nov 2026 12:00:00 +0700"}
                ],
                "parts": [
                    {"mimeType": "text/plain", "body": {"data": body_b64}},
                    {"mimeType": "text/html", "body": {"data": "PGgxPmgp"}}
                ]
            }
        })
        .to_string()
    }

    #[test]
    fn epoch_ms_to_iso_matches_date_u() {
        // Verified with `date -u -d @<secs>`: 0 is the epoch, and
        // 1795064400 is 2026-11-19 05:00:00 UTC.
        assert_eq!(epoch_ms_to_iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(epoch_ms_to_iso(1795064400000), "2026-11-19T05:00:00.000Z");
        assert_eq!(epoch_ms_to_iso(1795064400123), "2026-11-19T05:00:00.123Z");
        assert_eq!(epoch_ms_to_iso(-1000), "1969-12-31T23:59:59.000Z");
        assert_eq!(epoch_ms_to_iso(1735689600000), "2025-01-01T00:00:00.000Z");
    }

    #[test]
    fn base64url_decode_vectors() {
        assert_eq!(base64url_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64url_decode("aGVsbG8td29ybGQ").unwrap(), b"hello-world");
        // URL-safe alphabet: '-' is 62, '_' is 63.
        assert_eq!(base64url_decode("AB-_").unwrap(), vec![0x00, 0x1F, 0xBF]);
        assert!(base64url_decode("a").is_err());
        assert!(base64url_decode("aGVsbG8!").is_err());
    }

    #[test]
    fn extract_text_body_prefers_plain_part() {
        let payload: serde_json::Value = serde_json::from_str(
            r#"{"mimeType":"multipart/alternative","parts":[
                {"mimeType":"text/plain","body":{"data":"aGVsbG8"}},
                {"mimeType":"text/html","body":{"data":"PGgxPmgp"}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(extract_text_body(&payload), "hello");
        let single: serde_json::Value =
            serde_json::from_str(r#"{"mimeType":"text/plain","body":{"data":"d29ybGQ"}}"#).unwrap();
        assert_eq!(extract_text_body(&single), "world");
        // No text part at all: empty, and the caller falls back to
        // the snippet.
        let html_only: serde_json::Value = serde_json::from_str(
            r#"{"mimeType":"multipart/alternative","parts":[
                {"mimeType":"text/html","body":{"data":"PGgxPmgp"}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(extract_text_body(&html_only), "");
    }

    #[test]
    fn fetch_happy_path_reads_gmail_and_calendar() {
        let mut responses = HashMap::new();
        responses.insert(
            "https://gmail.test/gmail/v1/users/me/messages?q=newer_than%3A7d&maxResults=20"
                .to_string(),
            r#"{"messages":[{"id":"m1"},{"id":"m2"}],"resultSizeEstimate":2}"#.to_string(),
        );
        responses.insert(
            "https://gmail.test/gmail/v1/users/me/messages/m1?format=full".to_string(),
            message_json(
                "m1",
                "1795060800000",
                "sep@example.com",
                "Vui lòng phê duyệt ngân sách",
                "SGVsbG8gd29ybGQ",
            ),
        );
        responses.insert(
            "https://gmail.test/gmail/v1/users/me/messages/m2?format=full".to_string(),
            message_json(
                "m2",
                "1795057200000",
                "boss@example.com",
                "Lịch họp",
                "TW9yZSB0ZXh0",
            ),
        );
        responses.insert(
            "https://calendar.test/calendar/v3/calendars/primary/events?timeMin=2026-11-19T05%3A00%3A00.000Z&timeMax=2026-11-26T05%3A00%3A00.000Z&singleEvents=true&orderBy=startTime&maxResults=50".to_string(),
            r#"{"items":[
                {"id":"e1","summary":"Họp đội","start":{"dateTime":"2026-11-19T09:00:00+07:00"},"end":{"dateTime":"2026-11-19T10:00:00+07:00"},"attendees":[{"email":"a@example.com"}],"location":"Phòng 1"},
                {"id":"e2","summary":"Nghỉ","start":{"date":"2026-11-20"},"end":{"date":"2026-11-21"}}
            ]}"#.to_string(),
        );
        let source = BriefSource::new(FakeHttp::new(responses), test_config());
        let data = source.fetch(test_now()).unwrap();
        assert_eq!(data.emails.len(), 2);
        assert_eq!(data.emails[0].id, "m1");
        assert_eq!(data.emails[0].from, "sep@example.com");
        assert_eq!(data.emails[0].subject, "Vui lòng phê duyệt ngân sách");
        assert_eq!(data.emails[0].body, "Hello world");
        assert_eq!(data.emails[0].received_at, "2026-11-19T04:00:00.000Z");
        assert_eq!(data.emails[0].needs_decision, None);
        assert_eq!(data.emails[1].body, "More text");
        assert_eq!(data.events.len(), 2);
        assert_eq!(data.events[0].title, "Họp đội");
        assert_eq!(data.events[0].start, "2026-11-19T09:00:00+07:00");
        assert_eq!(
            data.events[0].attendees,
            Some(vec!["a@example.com".to_string()])
        );
        assert_eq!(data.events[0].location, Some("Phòng 1".to_string()));
        // All-day event: date passes through as-is.
        assert_eq!(data.events[1].start, "2026-11-20");
        // The fetched data is exactly what the TS brief builder
        // takes: validate it with the same wire shape.
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("receivedAt"));
        assert!(!json.contains("needsDecision"));
    }

    #[test]
    fn fetch_fails_loud_on_http_error_and_bad_json() {
        let source = BriefSource::new(
            FakeHttp::failing("Brief GET HTTP 401: {\"error\": \"invalid_token\"}"),
            test_config(),
        );
        let err = source.fetch(test_now()).unwrap_err();
        assert_eq!(err.code, ERR_BRIEF_HTTP);
        // The token never appears in the error: it travels in a
        // header, never in a URL or a body.
        assert!(!err.message.contains("test-token"));

        let mut responses = HashMap::new();
        responses.insert(
            "https://gmail.test/gmail/v1/users/me/messages?q=newer_than%3A7d&maxResults=20"
                .to_string(),
            "not json".to_string(),
        );
        let source = BriefSource::new(FakeHttp::new(responses), test_config());
        let err = source.fetch(test_now()).unwrap_err();
        assert_eq!(err.code, ERR_BRIEF_INVALID);
    }

    #[test]
    fn fetch_fails_when_list_has_no_messages_array() {
        let mut responses = HashMap::new();
        responses.insert(
            "https://gmail.test/gmail/v1/users/me/messages?q=newer_than%3A7d&maxResults=20"
                .to_string(),
            r#"{"resultSizeEstimate":0}"#.to_string(),
        );
        let source = BriefSource::new(FakeHttp::new(responses), test_config());
        let err = source.fetch(test_now()).unwrap_err();
        assert_eq!(err.code, ERR_BRIEF_INVALID);
    }

    #[test]
    fn from_env_requires_a_token() {
        // This test owns the SEN_GMAIL_ACCESS_TOKEN var for its
        // duration; no other test in this module reads it.
        std::env::remove_var(ENV_GMAIL_TOKEN);
        let err = BriefSourceConfig::from_env().unwrap_err();
        assert_eq!(err.code, ERR_BRIEF_NOT_CONFIGURED);
        std::env::set_var(ENV_GMAIL_TOKEN, "tok");
        let config = BriefSourceConfig::from_env().unwrap();
        assert_eq!(config.token, "tok");
        assert_eq!(config.gmail_base_url, DEFAULT_GMAIL_BASE_URL);
        assert_eq!(config.calendar_base_url, DEFAULT_CALENDAR_BASE_URL);
        std::env::set_var(ENV_GOOGLE_API_BASE_URL, "https://fake.test/");
        let config = BriefSourceConfig::from_env().unwrap();
        assert_eq!(config.gmail_base_url, "https://fake.test");
        std::env::remove_var(ENV_GOOGLE_API_BASE_URL);
        std::env::remove_var(ENV_GMAIL_TOKEN);
    }
}
