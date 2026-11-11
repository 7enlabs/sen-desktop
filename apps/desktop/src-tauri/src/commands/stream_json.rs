// stream-json event parsing + translation (Phase 2, tin 9530 quyết 4).
// PURE RUST, no Tauri dependency: compiles and tests standalone.
//
// Model: DV-3 / B2 (packages/core/src/cli-bridge.ts in the fix round,
// learned, not copied). Record shapes come from the CLI emitter itself:
// StreamJsonSink in crates/zen-cli/src/output.rs and docs/stream-json-v1.md
// at the pinned CLI rev 8ac73fe (the checkout at
// ~/workspace/cli-core-main is that rev; `zen --version` prints 0.1.0).
//
// Contract (stream-json v1, additive only):
// - NDJSON on stdout, one JSON object per line, `v` = 1 on every record.
// - Record types: system, assistant, user, stream_event, result. System
//   records carry a subtype (init, tool_progress, api_retry,
//   compact_boundary, prune, permission_denied, hook_message).
// - `result` is always last on a normal end, an error, or SIGINT.
// - Unknown record types and unknown fields may be added in a minor
//   release: they are counted, never fatal. An event type this module
//   does not know becomes SenEvent::Unknown; the bridge counts it, and
//   the counts travel to the frontend with the session result
//   (SessionResult.unknown_records / version_mismatches, R2B-04).
// - No key is ever in a record. Records carry model text, which the
//   caller may log truncated, never the full raw line blindly.
use serde::{Deserialize, Serialize};

/// Stream-json schema version (STREAM_JSON_VERSION in zen-cli output.rs).
pub const STREAM_JSON_VERSION: u32 = 1;

/// Max chars of a malformed line kept in an error message (the line can
/// hold model text; keep the error short).
pub const LINE_SNIPPET_LEN: usize = 200;

/// Truncate a line to at most `max` characters for an error message.
/// Char-based, not byte-based: slicing a &str by byte index panics when
/// the cut lands inside a multi-byte character, and stream-json lines
/// hold model text (often non-ASCII).
pub fn snippet(line: &str, max: usize) -> &str {
    match line.char_indices().nth(max) {
        Some((i, _)) => &line[..i],
        None => line,
    }
}

/// One CLI stream-json record. Unknown fields are ignored: the contract
/// is additive only.
#[derive(Debug, Clone, Deserialize)]
pub struct CliRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub v: Option<u32>,
    pub uuid: Option<String>,
    pub session_id: Option<String>,
    pub subtype: Option<String>,
    pub message: Option<serde_json::Value>,
    pub event: Option<serde_json::Value>,
    pub parent_tool_use_id: Option<serde_json::Value>,
    pub result: Option<String>,
    pub is_error: Option<bool>,
    pub duration_ms: Option<u64>,
    pub duration_api_ms: Option<u64>,
    pub num_turns: Option<u32>,
    pub usage: Option<serde_json::Value>,
    pub model: Option<String>,
    pub stop_reason: Option<serde_json::Value>,
    pub permission_denials: Option<serde_json::Value>,
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
}

/// Sen event derived from one CLI record. Serialized to the frontend over
/// the Tauri event `sen-session-event` with `type` as the tag.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SenEvent {
    SessionStart {
        session_id: String,
        model: String,
    },
    Message {
        session_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<serde_json::Value>,
    },
    MessageUpdate {
        session_id: String,
        delta: String,
    },
    ToolResult {
        session_id: String,
        tool_use_id: String,
        content: serde_json::Value,
        is_error: bool,
    },
    System {
        session_id: String,
        subtype: String,
    },
    Result {
        session_id: String,
        subtype: String,
        is_error: bool,
        text: String,
        num_turns: u64,
        usage: TurnUsage,
        duration_ms: u64,
        model: String,
    },
    /// A record type this bridge does not know. The caller counts it;
    /// the count travels to the frontend with the session result
    /// (SessionResult.unknown_records, R2B-04); the turn continues.
    Unknown {
        session_id: String,
        record_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subtype: Option<String>,
    },
}

/// Parse one NDJSON line. Blank lines are skipped (Ok(None)). A line that
/// is not JSON, not an object, or has no string `type` is a real failure:
/// the error carries the offending line truncated, never the whole line.
pub fn parse_stream_json_line(line: &str) -> Result<Option<CliRecord>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => {
            return Err(format!(
                "Invalid stream-json record, FAIL: not JSON: {}",
                snippet(trimmed, LINE_SNIPPET_LEN)
            ));
        }
    };
    if !parsed.is_object() {
        return Err(format!(
            "Invalid stream-json record, FAIL: not an object: {}",
            snippet(trimmed, LINE_SNIPPET_LEN)
        ));
    }
    let record_type = parsed.get("type").and_then(|v| v.as_str());
    if record_type.is_none_or(|t| t.is_empty()) {
        return Err(format!(
            "Invalid stream-json record, FAIL: missing string type: {}",
            snippet(trimmed, LINE_SNIPPET_LEN)
        ));
    }
    match serde_json::from_value::<CliRecord>(parsed) {
        Ok(record) => Ok(Some(record)),
        Err(e) => Err(format!(
            "Invalid stream-json record, FAIL: {}: {}",
            e,
            snippet(trimmed, LINE_SNIPPET_LEN)
        )),
    }
}

fn assistant_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(blocks) = content.as_array() {
        let mut parts: Vec<String> = Vec::new();
        for block in blocks {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
        return parts.join("");
    }
    String::new()
}

fn session_id_of(record: &CliRecord) -> String {
    record.session_id.clone().unwrap_or_default()
}

fn usage_of(record: &CliRecord) -> TurnUsage {
    let usage = record.usage.clone().unwrap_or(serde_json::Value::Null);
    TurnUsage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read_input_tokens: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    }
}

/// Translate one CLI record into a Sen event. Unknown record types return
/// SenEvent::Unknown (the caller counts them); the additive-only contract
/// means a new record type must never break the bridge. Missing optional
/// fields degrade to empty values, never a crash.
pub fn translate_cli_record(record: &CliRecord) -> SenEvent {
    let session_id = session_id_of(record);
    match record.record_type.as_str() {
        "system" => {
            let subtype = record.subtype.clone().unwrap_or_default();
            if subtype == "init" {
                SenEvent::SessionStart {
                    session_id,
                    model: record.model.clone().unwrap_or_default(),
                }
            } else {
                SenEvent::System {
                    session_id,
                    subtype,
                }
            }
        }
        "assistant" => {
            let message = record.message.clone().unwrap_or(serde_json::Value::Null);
            let text = assistant_text(message.get("content").unwrap_or(&serde_json::Value::Null));
            let tool_calls = message.get("tool_calls").cloned();
            SenEvent::Message {
                session_id,
                text,
                tool_calls,
            }
        }
        "user" => {
            let message = record.message.clone().unwrap_or(serde_json::Value::Null);
            let mut tool_use_id = String::new();
            let mut tool_content = serde_json::Value::Null;
            let mut is_error = false;
            if let Some(blocks) = message.get("content").and_then(|v| v.as_array()) {
                for block in blocks {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        if let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) {
                            tool_use_id = id.to_string();
                        }
                        tool_content = block
                            .get("content")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        break;
                    }
                }
            }
            if tool_use_id.is_empty() {
                if let Some(parent) = record.parent_tool_use_id.as_ref().and_then(|v| v.as_str()) {
                    tool_use_id = parent.to_string();
                }
            }
            SenEvent::ToolResult {
                session_id,
                tool_use_id,
                content: tool_content,
                is_error,
            }
        }
        "stream_event" => {
            // Only emitted with --include-partial-messages, which this
            // bridge never passes. Translated defensively from the
            // documented shape event.delta.text; any other shape yields
            // an empty delta, never a crash.
            let delta = record
                .event
                .as_ref()
                .and_then(|e| e.get("delta"))
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            SenEvent::MessageUpdate { session_id, delta }
        }
        "result" => SenEvent::Result {
            session_id,
            subtype: record.subtype.clone().unwrap_or_default(),
            is_error: record.is_error.unwrap_or(false),
            text: record.result.clone().unwrap_or_default(),
            num_turns: record.num_turns.map(u64::from).unwrap_or(0),
            usage: usage_of(record),
            duration_ms: record.duration_ms.unwrap_or(0),
            model: record.model.clone().unwrap_or_default(),
        },
        other => SenEvent::Unknown {
            session_id,
            record_type: other.to_string(),
            subtype: record.subtype.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(json: &str) -> CliRecord {
        parse_stream_json_line(json).unwrap().unwrap()
    }

    #[test]
    fn parse_blank_lines_are_skipped() {
        assert!(parse_stream_json_line("").unwrap().is_none());
        assert!(parse_stream_json_line("   \n").unwrap().is_none());
    }

    #[test]
    fn parse_rejects_non_json_and_missing_type() {
        assert!(parse_stream_json_line("not json").is_err());
        assert!(parse_stream_json_line("[1,2]").is_err());
        assert!(parse_stream_json_line("{\"v\":1}").is_err());
        assert!(parse_stream_json_line("{\"type\":\"\"}").is_err());
    }

    #[test]
    fn parse_rejects_non_json_multibyte_text_without_panicking() {
        // A non-JSON line of multi-byte chars: byte 200 lands inside a
        // character ("x" + "é" x 150: byte 200 is the second byte of one
        // "é"), so the old byte-slice truncation panicked here. The
        // error must come back as Err, never a panic.
        let line = "x".to_string() + &"é".repeat(150);
        assert!(line.len() > LINE_SNIPPET_LEN);
        assert!(parse_stream_json_line(&line).is_err());
    }

    #[test]
    fn translate_init_and_system_subtypes() {
        let ev = translate_cli_record(&record(
            r#"{"type":"system","subtype":"init","session_id":"s1","cwd":"/tmp","model":"m:free","tools":[],"mcp_servers":[],"permissionMode":"default","slash_commands":[],"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::SessionStart {
                session_id: "s1".to_string(),
                model: "m:free".to_string(),
            }
        );
        let ev = translate_cli_record(&record(
            r#"{"type":"system","subtype":"tool_progress","session_id":"s1","tool_use_id":"t1","tool_name":"read","elapsed_ms":42,"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::System {
                session_id: "s1".to_string(),
                subtype: "tool_progress".to_string(),
            }
        );
        let ev = translate_cli_record(&record(
            r#"{"type":"system","subtype":"api_retry","session_id":"s1","attempt":1,"max_retries":3,"retry_delay_ms":100,"error_status":429,"error":"slow down","v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::System {
                session_id: "s1".to_string(),
                subtype: "api_retry".to_string(),
            }
        );
    }

    #[test]
    fn translate_assistant_text_and_tool_calls() {
        let ev = translate_cli_record(&record(
            r#"{"type":"assistant","session_id":"s1","message":{"role":"assistant","content":"hello"},"parent_tool_use_id":null,"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::Message {
                session_id: "s1".to_string(),
                text: "hello".to_string(),
                tool_calls: None,
            }
        );
        let ev = translate_cli_record(&record(
            r#"{"type":"assistant","session_id":"s1","message":{"role":"assistant","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},"parent_tool_use_id":null,"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::Message {
                session_id: "s1".to_string(),
                text: "ab".to_string(),
                tool_calls: None,
            }
        );
    }

    #[test]
    fn translate_user_tool_result_and_stream_event() {
        let ev = translate_cli_record(&record(
            r#"{"type":"user","session_id":"s1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t9","content":"ok","is_error":false,"parallel":false}]},"parent_tool_use_id":null,"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::ToolResult {
                session_id: "s1".to_string(),
                tool_use_id: "t9".to_string(),
                content: serde_json::json!("ok"),
                is_error: false,
            }
        );
        let ev = translate_cli_record(&record(
            r#"{"type":"stream_event","session_id":"s1","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hel"}} ,"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::MessageUpdate {
                session_id: "s1".to_string(),
                delta: "hel".to_string(),
            }
        );
        // A stream_event of an unexpected shape degrades to an empty
        // delta, never a crash.
        let ev = translate_cli_record(&record(
            r#"{"type":"stream_event","session_id":"s1","event":{"type":"other"},"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::MessageUpdate {
                session_id: "s1".to_string(),
                delta: "".to_string(),
            }
        );
    }

    #[test]
    fn translate_result_record() {
        let ev = translate_cli_record(&record(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","duration_ms":123,"num_turns":2,"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":1},"model":"m:free","session_id":"s1","v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::Result {
                session_id: "s1".to_string(),
                subtype: "success".to_string(),
                is_error: false,
                text: "done".to_string(),
                num_turns: 2,
                usage: TurnUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_input_tokens: 1,
                },
                duration_ms: 123,
                model: "m:free".to_string(),
            }
        );
    }

    #[test]
    fn translate_unknown_type_is_unknown_not_fatal() {
        let ev = translate_cli_record(&record(
            r#"{"type":"future_thing","subtype":"x","session_id":"s1","v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::Unknown {
                session_id: "s1".to_string(),
                record_type: "future_thing".to_string(),
                subtype: Some("x".to_string()),
            }
        );
        // Unknown extra fields on a known type are ignored (additive).
        let ev = translate_cli_record(&record(
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","brand_new_field":{"a":1},"v":1}"#,
        ));
        assert_eq!(
            ev,
            SenEvent::SessionStart {
                session_id: "s1".to_string(),
                model: "m".to_string(),
            }
        );
    }

    #[test]
    fn translate_missing_fields_degrade_gracefully() {
        let ev = translate_cli_record(&record(r#"{"type":"result","v":1}"#));
        assert_eq!(
            ev,
            SenEvent::Result {
                session_id: "".to_string(),
                subtype: "".to_string(),
                is_error: false,
                text: "".to_string(),
                num_turns: 0,
                usage: TurnUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_input_tokens: 0,
                },
                duration_ms: 0,
                model: "".to_string(),
            }
        );
    }
}
