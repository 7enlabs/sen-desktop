// MCP delegate through the CLI core (Tauri follow-up 1, ZC Leader
// tin 17093). PURE RUST, no Tauri dependency: compiles and tests
// standalone in the scratch harness.
//
// The webview cannot spawn `zen` (no node:child_process there), so
// /mcp used to refuse honestly until a backend command owned the
// delegate path. This module is that path: it validates the server
// config, writes the {"mcpServers": ...} file the CLI core reads,
// and runs ONE delegated turn through the session CLI bridge
// (cli_spawn.rs) — the same spawn/read/wait loop as a model turn,
// only the argv differs (--mcp-config + the delegate prompt).
// The agent loop, the permission checks, and the MCP server
// lifecycle all belong to the CLI core; Sen only asks and reads the
// result record.
//
// Session isolation, decided from the CLI source (cli-core-main,
// crates/zen-cli/src/lib.rs resolve_session, rev 8ac73fe):
// --session-id <id> with no -r opens/creates the session file at
// that id WITHOUT loading history, so a delegate turn run under the
// chat session id would overwrite the model session's transcript
// file on disk. The delegate therefore runs on a FRESH bridge with
// a fresh UUID per call (same default as the TS delegate,
// delegateToolCallToCliCore, which defaults to randomUUID()), never
// on the session's bridge. The session bridge is only read for the
// model/base URL the delegate turn runs on (see commands/mcp.rs).
// The fresh bridge is discarded after the turn, so its `started`
// state never leaks into a later model turn.
//
// The config file lives in a fresh temp dir per call and is removed
// when the turn ends (TempMcpConfig Drop guard), even on error —
// the same finally-block rule as the TS delegate. Env values in the
// config are passed through untouched: the CLI core expands them
// (zen-mcp expand_with_missing), never this module. No key, token,
// or credential is stored, logged, or sent anywhere: the config
// travels as a file path in argv, never as a value.
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::cli_spawn::{BridgeError, CliBridge, TurnResult, ERR_CLI_SPAWN_FAILED};
use super::stream_json::SenEvent;

/// Default delegate turn timeout (ms): the CLI bridge cap
/// (TURN_TIMEOUT, 120s), same as the TS delegate
/// (MCP_DELEGATE_TIMEOUT_MS).
pub const MCP_DELEGATE_TIMEOUT: Duration = Duration::from_secs(120);

/// Error codes (same strings as the TS delegate, so the frontend
/// sees the same codes from either path).
pub const ERR_MCP_CONFIG_INVALID: &str = "MCP_CONFIG_INVALID";
pub const ERR_MCP_VALIDATION: &str = "MCP_VALIDATION";
pub const ERR_MCP_DELEGATE_ERROR: &str = "CLI_DELEGATE_ERROR";

/// One MCP server, same shape as zen-mcp McpServerConfig
/// (crates/zen-mcp/src/config.rs, ZenCore 8ac73fe) and the TS
/// McpServerConfig: a stdio server has command (+args/env), an HTTP
/// server has url (+headers). `type` may say "stdio" or "http";
/// when absent, a server with a command is stdio and one with only
/// a url is HTTP (same rule as zen-mcp is_http) — but the CLI
/// loader still skips a url server with no explicit type
/// (`url_missing_type`), so validation requires `type: "http"` on
/// a url server (see validate_mcp_server_config).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
}

/// True when the config is an HTTP server (same rule as zen-mcp
/// is_http and the TS isHttpServerConfig).
pub fn is_http_server_config(config: &McpServerConfig) -> bool {
    if let Some(t) = config.type_.as_deref() {
        if t.eq_ignore_ascii_case("http") {
            return true;
        }
    }
    config.command.is_none() && config.url.is_some()
}

/// Validate a server config from unknown input (JSON from IPC).
/// Mirrors the TS validateMcpServerConfig message for message, so a
/// config rejected on one side is rejected the same way on the
/// other. Unknown keys are ignored, like the TS version.
pub fn validate_mcp_server_config(config: &serde_json::Value) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let Some(raw) = config.as_object() else {
        return vec!["MCP server config must be an object".to_string()];
    };
    let get_str = |key: &str| -> Option<&str> { raw.get(key).and_then(|v| v.as_str()) };
    if let Some(v) = raw.get("type") {
        if !v.is_null() && v.as_str().is_none() {
            errors.push("MCP server type must be a string".to_string());
        }
    }
    let kind = get_str("type").unwrap_or("").to_ascii_lowercase();
    if !kind.is_empty() && kind != "stdio" && kind != "http" {
        errors.push(format!(
            "MCP server type must be \"stdio\" or \"http\", got {}",
            raw.get("type").unwrap()
        ));
    }
    if let Some(v) = raw.get("command") {
        if !v.is_null() && (v.as_str().is_none() || v.as_str().unwrap().trim().is_empty()) {
            errors.push("MCP server command must be a non-empty string".to_string());
        }
    }
    if let Some(v) = raw.get("args") {
        let bad = !v.is_array() || v.as_array().unwrap().iter().any(|a| a.as_str().is_none());
        if bad {
            errors.push("MCP server args must be an array of strings".to_string());
        }
    }
    if let Some(v) = raw.get("env") {
        let bad = !v.is_object()
            || v.as_object()
                .unwrap()
                .values()
                .any(|x| x.as_str().is_none());
        if bad {
            errors.push("MCP server env must be an object of strings".to_string());
        }
    }
    if let Some(v) = raw.get("url") {
        if !v.is_null() {
            match v.as_str() {
                None => errors.push("MCP server url must be a non-empty string".to_string()),
                Some(u) if u.trim().is_empty() => {
                    errors.push("MCP server url must be a non-empty string".to_string())
                }
                Some(u) => {
                    // Mirror the TS `new URL` check: the scheme must
                    // be http(s) and something must follow it.
                    let rest = u
                        .strip_prefix("http://")
                        .or_else(|| u.strip_prefix("https://"));
                    match rest {
                        Some(r) if !r.is_empty() => {}
                        Some(_) => {
                            errors.push("MCP server url must be a valid URL".to_string());
                        }
                        None => {
                            if u.contains("://") {
                                errors.push("MCP server url must be an http(s) URL".to_string());
                            } else {
                                errors.push("MCP server url must be a valid URL".to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(v) = raw.get("headers") {
        let bad = !v.is_object()
            || v.as_object()
                .unwrap()
                .values()
                .any(|x| x.as_str().is_none());
        if bad {
            errors.push("MCP server headers must be an object of strings".to_string());
        }
    }
    let has_command = get_str("command")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let has_url = get_str("url")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !has_command && !has_url {
        errors.push("MCP server needs a command (stdio) or a url (HTTP)".to_string());
    }
    if kind == "stdio" && !has_command {
        errors.push("MCP server type \"stdio\" needs a command".to_string());
    }
    if kind == "http" && !has_url {
        errors.push("MCP server type \"http\" needs a url".to_string());
    }
    // The CLI loader (zen-mcp manager.rs `validate`) skips a server
    // with a url but no type (`url_missing_type`): `is_http()` would
    // still classify it as HTTP, but the server never starts, so a
    // delegate turn would run with no server and fail confusingly
    // later. Require the explicit type here (live probe 19/11
    // against the real `zen` 0.1.0: a typeless url config yields
    // `mcp_servers: []` + `url_missing_type`, a typed one registers).
    if has_url && matches!(raw.get("type"), None | Some(serde_json::Value::Null)) {
        errors.push(
            "MCP server with a url needs type \"http\" (the CLI skips a url without a type: url_missing_type)"
                .to_string(),
        );
    }
    errors
}

/// The config file the CLI core reads: {"mcpServers": {name:
/// config}}, the same shape zen-mcp load_mcp_configs_scoped parses
/// and the TS mcpConfigFileJson writes.
pub fn mcp_config_file_json(name: &str, config: &McpServerConfig) -> String {
    let mut servers = serde_json::Map::new();
    servers.insert(
        name.to_string(),
        serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
    );
    let mut root = serde_json::Map::new();
    root.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
    serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .unwrap_or_else(|_| "{}".to_string())
}

/// The prompt handed to the CLI agent for one delegated tool call.
/// Byte-identical to the TS buildDelegatePrompt: it names the
/// server, the tool, and the exact JSON arguments, and asks for only
/// the tool result back.
pub fn build_delegate_prompt(server_name: &str, tool: &str, args: &serde_json::Value) -> String {
    format!(
        "Call the MCP tool \"{}\" on MCP server \"{}\" with arguments {}. Reply with only the tool result, no extra text.",
        tool,
        server_name,
        serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
    )
}

/// Build the `zen -p` argv for a delegated turn. Same flags as the
/// CLI bridge (cli_spawn.rs build_cli_argv) plus --mcp-config
/// <file> so the CLI core starts the server with its own zen-mcp
/// manager. Byte-identical to the TS buildDelegateArgv. The prompt
/// is the last argv element, preceded by `--` (same fix as Rust
/// P1-1). Flags never follow it.
#[allow(clippy::too_many_arguments)]
pub fn build_delegate_argv(
    server_name: &str,
    tool: &str,
    args: &serde_json::Value,
    config_path: &std::path::Path,
    session_id: &str,
    resume: bool,
    model: Option<&str>,
    base_url: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--bare".to_string(),
        "--setting-sources".to_string(),
        String::new(),
        "--mcp-config".to_string(),
        config_path.to_string_lossy().to_string(),
    ];
    if let Some(model) = model {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    if let Some(url) = base_url {
        argv.push("--base-url".to_string());
        argv.push(url.to_string());
    }
    if resume {
        argv.push("-r".to_string());
        argv.push(session_id.to_string());
    } else {
        argv.push("--session-id".to_string());
        argv.push(session_id.to_string());
    }
    argv.push("--".to_string());
    argv.push(build_delegate_prompt(server_name, tool, args));
    argv
}

/// True when `value` is a UUID the CLI accepts for --session-id:
/// the hyphenated form `randomUUID()` produces, or the 32-hex
/// simple form. Mirrors the TS isValidUuid and the CLI's
/// Uuid::parse_str (zen-cli cli.rs parse_session_uuid).
pub fn is_valid_uuid(value: &str) -> bool {
    let hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    if value.len() == 36 {
        let parts: Vec<&str> = value.split('-').collect();
        return parts.len() == 5
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
            && parts.iter().all(|p| hex(p));
    }
    value.len() == 32 && hex(value)
}

/// The temp dir holding one delegate call's mcp-servers.json.
/// Removed (whole dir) when dropped, even on error — the Rust side
/// of the TS delegate's finally block.
pub struct TempMcpConfig {
    pub dir: PathBuf,
    pub path: PathBuf,
}

impl Drop for TempMcpConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Write the config file into a fresh temp dir and return the guard
/// holding it. The dir name carries the process id plus a
/// nanosecond timestamp, so two concurrent delegate calls never
/// share a dir. The caller owns the guard: dropping it removes the
/// whole dir.
pub fn write_mcp_config_file(
    name: &str,
    config: &McpServerConfig,
) -> Result<TempMcpConfig, BridgeError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("sen-mcp-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).map_err(|e| {
        BridgeError::new(
            ERR_CLI_SPAWN_FAILED,
            &format!("Cannot create MCP config temp dir: {}", e),
        )
    })?;
    let path = dir.join("mcp-servers.json");
    std::fs::write(&path, mcp_config_file_json(name, config)).map_err(|e| {
        let _ = std::fs::remove_dir_all(&dir);
        BridgeError::new(
            ERR_CLI_SPAWN_FAILED,
            &format!("Cannot write MCP config file: {}", e),
        )
    })?;
    Ok(TempMcpConfig { dir, path })
}

/// One delegated MCP tool call: everything the turn needs except the
/// bridge itself. `session_id` is the DELEGATE CLI session id (a
/// fresh UUID per call, generated by the caller — never the chat
/// session id, see the module header). `arguments` defaults to an
/// empty object when None. `timeout` defaults to
/// MCP_DELEGATE_TIMEOUT (120s) when None.
pub struct McpDelegateInput {
    pub server_name: String,
    pub server_config: McpServerConfig,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub session_id: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub timeout: Option<Duration>,
}

/// Run one delegated MCP tool call on `bridge` (a fresh per-call
/// bridge, see the module header): validate, write the temp config,
/// run the turn through the bridge's shared loop, remove the temp
/// dir. The TurnResult is returned as-is, including an is_error
/// result record: the caller (commands/mcp.rs) maps that to
/// CLI_DELEGATE_ERROR, mirroring the TS delegate's rejection.
pub fn run_mcp_delegate_turn(
    bridge: &CliBridge,
    input: &McpDelegateInput,
    emit: &mut dyn FnMut(SenEvent),
) -> Result<TurnResult, BridgeError> {
    let config_value = serde_json::to_value(&input.server_config)
        .map_err(|e| BridgeError::new(ERR_MCP_CONFIG_INVALID, &e.to_string()))?;
    let config_errors = validate_mcp_server_config(&config_value);
    if !config_errors.is_empty() {
        return Err(BridgeError::new(
            ERR_MCP_CONFIG_INVALID,
            &config_errors.join("; "),
        ));
    }
    if input.server_name.trim().is_empty() {
        return Err(BridgeError::new(
            ERR_MCP_VALIDATION,
            "MCP_VALIDATION: serverName must be non-empty",
        ));
    }
    if input.tool.trim().is_empty() {
        return Err(BridgeError::new(
            ERR_MCP_VALIDATION,
            "MCP_VALIDATION: tool must be non-empty",
        ));
    }
    if !is_valid_uuid(&input.session_id) {
        return Err(BridgeError::new(
            ERR_MCP_VALIDATION,
            "MCP_VALIDATION: sessionId must be a valid UUID",
        ));
    }
    // The temp config lives until the turn ends: the guard is
    // dropped (dir removed) only after run_turn_with_custom_argv
    // returns, on every path.
    let temp = write_mcp_config_file(&input.server_name, &input.server_config)?;
    let prompt = build_delegate_prompt(&input.server_name, &input.tool, &input.arguments);
    let argv = build_delegate_argv(
        &input.server_name,
        &input.tool,
        &input.arguments,
        &temp.path,
        &input.session_id,
        false,
        input.model.as_deref(),
        input.base_url.as_deref(),
    );
    let timeout = input.timeout.unwrap_or(MCP_DELEGATE_TIMEOUT);
    bridge.run_turn_with_custom_argv("mcp-delegate", &prompt, timeout, argv, emit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const DELEGATE_SESSION: &str = "11111111-2222-4333-8444-555555555555";

    /// A fake `zen` binary (same shape as cli_spawn.rs tests):
    /// prints "zen 0.1.0" for --version, else the canned
    /// stream-json turn on stdout. Records its argv to `marker`
    /// when set, so the test can prove --mcp-config was passed.
    fn fake_zen(tag: &str, body: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sen_fake_zen_mcp_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zen");
        let mut script = String::from("#!/bin/sh\n");
        script.push_str("if [ \"$1\" = \"--version\" ]; then echo \"zen 0.1.0\"; exit 0; fi\n");
        script.push_str(body);
        script.push('\n');
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    const HAPPY_BODY: &str = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","cwd":"/tmp","model":"m:free","tools":[],"mcp_servers":[],"permissionMode":"default","slash_commands":[],"v":1}
{"type":"result","subtype":"success","is_error":false,"result":"echo: hello","duration_ms":7,"num_turns":1,"usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0},"model":"m:free","session_id":"s1","v":1}
EOF
exit 0"#;

    fn stdio_config() -> McpServerConfig {
        McpServerConfig {
            type_: None,
            command: Some("node".to_string()),
            args: Some(vec!["server.mjs".to_string()]),
            env: None,
            url: None,
            headers: None,
        }
    }

    #[test]
    fn prompt_names_server_tool_and_args() {
        let prompt = build_delegate_prompt("calc", "echo", &serde_json::json!({"hello": "world"}));
        assert_eq!(
            prompt,
            "Call the MCP tool \"echo\" on MCP server \"calc\" with arguments {\"hello\":\"world\"}. Reply with only the tool result, no extra text."
        );
    }

    #[test]
    fn argv_matches_ts_delegate_argv() {
        let argv = build_delegate_argv(
            "calc",
            "echo",
            &serde_json::json!({"a": 1}),
            std::path::Path::new("/tmp/sen-mcp-x/mcp-servers.json"),
            DELEGATE_SESSION,
            false,
            Some("m:free"),
            None,
        );
        assert_eq!(
            argv,
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--bare",
                "--setting-sources",
                "",
                "--mcp-config",
                "/tmp/sen-mcp-x/mcp-servers.json",
                "--model",
                "m:free",
                "--session-id",
                DELEGATE_SESSION,
                "--",
                "Call the MCP tool \"echo\" on MCP server \"calc\" with arguments {\"a\":1}. Reply with only the tool result, no extra text.",
            ]
        );
        // Resume uses -r; no model/base-url passes no such flag.
        let resumed = build_delegate_argv(
            "calc",
            "echo",
            &serde_json::json!({}),
            std::path::Path::new("/tmp/c.json"),
            DELEGATE_SESSION,
            true,
            None,
            Some("https://example.invalid/v1"),
        );
        assert!(resumed.contains(&"-r".to_string()));
        assert!(!resumed.contains(&"--session-id".to_string()));
        assert!(!resumed.contains(&"--model".to_string()));
        let base_pos = resumed.iter().position(|a| a == "--base-url").unwrap();
        let resume_pos = resumed.iter().position(|a| a == "-r").unwrap();
        assert!(base_pos < resume_pos, "--base-url must precede -r");
    }

    #[test]
    fn config_file_json_shape_matches_zen_mcp() {
        let json = mcp_config_file_json("calc", &stdio_config());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["mcpServers"]["calc"]["command"], "node");
        assert_eq!(value["mcpServers"]["calc"]["args"][0], "server.mjs");
    }

    #[test]
    fn validate_mirrors_ts_cases() {
        let ok = validate_mcp_server_config(&serde_json::to_value(stdio_config()).unwrap());
        assert!(ok.is_empty(), "stdio config must validate: {:?}", ok);
        let http = serde_json::json!({"type": "http", "url": "https://example.invalid/mcp"});
        assert!(validate_mcp_server_config(&http).is_empty());
        // A url without an explicit type does NOT validate: the CLI
        // loader skips it (url_missing_type, live probe 19/11).
        let typeless = serde_json::json!({"url": "https://example.invalid/mcp"});
        assert!(validate_mcp_server_config(&typeless)
            .iter()
            .any(|e| e.contains("url_missing_type")));
        // No command and no url.
        assert!(validate_mcp_server_config(&serde_json::json!({}))
            .iter()
            .any(|e| e.contains("needs a command")));
        // stdio without a command.
        assert!(
            validate_mcp_server_config(&serde_json::json!({"type": "stdio"}))
                .iter()
                .any(|e| e.contains("stdio") && e.contains("command"))
        );
        // http without a url.
        assert!(
            validate_mcp_server_config(&serde_json::json!({"type": "http"}))
                .iter()
                .any(|e| e.contains("http") && e.contains("url"))
        );
        // Bad type value.
        assert!(
            validate_mcp_server_config(&serde_json::json!({"type": "grpc", "command": "x"}))
                .iter()
                .any(|e| e.contains("stdio") && e.contains("http"))
        );
        // Non-string args / env values.
        assert!(
            validate_mcp_server_config(&serde_json::json!({"command": "x", "args": [1]}))
                .iter()
                .any(|e| e.contains("args"))
        );
        assert!(
            validate_mcp_server_config(&serde_json::json!({"command": "x", "env": {"K": 1}}))
                .iter()
                .any(|e| e.contains("env"))
        );
        // ftp scheme is not http(s); a bare host is not a URL.
        assert!(
            validate_mcp_server_config(&serde_json::json!({"url": "ftp://example.invalid/x"}))
                .iter()
                .any(|e| e.contains("http(s)"))
        );
        assert!(
            validate_mcp_server_config(&serde_json::json!({"url": "example.invalid/x"}))
                .iter()
                .any(|e| e.contains("valid URL"))
        );
        // Not an object at all.
        assert_eq!(
            validate_mcp_server_config(&serde_json::json!("nope")),
            vec!["MCP server config must be an object".to_string()]
        );
    }

    #[test]
    fn uuid_accepts_hyphenated_and_simple_forms() {
        assert!(is_valid_uuid("11111111-2222-4333-8444-555555555555"));
        assert!(is_valid_uuid("11111111222243338444555555555555"));
        assert!(is_valid_uuid("AAAAAAAA-2222-4333-8444-555555555555"));
        assert!(!is_valid_uuid("app-live-session"));
        assert!(!is_valid_uuid("11111111-2222-4333-8444"));
        assert!(!is_valid_uuid(""));
        assert!(!is_valid_uuid("zzzzzzzz-2222-4333-8444-555555555555"));
    }

    #[test]
    fn temp_config_file_is_written_and_removed_on_drop() {
        let temp = write_mcp_config_file("calc", &stdio_config()).unwrap();
        let text = std::fs::read_to_string(&temp.path).unwrap();
        assert!(text.contains("\"calc\""));
        assert!(text.contains("\"node\""));
        let dir = temp.dir.clone();
        assert!(dir.exists());
        drop(temp);
        assert!(!dir.exists(), "temp MCP config dir must be removed on drop");
    }

    #[test]
    fn delegate_turn_passes_mcp_config_and_returns_result_text() {
        let marker = std::env::temp_dir().join(format!("sen_mcp_argv_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let body = format!(
            "printf '%s\\n' \"$@\" > '{marker}'\n{HAPPY_BODY}",
            marker = marker.display()
        );
        let bin = fake_zen("delegate", &body);
        let bridge = CliBridge::new(Some(bin), DELEGATE_SESSION, "m:free", None);
        let events: Arc<Mutex<Vec<SenEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let mut emit = move |ev: SenEvent| {
            sink.lock().unwrap().push(ev);
        };
        let input = McpDelegateInput {
            server_name: "calc".to_string(),
            server_config: stdio_config(),
            tool: "echo".to_string(),
            arguments: serde_json::json!({"hello": "world"}),
            session_id: DELEGATE_SESSION.to_string(),
            model: None,
            base_url: None,
            timeout: None,
        };
        let result = run_mcp_delegate_turn(&bridge, &input, &mut emit).unwrap();
        assert_eq!(result.text, "echo: hello");
        assert!(!result.is_error);
        assert_eq!(result.subtype, "success");
        // The spawned argv really carried --mcp-config pointing at a
        // file that held the server config...
        let argv = std::fs::read_to_string(&marker).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        let cfg_pos = argv.iter().position(|a| *a == "--mcp-config").unwrap();
        let cfg_path = argv[cfg_pos + 1];
        // ...and the temp dir is gone after the turn (guard drop).
        assert!(
            !std::path::Path::new(cfg_path).exists(),
            "temp config file must be removed after the turn"
        );
        assert_eq!(
            argv.last().unwrap(),
            &build_delegate_prompt("calc", "echo", &serde_json::json!({"hello": "world"}))
        );
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn delegate_turn_rejects_bad_config_before_spawn() {
        let marker = std::env::temp_dir().join(format!("sen_mcp_nospawn_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let body = format!(
            "printf '%s\\n' \"$@\" > '{marker}'\n{HAPPY_BODY}",
            marker = marker.display()
        );
        let bin = fake_zen("nospawn", &body);
        let bridge = CliBridge::new(Some(bin), DELEGATE_SESSION, "m:free", None);
        let mut emit = |_: SenEvent| {};
        let bad = McpServerConfig {
            type_: None,
            command: None,
            args: None,
            env: None,
            url: None,
            headers: None,
        };
        let input = McpDelegateInput {
            server_name: "calc".to_string(),
            server_config: bad,
            tool: "echo".to_string(),
            arguments: serde_json::json!({}),
            session_id: DELEGATE_SESSION.to_string(),
            model: None,
            base_url: None,
            timeout: None,
        };
        let err = run_mcp_delegate_turn(&bridge, &input, &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_MCP_CONFIG_INVALID);
        assert!(
            !marker.exists(),
            "a rejected config must never spawn the CLI"
        );
        // A bad session id fails fast too.
        let input = McpDelegateInput {
            server_name: "calc".to_string(),
            server_config: stdio_config(),
            tool: "echo".to_string(),
            arguments: serde_json::json!({}),
            session_id: "not-a-uuid".to_string(),
            model: None,
            base_url: None,
            timeout: None,
        };
        let err = run_mcp_delegate_turn(&bridge, &input, &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_MCP_VALIDATION);
        assert!(!marker.exists());
    }

    #[test]
    fn delegate_turn_returns_error_result_as_turn_result() {
        let body = r#"cat <<'EOF'
{"type":"result","subtype":"error","is_error":true,"result":"tool exploded","duration_ms":3,"num_turns":1,"usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0},"model":"m:free","session_id":"s1","v":1}
EOF
exit 0"#;
        let bin = fake_zen("errresult", body);
        let bridge = CliBridge::new(Some(bin), DELEGATE_SESSION, "m:free", None);
        let mut emit = |_: SenEvent| {};
        let input = McpDelegateInput {
            server_name: "calc".to_string(),
            server_config: stdio_config(),
            tool: "echo".to_string(),
            arguments: serde_json::json!({}),
            session_id: DELEGATE_SESSION.to_string(),
            model: None,
            base_url: None,
            timeout: None,
        };
        let result = run_mcp_delegate_turn(&bridge, &input, &mut emit).unwrap();
        assert!(result.is_error);
        assert_eq!(result.text, "tool exploded");
    }
}
