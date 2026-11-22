// Real Tauri command handler for the MCP delegate (Tauri
// follow-up 1, ZC Leader tin 17093). Thin wrapper only: it
// validates the contract, resolves the session's bridge for the
// model/base URL, and delegates to commands/mcp_delegate.rs, which
// owns the config validation, the temp config file, and the turn
// through the CLI bridge. No delegate logic lives here.
//
// The delegate runs on a FRESH bridge with a fresh UUID per call
// (see mcp_delegate.rs): --session-id with no -r opens the session
// file at that id without loading history, so running the delegate
// on the chat session's bridge would overwrite the model session's
// transcript file on disk. The session bridge is only read for the
// model and base URL the delegate turn runs on; the session must
// exist, or the command fails closed with UNKNOWN_SESSION.
// A result record with is_error=true is an Err here
// (CLI_DELEGATE_ERROR), mirroring the TS delegate's rejection:
// the tool ran and reported an error, so there is no result text
// to hand back as a success.
// No key, token, or credential travels through this command: the
// server config travels as a file path in argv, never as a value.
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::mcp_delegate::{
    run_mcp_delegate_turn, validate_mcp_server_config, McpDelegateInput, McpServerConfig,
    ERR_MCP_CONFIG_INVALID, ERR_MCP_DELEGATE_ERROR,
};
use super::registry::Registry;
use super::session::ContractArgs;
use super::stream_json::SenEvent;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDelegateArgs {
    pub server_name: String,
    pub server_config: serde_json::Value,
    pub tool: String,
    /// Tool arguments as a JSON object. Missing or null means {}.
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDelegateResult {
    /// Text of the stream-json result record.
    pub text: String,
    /// The fresh delegate CLI session id (a UUID per call, NOT the
    /// chat session id — see the module header).
    pub session_id: String,
    pub exit_code: Option<i32>,
}

/// sen_mcp_delegate: run one MCP tool call through the CLI core.
/// The frontend passes the chat session id (to resolve the model),
/// the server name + config, the tool, and its arguments; the
/// backend spawns `zen -p --mcp-config <temp file>` and returns
/// the result record's text. Events from the delegate turn are not
/// forwarded to the frontend (the caller awaits this command's
/// return value, not the event stream); the turn result is the
/// evidence.
#[tauri::command]
pub fn sen_mcp_delegate(
    state: State<'_, Mutex<Registry>>,
    session_id: String,
    contract: ContractArgs,
    args: McpDelegateArgs,
) -> Result<McpDelegateResult, String> {
    let registry_contract = contract.to_registry();
    super::registry::validate_contract(&registry_contract).map_err(|e| e.to_wire())?;
    // The session must exist: its bridge decides the model and
    // base URL the delegate turn runs on.
    let (model, base_url) = {
        let registry = state.lock().map_err(|e| e.to_string())?;
        match registry.bridge(&session_id) {
            Some(bridge) => (
                bridge.model().to_string(),
                bridge.base_url().map(|u| u.to_string()),
            ),
            None => {
                return Err(format!(
                    "{}: Session does not exist: {}",
                    super::registry::ERR_UNKNOWN_SESSION,
                    session_id
                ));
            }
        }
    };
    // Validate the raw config before converting it: the typed
    // conversion below would lose the TS-shaped error messages.
    let config_errors = validate_mcp_server_config(&args.server_config);
    if !config_errors.is_empty() {
        return Err(format!(
            "{}: {}",
            ERR_MCP_CONFIG_INVALID,
            config_errors.join("; ")
        ));
    }
    let server_config: McpServerConfig = serde_json::from_value(args.server_config)
        .map_err(|e| format!("{}: {}", ERR_MCP_CONFIG_INVALID, e))?;
    let arguments = if args.arguments.is_null() {
        serde_json::json!({})
    } else {
        args.arguments
    };
    // A fresh UUID per call, never the chat session id (see the
    // module header: the delegate must not touch the model
    // session's transcript file or bridge state).
    let delegate_session_id = uuid::Uuid::new_v4().to_string();
    let bridge =
        super::cli_spawn::CliBridge::new(None, &delegate_session_id, &model, base_url.as_deref());
    let input = McpDelegateInput {
        server_name: args.server_name,
        server_config,
        tool: args.tool,
        arguments,
        session_id: delegate_session_id.clone(),
        model: Some(model),
        base_url,
        timeout: Some(Duration::from_millis(contract.timeout_ms)),
    };
    let mut emit = |_: SenEvent| {};
    let turn = run_mcp_delegate_turn(&bridge, &input, &mut emit)
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    if turn.is_error {
        return Err(format!("{}: {}", ERR_MCP_DELEGATE_ERROR, turn.text));
    }
    Ok(McpDelegateResult {
        text: turn.text,
        session_id: delegate_session_id,
        exit_code: turn.exit_code,
    })
}
