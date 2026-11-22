// Chat command parser (P0-1). Memory commands are executed locally
// by the chat layer and never reach the model: a `quên` must delete a
// fact, not ask the model to talk about forgetting.
//
// Which inputs are commands:
// - Everything starting with "/" is a command (or an unknown command,
//   answered with the command list instead of being sent to the model).
// - A bare "quên ..." is also a command: "quên" at the start of a
//   message means "forget ...", and no ordinary sentence starts that
//   way. Bare "nhớ ..." is NOT a command: "nhớ" starts ordinary
//   sentences ("nhớ hôm qua..."), so remembering a fact needs the slash.
// - Anything else is a normal message (null: not a command).
//
// All numbers are 1-based item numbers, matching the numbers the
// memory list shows.

export type ChatCommand =
  | { kind: "memory-add"; scope: "memory" | "user"; text: string }
  | {
      kind: "memory-forget";
      scope: "memory" | "user";
      target: { number: number } | { text: string };
    }
  | {
      kind: "memory-edit";
      scope: "memory" | "user";
      index: number;
      text: string;
    }
  | { kind: "soul-set"; text: string }
  | { kind: "memory-list" }
  | { kind: "memory-log" }
  // P0-3: /model shows the active route + chain; /model <id> switches
  // the preferred route (new sessions use it; a running session keeps
  // its route). Never sent to the model.
  | { kind: "model-show" }
  | { kind: "model-set"; routeId: string }
  // CI P2-9: /brief builds the morning brief from the injected
  // brief source (ChatSession); /mcp <server> <tool> [args-json]
  // delegates one MCP tool call to the CLI core through the injected
  // delegate. Both are local commands, never sent to the model.
  | { kind: "morning-brief" }
  | {
      kind: "mcp-call";
      server: string;
      tool: string;
      args: Record<string, unknown>;
    }
  | { kind: "unknown"; text: string };

function forgetTarget(
  rest: string,
): { number: number } | { text: string } {
  const trimmed = rest.trim();
  return /^\d+$/.test(trimmed)
    ? { number: Number(trimmed) }
    : { text: trimmed };
}

export function parseChatCommand(text: string): ChatCommand | null {
  const trimmed = text.trim();
  if (!trimmed) {
    return null;
  }
  // Forget: "/quên ..." or bare "quên ...", with an optional "tôi"
  // (user scope) before the target.
  const forgetUser = /^\/?quên\s+tôi\s+(.+)$/.exec(trimmed);
  if (forgetUser) {
    return {
      kind: "memory-forget",
      scope: "user",
      target: forgetTarget(forgetUser[1]),
    };
  }
  const forget = /^\/?quên\s+(.+)$/.exec(trimmed);
  if (forget) {
    return {
      kind: "memory-forget",
      scope: "memory",
      target: forgetTarget(forget[1]),
    };
  }
  const remember = /^\/nhớ\s+(.+)$/.exec(trimmed);
  if (remember) {
    return { kind: "memory-add", scope: "memory", text: remember[1].trim() };
  }
  const aboutMe = /^\/tôi\s+(.+)$/.exec(trimmed);
  if (aboutMe) {
    return { kind: "memory-add", scope: "user", text: aboutMe[1].trim() };
  }
  const soul = /^\/sửa\s+soul\s+(.+)$/.exec(trimmed);
  if (soul) {
    return { kind: "soul-set", text: soul[1].trim() };
  }
  const edit = /^\/sửa(?:\s+tôi)?\s+(\d+)\s+(.+)$/.exec(trimmed);
  if (edit) {
    const scope = /^\/sửa\s+tôi\s+\d+/.test(trimmed) ? "user" : "memory";
    return {
      kind: "memory-edit",
      scope,
      index: Number(edit[1]),
      text: edit[2].trim(),
    };
  }
  if (/^\/xem\s*$/.test(trimmed)) {
    return { kind: "memory-list" };
  }
  if (/^\/nhật ký\s*$/.test(trimmed)) {
    return { kind: "memory-log" };
  }
  if (/^\/model\s*$/.test(trimmed)) {
    return { kind: "model-show" };
  }
  const modelSet = /^\/model\s+(\S+)\s*$/.exec(trimmed);
  if (modelSet) {
    return { kind: "model-set", routeId: modelSet[1] };
  }
  if (/^\/brief\s*$/.test(trimmed)) {
    return { kind: "morning-brief" };
  }
  const mcpCall = /^\/mcp\s+(\S+)\s+(\S+)(?:\s+(.+))?$/.exec(trimmed);
  if (mcpCall) {
    const rest = (mcpCall[3] ?? "").trim();
    let args: Record<string, unknown> = {};
    if (rest) {
      let parsed: unknown;
      try {
        parsed = JSON.parse(rest);
      } catch {
        return { kind: "unknown", text: trimmed };
      }
      if (
        !parsed ||
        typeof parsed !== "object" ||
        Array.isArray(parsed)
      ) {
        return { kind: "unknown", text: trimmed };
      }
      args = parsed as Record<string, unknown>;
    }
    return {
      kind: "mcp-call",
      server: mcpCall[1] as string,
      tool: mcpCall[2] as string,
      args,
    };
  }
  if (trimmed.startsWith("/")) {
    return { kind: "unknown", text: trimmed };
  }
  return null;
}

/// Short usage text for the unknown-command reply.
export const COMMAND_USAGE =
  "Lệnh không rõ. Các lệnh: /nhớ <nội dung>, /tôi <nội dung>, " +
  "/quên <số hoặc chữ>, /quên tôi <số hoặc chữ>, /sửa <số> <nội dung>, " +
  "/sửa tôi <số> <nội dung>, /sửa soul <nội dung>, /xem, /nhật ký, " +
  "/model, /model <tên đường: spark, free, ollama>, " +
  "/brief, /mcp <server> <tool> [args-json].";
