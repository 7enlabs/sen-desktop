// MCP client (Sen P0-5): speak the Model Context Protocol to an MCP
// server, and delegate tool calls to the ZenCore CLI core when the
// caller wants the CLI's agent runtime to run them.
//
// The MCP protocol itself is plain JSON-RPC 2.0 over newline-delimited
// JSON (stdio transport) or Streamable HTTP (HTTP transport). This
// module owns the protocol framing only: request ids, timeouts,
// initialize handshake, tools/list, tools/call, ping. It owns NO agent
// loop, NO model call, and NO subagent spawn: there is no second
// runtime here. When a tool call must run inside an agent turn, use
// delegateToolCallToCliCore below, which hands the call to the `zen`
// CLI (its zen-mcp manager + SubAgentExecutor, max_depth 1) through
// the same stream-json bridge the rest of Sen already uses
// (apps/desktop/src-tauri/src/commands/cli_spawn.rs).
//
// References (read, not copied): the MCP config shape mirrors
// zen-mcp McpServerConfig (crates/zen-mcp/src/config.rs, ZenCore
// 8ac73fe): {"mcpServers": {name: {command, args, env, url,
// headers, type}}}; the stream-json result record parsed by the
// delegate is the one stream_json.rs translates (type "result",
// field "result", is_error flag).
//
// Tool calls with a side effect go through the P0-2 approval gate
// (packages/core/src/approval-gate.ts) via executeMcpTool: same
// default-deny policy, same 3 tiers, same append-only activity log.
// A gate pass means permission granted, not executed: the MCP server
// still runs the tool, and a tool that reports isError still ran -
// the caller reads isError from the returned result.
//
// No key, token, or credential is stored, logged, or sent anywhere:
// server env values come from the caller's config object, and the
// delegate passes --mcp-config as a file path, never a key value in
// argv. Env values are expanded by the CLI core (zen-mcp
// expand_with_missing), not by this module.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import {
  ActivityLog,
  ApprovalStore,
  evaluateToolCall,
  gateToolCall,
  type GateEvaluation,
  type ToolPolicy,
} from "./approval-gate.ts";

/** MCP protocol version this client speaks in initialize. */
export const MCP_PROTOCOL_VERSION = "2024-11-05";
/** Client name sent in the initialize handshake. */
export const MCP_CLIENT_NAME = "sen-desktop";
export const MCP_CLIENT_VERSION = "0.0.0";
/** Default per-request timeout (ms). */
export const MCP_DEFAULT_TIMEOUT_MS = 30000;
/** Default delegate turn timeout (ms): the CLI bridge cap (TURN_TIMEOUT). */
export const MCP_DELEGATE_TIMEOUT_MS = 120000;

/**
 * One MCP server, same shape as zen-mcp McpServerConfig. A stdio
 * server has command (+args/env); an HTTP server has url (+headers).
 * `type` may say "stdio" or "http"; when absent, a server with a
 * command is stdio and one with only a url is HTTP (same rule as
 * zen-mcp is_http).
 */
export interface McpServerConfig {
  type?: string | null;
  command?: string | null;
  args?: string[];
  env?: Record<string, string>;
  url?: string | null;
  headers?: Record<string, string>;
}

export function isHttpServerConfig(config: McpServerConfig): boolean {
  if (
    typeof config.type === "string" &&
    config.type.toLowerCase() === "http"
  ) {
    return true;
  }
  return !config.command && !!config.url;
}

/** Validate a server config from unknown input (JSON from disk or IPC). */
export function validateMcpServerConfig(config: unknown): string[] {
  if (!config || typeof config !== "object" || Array.isArray(config)) {
    return ["MCP server config must be an object"];
  }
  const raw = config as Record<string, unknown>;
  const errors: string[] = [];
  if (
    raw.type !== undefined &&
    raw.type !== null &&
    typeof raw.type !== "string"
  ) {
    errors.push("MCP server type must be a string");
  }
  const kind =
    typeof raw.type === "string" ? raw.type.toLowerCase() : "";
  if (kind && kind !== "stdio" && kind !== "http") {
    errors.push(`MCP server type must be "stdio" or "http", got ${raw.type}`);
  }
  if (
    raw.command !== undefined &&
    raw.command !== null &&
    (typeof raw.command !== "string" || !raw.command.trim())
  ) {
    errors.push("MCP server command must be a non-empty string");
  }
  if (
    raw.args !== undefined &&
    (!Array.isArray(raw.args) ||
      raw.args.some((a) => typeof a !== "string"))
  ) {
    errors.push("MCP server args must be an array of strings");
  }
  if (
    raw.env !== undefined &&
    (typeof raw.env !== "object" ||
      raw.env === null ||
      Array.isArray(raw.env) ||
      Object.values(raw.env as Record<string, unknown>).some(
        (v) => typeof v !== "string",
      ))
  ) {
    errors.push("MCP server env must be an object of strings");
  }
  if (
    raw.url !== undefined &&
    raw.url !== null &&
    (typeof raw.url !== "string" || !raw.url.trim())
  ) {
    errors.push("MCP server url must be a non-empty string");
  }
  if (
    raw.url !== undefined &&
    raw.url !== null &&
    typeof raw.url === "string" &&
    raw.url.trim()
  ) {
    try {
      const parsed = new URL(raw.url);
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
        errors.push("MCP server url must be an http(s) URL");
      }
    } catch {
      errors.push("MCP server url must be a valid URL");
    }
  }
  if (
    raw.headers !== undefined &&
    (typeof raw.headers !== "object" ||
      raw.headers === null ||
      Array.isArray(raw.headers) ||
      Object.values(raw.headers as Record<string, unknown>).some(
        (v) => typeof v !== "string",
      ))
  ) {
    errors.push("MCP server headers must be an object of strings");
  }
  const hasCommand =
    typeof raw.command === "string" && raw.command.trim() !== "";
  const hasUrl = typeof raw.url === "string" && raw.url.trim() !== "";
  if (!hasCommand && !hasUrl) {
    errors.push(
      "MCP server needs a command (stdio) or a url (HTTP)",
    );
  }
  if (kind === "stdio" && !hasCommand) {
    errors.push('MCP server type "stdio" needs a command');
  }
  if (kind === "http" && !hasUrl) {
    errors.push('MCP server type "http" needs a url');
  }
  return errors;
}

/**
 * The config file the CLI core reads: {"mcpServers": {name: config}},
 * the same shape zen-mcp load_mcp_configs_scoped parses. Returned
 * as a JSON string so the caller (or the delegate below) can write
 * it to a file and pass --mcp-config <path> to `zen`.
 */
export function mcpConfigFileJson(
  name: string,
  config: McpServerConfig,
): string {
  return JSON.stringify({ mcpServers: { [name]: config } }, null, 2);
}

/**
 * Write that config file into a fresh temp dir and return its path.
 * The caller owns the file: remove the whole dir when done (the
 * delegate does this in a finally block).
 */
export function writeMcpConfigFile(
  name: string,
  config: McpServerConfig,
): { dir: string; path: string } {
  const dir = mkdtempSync(join(tmpdir(), "sen-mcp-"));
  const path = join(dir, "mcp-servers.json");
  writeFileSync(path, mcpConfigFileJson(name, config), "utf8");
  return { dir, path };
}

/** One tool a server offers (tools/list). */
export interface McpTool {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

/** One tool result (tools/call). content is the MCP content array. */
export interface McpToolResult {
  content: unknown[];
  isError?: boolean;
  [key: string]: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function asMcpTool(value: unknown): McpTool {
  if (!isRecord(value) || typeof value.name !== "string" || !value.name) {
    throw new Error(
      "MCP_PROTOCOL_ERROR: server listed a tool without a name: " +
        JSON.stringify(value),
    );
  }
  const tool: McpTool = { name: value.name };
  if (typeof value.description === "string") {
    tool.description = value.description;
  }
  if (isRecord(value.inputSchema)) {
    tool.inputSchema = value.inputSchema;
  }
  return tool;
}

function asMcpToolResult(value: unknown): McpToolResult {
  if (!isRecord(value) || !Array.isArray(value.content)) {
    throw new Error(
      "MCP_PROTOCOL_ERROR: tools/call returned no content array: " +
        JSON.stringify(value),
    );
  }
  const result: McpToolResult = { content: value.content };
  if (typeof value.isError === "boolean") {
    result.isError = value.isError;
  }
  for (const [key, entry] of Object.entries(value)) {
    if (key !== "content" && key !== "isError") {
      result[key] = entry;
    }
  }
  return result;
}

/**
 * A transport moves JSON-RPC messages. The client only ever sends
 * one JSON object per send(); the stdio transport frames it as one
 * line, the HTTP transport as one POST body. The server side of the
 * boundary is the only thing a test fakes (a fake server process /
 * a local HTTP server); the client code itself is never mocked.
 */
export interface McpTransport {
  send(message: string): void;
  onMessage(handler: (message: string) => void): void;
  onClose?(handler: () => void): void;
  close(): void | Promise<void>;
}

/** stdio transport: spawn the server, newline-delimited JSON. */
export class StdioMcpTransport implements McpTransport {
  private child: ChildProcess;
  private handler: ((message: string) => void) | null = null;
  private closeHandler: (() => void) | null = null;
  private buffer = "";
  private closed = false;
  readonly stderrTail: string[] = [];

  constructor(config: McpServerConfig) {
    const command = config.command as string;
    this.child = spawn(command, config.args ?? [], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, ...(config.env ?? {}) },
    });
    this.child.stdout?.on("data", (chunk: Buffer) => {
      this.buffer += chunk.toString("utf8");
      let at = this.buffer.indexOf("\n");
      while (at >= 0) {
        const line = this.buffer.slice(0, at).trim();
        this.buffer = this.buffer.slice(at + 1);
        if (line) {
          this.handler?.(line);
        }
        at = this.buffer.indexOf("\n");
      }
    });
    this.child.stderr?.on("data", (chunk: Buffer) => {
      // Stderr holds server warnings/diagnostics, never the stream;
      // keep only the tail so a chatty server cannot grow memory.
      this.stderrTail.push(chunk.toString("utf8"));
      while (this.stderrTail.join("").length > 8192) {
        this.stderrTail.shift();
      }
    });
    this.child.on("error", () => {
      this.closed = true;
      this.closeHandler?.();
    });
    this.child.on("close", () => {
      this.closed = true;
      this.closeHandler?.();
    });
  }

  get isClosed(): boolean {
    return this.closed;
  }

  send(message: string): void {
    if (this.closed || !this.child.stdin || this.child.stdin.destroyed) {
      throw new Error("MCP_SERVER_CLOSED: cannot send, server is gone");
    }
    this.child.stdin.write(message + "\n");
  }

  onMessage(handler: (message: string) => void): void {
    this.handler = handler;
  }

  onClose(handler: () => void): void {
    this.closeHandler = handler;
  }

  close(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.child.kill();
  }
}

/**
 * Streamable HTTP transport: POST one JSON-RPC object, read the
 * reply as a single JSON object or as an SSE stream (data: lines).
 * One POST per request; no GET stream is held open (the server is
 * asked for a reply, not a subscription).
 */
export class HttpMcpTransport implements McpTransport {
  private handler: ((message: string) => void) | null = null;
  private closed = false;
  private readonly url: string;
  private readonly headers: Record<string, string>;

  constructor(url: string, headers: Record<string, string> = {}) {
    this.url = url;
    this.headers = headers;
  }

  send(message: string): void {
    if (this.closed) {
      throw new Error("MCP_SERVER_CLOSED: cannot send, transport is closed");
    }
    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      ...this.headers,
    };
    // Fire-and-forget at this layer: the reply is delivered through
    // onMessage. The client awaits it via its pending-request map,
    // so a POST that never answers surfaces as MCP_TIMEOUT there.
    // A network failure is reported the same way, as a synthetic
    // JSON-RPC error object, so the client path stays one shape.
    void fetch(this.url, { method: "POST", headers, body: message })
      .then(async (res) => {
        const text = await res.text();
        const contentType = res.headers.get("content-type") ?? "";
        if (contentType.includes("text/event-stream")) {
          for (const line of text.split("\n")) {
            const data = line.startsWith("data:")
              ? line.slice("data:".length).trim()
              : "";
            if (data && data !== "[DONE]") {
              this.handler?.(data);
            }
          }
        } else if (text.trim()) {
          this.handler?.(text.trim());
        }
      })
      .catch((error: unknown) => {
        const detail =
          error instanceof Error ? error.message : String(error);
        // No request id is known here, so the client cannot route
        // this to one pending call; surface it as a protocol error
        // on the next inbound message instead is wrong - instead,
        // deliver a synthetic error reply for the in-flight request
        // by reusing the id from the sent message.
        let id: unknown = null;
        try {
          id = (JSON.parse(message) as { id?: unknown }).id ?? null;
        } catch {
          id = null;
        }
        this.handler?.(
          JSON.stringify({
            jsonrpc: "2.0",
            id,
            error: { code: -32000, message: `MCP_HTTP_ERROR: ${detail}` },
          }),
        );
      });
  }

  onMessage(handler: (message: string) => void): void {
    this.handler = handler;
  }

  close(): void {
    this.closed = true;
  }
}

export interface McpClientOptions {
  timeoutMs?: number;
  protocolVersion?: string;
  clientName?: string;
  clientVersion?: string;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/**
 * The MCP client: JSON-RPC 2.0 over any McpTransport. One client,
 * one server. Request ids are monotonic per client; a request that
 * gets no reply within timeoutMs fails with MCP_TIMEOUT; a
 * JSON-RPC error reply fails with MCP_PROTOCOL_ERROR (or
 * MCP_TOOL_ERROR for tools/call). A server request (a message with
 * a method and an id, but no result/error) is answered with
 * -32601 Method not found: this client implements no server-side
 * method. Malformed lines are ignored: they cannot be correlated
 * to a request, and a server may interleave diagnostics on stdout.
 */
export class McpClient {
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private initialized = false;
  private closed = false;
  private readonly timeoutMs: number;
  private readonly protocolVersion: string;
  private readonly clientName: string;
  private readonly clientVersion: string;
  private readonly transport: McpTransport;

  constructor(transport: McpTransport, options: McpClientOptions = {}) {
    this.transport = transport;
    this.timeoutMs = options.timeoutMs ?? MCP_DEFAULT_TIMEOUT_MS;
    this.protocolVersion =
      options.protocolVersion ?? MCP_PROTOCOL_VERSION;
    this.clientName = options.clientName ?? MCP_CLIENT_NAME;
    this.clientVersion = options.clientVersion ?? MCP_CLIENT_VERSION;
    this.transport.onMessage((message) => this.onMessage(message));
    this.transport.onClose?.(() => this.onTransportClose());
  }

  get isInitialized(): boolean {
    return this.initialized;
  }

  get isClosed(): boolean {
    return this.closed;
  }

  /** Full handshake: initialize + initialized notification. */
  async initialize(): Promise<Record<string, unknown>> {
    const result = await this.request("initialize", {
      protocolVersion: this.protocolVersion,
      capabilities: {},
      clientInfo: { name: this.clientName, version: this.clientVersion },
    });
    if (!isRecord(result)) {
      throw new Error(
        "MCP_PROTOCOL_ERROR: initialize returned a non-object result",
      );
    }
    this.initialized = true;
    this.notify("notifications/initialized", {});
    return result;
  }

  async listTools(): Promise<McpTool[]> {
    this.requireInitialized();
    const result = await this.request("tools/list", {});
    if (!isRecord(result) || !Array.isArray(result.tools)) {
      throw new Error(
        "MCP_PROTOCOL_ERROR: tools/list returned no tools array: " +
          JSON.stringify(result),
      );
    }
    return result.tools.map((tool) => asMcpTool(tool));
  }

  async callTool(
    name: string,
    args: Record<string, unknown> = {},
  ): Promise<McpToolResult> {
    this.requireInitialized();
    if (!name.trim()) {
      throw new Error("MCP_VALIDATION: tool name must be non-empty");
    }
    try {
      const result = await this.request("tools/call", {
        name,
        arguments: args,
      });
      return asMcpToolResult(result);
    } catch (error) {
      // A tools/call JSON-RPC error is a tool failure, not a
      // protocol failure: re-label it so callers can tell the two
      // apart (a timeout stays MCP_TIMEOUT either way).
      if (
        error instanceof Error &&
        error.message.startsWith("MCP_PROTOCOL_ERROR")
      ) {
        throw new Error(
          error.message.replace(
            "MCP_PROTOCOL_ERROR",
            "MCP_TOOL_ERROR",
          ),
        );
      }
      throw error;
    }
  }

  async ping(): Promise<void> {
    this.requireInitialized();
    await this.request("ping", {});
  }

  async close(): Promise<void> {
    if (this.closed) {
      return;
    }
    this.closed = true;
    for (const [, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(new Error("MCP_CLIENT_CLOSED: client closed"));
    }
    this.pending.clear();
    await this.transport.close();
  }

  private requireInitialized(): void {
    if (!this.initialized) {
      throw new Error(
        "MCP_NOT_INITIALIZED: call initialize() before tools/list, tools/call, or ping",
      );
    }
  }

  private notify(method: string, params: Record<string, unknown>): void {
    this.transport.send(
      JSON.stringify({ jsonrpc: "2.0", method, params }),
    );
  }

  private request(
    method: string,
    params: Record<string, unknown>,
  ): Promise<unknown> {
    if (this.closed) {
      return Promise.reject(
        new Error("MCP_CLIENT_CLOSED: client closed"),
      );
    }
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          new Error(
            `MCP_TIMEOUT: no reply to ${method} within ${this.timeoutMs}ms`,
          ),
        );
      }, this.timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      try {
        this.transport.send(
          JSON.stringify({ jsonrpc: "2.0", id, method, params }),
        );
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(
          error instanceof Error
            ? error
            : new Error(`MCP_SEND_FAILED: ${String(error)}`),
        );
      }
    });
  }

  private onTransportClose(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    for (const [, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(
        new Error("MCP_SERVER_EXITED: server closed the transport"),
      );
    }
    this.pending.clear();
  }

  private onMessage(message: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(message);
    } catch {
      return;
    }
    if (!isRecord(parsed) || parsed.jsonrpc !== "2.0") {
      return;
    }
    // Server request: no server-side method exists on this client.
    if (
      typeof parsed.method === "string" &&
      parsed.id !== undefined &&
      parsed.result === undefined &&
      parsed.error === undefined
    ) {
      this.transport.send(
        JSON.stringify({
          jsonrpc: "2.0",
          id: parsed.id,
          error: { code: -32601, message: "Method not found" },
        }),
      );
      return;
    }
    // Notification from the server: nothing to do with it.
    if (typeof parsed.method === "string") {
      return;
    }
    if (typeof parsed.id !== "number") {
      return;
    }
    const pending = this.pending.get(parsed.id);
    if (!pending) {
      return;
    }
    this.pending.delete(parsed.id);
    clearTimeout(pending.timer);
    if (isRecord(parsed.error)) {
      const code =
        typeof parsed.error.code === "number" ? parsed.error.code : "";
      const detail =
        typeof parsed.error.message === "string"
          ? parsed.error.message
          : JSON.stringify(parsed.error);
      pending.reject(
        new Error(`MCP_PROTOCOL_ERROR: server error ${code}: ${detail}`),
      );
      return;
    }
    pending.resolve(parsed.result ?? null);
  }
}

/** Connect to a stdio server and run the handshake. */
export async function connectStdioMcp(
  config: McpServerConfig,
  options: McpClientOptions = {},
): Promise<McpClient> {
  const errors = validateMcpServerConfig(config);
  if (errors.length > 0) {
    throw new Error("MCP_CONFIG_INVALID: " + errors.join("; "));
  }
  if (!config.command) {
    throw new Error("MCP_CONFIG_INVALID: stdio server needs a command");
  }
  const client = new McpClient(new StdioMcpTransport(config), options);
  try {
    await client.initialize();
  } catch (error) {
    await client.close();
    throw error;
  }
  return client;
}

/** Connect to a Streamable HTTP server and run the handshake. */
export async function connectHttpMcp(
  config: McpServerConfig,
  options: McpClientOptions = {},
): Promise<McpClient> {
  const errors = validateMcpServerConfig(config);
  if (errors.length > 0) {
    throw new Error("MCP_CONFIG_INVALID: " + errors.join("; "));
  }
  if (!config.url) {
    throw new Error("MCP_CONFIG_INVALID: HTTP server needs a url");
  }
  const client = new HttpMcpTransport(
    config.url,
    config.headers ?? {},
  );
  const mcp = new McpClient(client, options);
  try {
    await mcp.initialize();
  } catch (error) {
    await mcp.close();
    throw error;
  }
  return mcp;
}

// --- Approval-gated execution ----------------------------------------

export interface McpGateContext {
  policy?: ToolPolicy;
  approvals?: ApprovalStore;
  /** When given, every evaluation is recorded, exactly once. */
  log?: ActivityLog;
}

export interface McpToolCallInput {
  actor: string;
  /** Server name (for the log reason); the client is already connected. */
  server: string;
  tool: string;
  arguments?: Record<string, unknown>;
  idempotencyKey: string;
  /** The live session asking for the tool: the CLI --session-id
   * UUID (input.sessionId ?? randomUUID() in the delegate path).
   * The approval must belong to this session, not just to this
   * tool + key. */
  sessionId: string;
}

/**
 * Call an MCP tool through the P0-2 gate. The gate classifies the
 * tool by its MCP name (unknown names are ask-first, fail closed)
 * and evaluates against the explicit allow-list. A non-allow
 * decision throws with the P0-2 code (TOOL_POLICY_DENIED /
 * APPROVAL_REQUIRED / APPROVAL_NEVER_DENIED) and the tool never
 * runs; an allow decision runs tools/call on the connected server.
 * An allow for an ask-first tool spends the approval (P1-4): one
 * grant authorizes exactly one call, a retry needs a fresh grant.
 * The evaluation is recorded in the log exactly once when a log is
 * given. A tool result with isError: true is still a completed
 * call - it is returned, not thrown.
 */
export async function executeMcpTool(
  client: McpClient,
  gate: McpGateContext,
  input: McpToolCallInput,
): Promise<{ result: McpToolResult; evaluation: GateEvaluation }> {
  const policy = gate.policy ?? { allow: [] };
  const approvals = gate.approvals ?? new ApprovalStore();
  if (!input.sessionId?.trim()) {
    throw new Error("MCP_VALIDATION: sessionId must be a non-empty string");
  }
  const evaluation = gate.log
    ? gateToolCall(policy, approvals, gate.log, {
        actor: input.actor,
        tool: input.tool,
        idempotencyKey: input.idempotencyKey,
        sessionId: input.sessionId,
      })
    : evaluateToolCall(policy, approvals, {
        actor: input.actor,
        tool: input.tool,
        idempotencyKey: input.idempotencyKey,
        sessionId: input.sessionId,
      });
  if (evaluation.decision !== "allow") {
    if (evaluation.decision === "ask-first") {
      throw new Error("APPROVAL_REQUIRED: " + evaluation.reason);
    }
    if (evaluation.tier === "never") {
      throw new Error("APPROVAL_NEVER_DENIED: " + evaluation.reason);
    }
    throw new Error("TOOL_POLICY_DENIED: " + evaluation.reason);
  }
  // An allow for an ask-first tool spends the approval (P1-4): one
  // grant authorizes exactly one call. The logged path already
  // spent it inside gateToolCall, so this second consume is a
  // no-op there; the unlogged path spends it here.
  if (evaluation.tier === "ask-first") {
    approvals.consume(input.tool, input.idempotencyKey, input.sessionId);
  }
  const result = await client.callTool(
    input.tool,
    input.arguments ?? {},
  );
  return { result, evaluation };
}

// --- Delegate to the ZenCore CLI core ---------------------------------

export interface CliDelegateInput {
  /** Path to the zen binary (default "zen" on PATH). */
  zenBin?: string;
  /** Session id for --session-id / -r; a fresh bare UUID when absent.
   * A caller-supplied id must be a valid UUID: the CLI parses
   * --session-id with Uuid::parse_str and rejects anything else
   * (exit 4, "must be a valid UUID"). */
  sessionId?: string;
  /** Resume an existing CLI session instead of starting one. */
  resume?: boolean;
  serverName: string;
  serverConfig: McpServerConfig;
  tool: string;
  arguments?: Record<string, unknown>;
  /** Model flag for the turn; absent means the CLI default. */
  model?: string;
  /** Base URL flag for the turn; absent means the CLI default. */
  baseUrl?: string;
  timeoutMs?: number;
}

export interface CliDelegateResult {
  /** Text of the stream-json result record. */
  text: string;
  sessionId: string;
  exitCode: number | null;
}

/**
 * The prompt handed to the CLI agent for one delegated tool call.
 * It names the server, the tool, and the exact JSON arguments, and
 * asks for only the tool result back: the CLI agent loop (not Sen)
 * decides how to call the tool through its own zen-mcp manager.
 */
export function buildDelegatePrompt(
  serverName: string,
  tool: string,
  args: Record<string, unknown>,
): string {
  return (
    `Call the MCP tool "${tool}" on MCP server "${serverName}" ` +
    `with arguments ${JSON.stringify(args)}. ` +
    `Reply with only the tool result, no extra text.`
  );
}

/**
 * Build the `zen -p` argv for a delegated turn. Same flags as the
 * CLI bridge (cli_spawn.rs): -p, stream-json, --bare,
 * --setting-sources "", plus --mcp-config <file> so the CLI core
 * starts the server with its own zen-mcp manager. The prompt is
 * the last argv element, preceded by `--` so a prompt starting
 * with `-` is passed as the prompt value, never parsed as a CLI
 * flag (same fix as Rust P1-1 in cli_spawn.rs build_cli_argv).
 * Flags never follow it.
 */
export function buildDelegateArgv(
  input: CliDelegateInput,
  configPath: string,
  sessionId: string,
): string[] {
  const argv: string[] = [
    "-p",
    "--output-format",
    "stream-json",
    "--bare",
    "--setting-sources",
    "",
    "--mcp-config",
    configPath,
  ];
  if (input.model) {
    argv.push("--model", input.model);
  }
  if (input.baseUrl) {
    argv.push("--base-url", input.baseUrl);
  }
  if (input.resume) {
    argv.push("-r", sessionId);
  } else {
    argv.push("--session-id", sessionId);
  }
  // `--` ends flag parsing, so the prompt that follows is always the
  // positional prompt, even when it starts with `-`. The prompt is
  // built by buildDelegatePrompt from server/tool/args, which can
  // carry user text; without the separator a prompt starting with
  // `-` (e.g. "- fix bug", "--help") would be parsed as a CLI flag.
  argv.push("--");
  argv.push(
    buildDelegatePrompt(
      input.serverName,
      input.tool,
      input.arguments ?? {},
    ),
  );
  return argv;
}

interface StreamJsonLine {
  type?: unknown;
  subtype?: unknown;
  result?: unknown;
  text?: unknown;
  is_error?: unknown;
  isError?: unknown;
}

/**
 * True when `value` is a UUID the CLI accepts for --session-id:
 * the hyphenated form `randomUUID()` produces, or the 32-hex simple
 * form. Mirrors the CLI's `Uuid::parse_str` (zen-cli cli.rs
 * `parse_session_uuid`): anything else is rejected at parse time
 * with exit 4 ("must be a valid UUID").
 */
export function isValidUuid(value: string): boolean {
  return (
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      value,
    ) || /^[0-9a-f]{32}$/i.test(value)
  );
}

/**
 * Delegate one MCP tool call to the ZenCore CLI core: write the
 * server config to a temp file, run `zen -p --output-format
 * stream-json --bare --setting-sources "" --mcp-config <file>`
 * with a prompt naming the tool, and return the text of the
 * stream-json result record. The agent loop, the subagent depth
 * limit, the permission checks, and the MCP server lifecycle all
 * belong to the CLI core; Sen only asks and reads the result.
 * stdin is null (like the bridge): the CLI must not wait on it.
 * The temp config dir is removed in a finally block. A turn with
 * no result record within timeoutMs is killed and reported as
 * CLI_TURN_TIMEOUT, the same code as the bridge.
 */
export async function delegateToolCallToCliCore(
  input: CliDelegateInput,
): Promise<CliDelegateResult> {
  const configErrors = validateMcpServerConfig(input.serverConfig);
  if (configErrors.length > 0) {
    throw new Error("MCP_CONFIG_INVALID: " + configErrors.join("; "));
  }
  if (!input.serverName.trim()) {
    throw new Error("MCP_VALIDATION: serverName must be non-empty");
  }
  if (!input.tool.trim()) {
    throw new Error("MCP_VALIDATION: tool must be non-empty");
  }
  // The CLI parses --session-id with Uuid::parse_str and rejects
  // anything that is not a UUID (exit 4). The default is therefore
  // a bare randomUUID(), and a caller-supplied id is validated here,
  // before any spawn, so a bad id fails fast instead of failing the
  // turn after the CLI has started (P0-1).
  const sessionId = input.sessionId ?? randomUUID();
  if (!isValidUuid(sessionId)) {
    throw new Error("MCP_VALIDATION: sessionId must be a valid UUID");
  }
  const timeoutMs = input.timeoutMs ?? MCP_DELEGATE_TIMEOUT_MS;
  const { dir, path } = writeMcpConfigFile(
    input.serverName,
    input.serverConfig,
  );
  try {
    const argv = buildDelegateArgv(input, path, sessionId);
    const zenBin = input.zenBin ?? "zen";
    return await new Promise<CliDelegateResult>((resolve, reject) => {
      const child = spawn(zenBin, argv, { stdio: ["ignore", "pipe", "pipe"] });
      let buffer = "";
      let stderrTail = "";
      let resultText: string | null = null;
      let resultIsError = false;
      let settled = false;
      const timer = setTimeout(() => {
        if (settled) {
          return;
        }
        settled = true;
        child.kill("SIGKILL");
        reject(
          new Error(
            `CLI_TURN_TIMEOUT: no result record within ${timeoutMs}ms`,
          ),
        );
      }, timeoutMs);
      const finish = (error?: Error) => {
        if (settled) {
          return;
        }
        settled = true;
        clearTimeout(timer);
        if (error) {
          reject(error);
          return;
        }
        if (resultText === null) {
          reject(
            new Error(
              "CLI_NO_RESULT: CLI exited without a result record" +
                (stderrTail ? `: ${stderrTail.slice(-500)}` : ""),
            ),
          );
          return;
        }
        if (resultIsError) {
          reject(new Error(`CLI_DELEGATE_ERROR: ${resultText}`));
          return;
        }
        resolve({ text: resultText, sessionId, exitCode: child.exitCode });
      };
      child.stdout?.on("data", (chunk: Buffer) => {
        buffer += chunk.toString("utf8");
        let at = buffer.indexOf("\n");
        while (at >= 0) {
          const line = buffer.slice(0, at).trim();
          buffer = buffer.slice(at + 1);
          if (line) {
            try {
              const record = JSON.parse(line) as StreamJsonLine;
              if (record.type === "result") {
                const text =
                  typeof record.result === "string"
                    ? record.result
                    : typeof record.text === "string"
                      ? record.text
                      : "";
                resultText = text;
                resultIsError =
                  record.is_error === true || record.isError === true ||
                  record.subtype === "error";
              }
            } catch {
              // A non-JSON line on stdout is ignored: only the
              // result record decides the turn.
            }
          }
          at = buffer.indexOf("\n");
        }
      });
      child.stderr?.on("data", (chunk: Buffer) => {
        stderrTail += chunk.toString("utf8");
        if (stderrTail.length > 8192) {
          stderrTail = stderrTail.slice(-8192);
        }
      });
      child.on("error", (error: Error) => {
        finish(
          new Error(`CLI_SPAWN_FAILED: cannot start ${zenBin}: ${error.message}`),
        );
      });
      child.on("close", () => finish());
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
