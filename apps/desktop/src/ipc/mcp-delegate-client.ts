// Frontend client for the Rust MCP delegate (Tauri follow-up 1,
// ZC Leader tin 17093): one call, through the bridge like every
// other client, to sen_mcp_delegate. The backend spawns `zen -p
// --mcp-config <temp file>` on a fresh delegate bridge and returns
// the result record's text. A backend failure surfaces here
// untouched — never mapped to a fake success. No key, token, or
// credential travels through this call: the server config travels
// as a JSON value the backend writes to a temp file, never as
// argv, and no secret is ever in it (the CLI core expands env
// values itself).
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";
import type { McpServerConfig } from "../../../../packages/core/src/mcp-client.ts";

export interface McpDelegateCall {
  sessionId: string;
  serverName: string;
  serverConfig: McpServerConfig;
  tool: string;
  arguments?: Record<string, unknown>;
}

export interface McpDelegateResult {
  /** Text of the stream-json result record. */
  text: string;
  /** The fresh delegate CLI session id (a UUID per call, NOT the chat session id). */
  sessionId: string;
  exitCode: number | null;
}

function asDelegateResult(value: unknown): McpDelegateResult {
  const res = value as McpDelegateResult | null;
  if (
    !res ||
    typeof res.text !== "string" ||
    typeof res.sessionId !== "string" ||
    !res.sessionId ||
    (res.exitCode !== null && typeof res.exitCode !== "number")
  ) {
    throw new Error(
      "Backend MCP delegate result malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/** Run one MCP tool call through the Rust delegate command. */
export async function delegateMcpTool(
  contract: ActionContract,
  call: McpDelegateCall,
): Promise<McpDelegateResult> {
  const raw = await invoke("sen_mcp_delegate", {
    contract,
    session_id: call.sessionId,
    args: {
      serverName: call.serverName,
      serverConfig: call.serverConfig,
      tool: call.tool,
      arguments: call.arguments ?? {},
    },
  });
  return asDelegateResult(raw);
}
