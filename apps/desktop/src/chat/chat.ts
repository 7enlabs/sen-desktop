// Shared chat session (P0-1): one agent context for the desktop chat
// and the Telegram bridge.
//
// One ChatSession owns:
// - the transcript both channels render from (desktop UI + Telegram
//   replies are derived from the same entries),
// - the send queue (one turn at a time: a second send waits for the
//   first reply, so entries never overtake each other),
// - the memory commands (executed locally, never sent to the model),
// - the daily-log writes (user + assistant + memory-op lines).
//
// Turns run through the IPC bridge (steer first, follow_up after) on
// the session startApp created. The assistant's reply text arrives as
// a "result" SenEvent through handleEvent: send() waits for that event
// (not just for the steer ack), so the transcript entry lands before
// the next queued send runs. A turn that errors (steer throws, the
// result never arrives within the contract timeout + 10s, or the
// result record carries isError) becomes a system entry instead of a
// hang or a fake assistant reply, and a Telegram turn also gets its
// error text sent back to the chat.
import {
  followUpSession,
  steerSession,
  type BackendSessionResult,
} from "../ipc/session-client.ts";
import type { SenEvent } from "../ipc/session-events.ts";
import type { ActionContract } from "../ipc/types.ts";
import {
  addMemory,
  editMemory,
  forgetMemoryByNumber,
  forgetMemoryByText,
  logMemory,
  readMemory,
  setSoul,
  type LogChannel,
  type LogKind,
  type MemoryForgetResult,
  type MemoryItemResult,
  type MemoryReadResult,
  type MemoryScope,
  type SoulResult,
} from "../ipc/memory-client.ts";
import { COMMAND_USAGE, parseChatCommand, type ChatCommand } from "./commands.ts";
import {
  buildMorningBrief,
  renderMorningBriefText,
  type CalendarEvent,
  type EmailItem,
} from "../../../../packages/core/src/morning-brief.ts";
import { renderMorningBriefHtml } from "../app/morning-brief-ui.ts";
// Type-only: McpClient and delegateToolCallToCliCore are node-only
// (node:child_process / node:fs), so this module must never import
// their VALUES — a value import here would enter the webview graph
// (main.ts -> chat-ui -> chat) and break vite build. The runtime
// functions arrive as injected options instead (see
// ChatSessionOptions); the wiring-p2-9 suite locks the graph.
import type {
  CliDelegateInput,
  CliDelegateResult,
  McpServerConfig,
} from "../../../../packages/core/src/mcp-client.ts";
import type { SkillMeta } from "../ipc/skills-client.ts";
import {
  describeRouterRoute,
  type RouterSettingResult,
} from "../ipc/settings-client.ts";

/// The model-router calls the chat layer needs (Sen P0-3). The
/// default is the real settings client, wired by the app at boot;
/// tests inject a fake. A missing client means /model only reports
/// the CLI free default, never a fake route list.
export interface RouterClient {
  getRouter(contract: ActionContract): Promise<RouterSettingResult>;
  setPreferredRoute(
    contract: ActionContract,
    routeId: string,
  ): Promise<RouterSettingResult>;
}

/// One-line router status for the UI status line, e.g.
/// "Model: spark · google/gemini-4-argon:free · 0 đồng".
export function formatRouterStatus(status: RouterSettingResult): string {
  const active = status.routes.find((r) => r.id === status.activeRouteId);
  const text = active
    ? describeRouterRoute(active)
    : status.activeRouteId;
  return "Model: " + text;
}

/// Full router listing for the /model reply: active route first,
/// then the rest of the chain in order, each with its cost.
export function formatRouterList(status: RouterSettingResult): string {
  const lines = status.routes.map((route) => {
    const mark = route.id === status.activeRouteId ? "● " : "○ ";
    return mark + describeRouterRoute(route);
  });
  const fallback = status.fallbackEnabled
    ? "Lỗi ở một đường sẽ thử đường tiếp theo."
    : "Fallback đang tắt: lỗi ở một đường sẽ báo lỗi luôn.";
  return "Đường model:\n" + lines.join("\n") + "\n" + fallback;
}

export type ChatChannel = "desktop" | "telegram";
export type TranscriptRole = "user" | "assistant" | "system";

export interface TranscriptEntry {
  id: number;
  channel: ChatChannel;
  role: TranscriptRole;
  text: string;
  ts: string;
  telegramChatId?: number;
}

/// The memory operations the chat layer needs. The default is the real
/// memory client; tests inject a fake.
export interface MemoryClient {
  read(contract: ActionContract): Promise<MemoryReadResult>;
  add(
    contract: ActionContract,
    scope: MemoryScope,
    text: string,
  ): Promise<MemoryItemResult>;
  forgetByNumber(
    contract: ActionContract,
    scope: MemoryScope,
    number: number,
  ): Promise<MemoryForgetResult>;
  forgetByText(
    contract: ActionContract,
    scope: MemoryScope,
    text: string,
  ): Promise<MemoryForgetResult>;
  edit(
    contract: ActionContract,
    scope: MemoryScope,
    index: number,
    text: string,
  ): Promise<MemoryItemResult>;
  setSoul(contract: ActionContract, text: string): Promise<SoulResult>;
  log(
    contract: ActionContract,
    kind: LogKind,
    channel: LogChannel,
    text: string,
  ): Promise<void>;
}

const realMemoryClient: MemoryClient = {
  read: (contract) => readMemory(contract),
  add: (contract, scope, text) => addMemory(contract, scope, text),
  forgetByNumber: (contract, scope, number) =>
    forgetMemoryByNumber(contract, scope, number),
  forgetByText: (contract, scope, text) =>
    forgetMemoryByText(contract, scope, text),
  edit: (contract, scope, index, text) =>
    editMemory(contract, scope, index, text),
  setSoul: (contract, text) => setSoul(contract, text),
  log: async (contract, kind, channel, text) => {
    await logMemory(contract, kind, channel, text);
  },
};

export interface ChatSessionOptions {
  /// Build a fresh contract per send (unique id + idempotency key).
  /// The actor is the channel ("desktop" | "telegram").
  makeContract: (channel: ChatChannel, purpose: string) => ActionContract;
  memory?: MemoryClient;
  /// Model-router client (P0-3). When omitted, /model only reports
  /// the CLI free default.
  router?: RouterClient;
  /// Send a reply back to a Telegram chat. Defaults to a no-op so a
  /// session without Telegram still works; the app passes the real
  /// sender at boot.
  sendTelegram?: (chatId: number, text: string) => Promise<void>;
  onTranscript?: (entries: TranscriptEntry[]) => void;
  onBusy?: (busy: boolean) => void;
  /// Router status line for the UI (P0-3). Called after a /model
  /// change, by refreshRouterStatus(), and after every model turn
  /// with the route that actually ran (rev2 F1: after a fallback the
  /// preferred route is still the route that failed, so the turn
  /// result — not the stored preference — decides the line).
  onRouterStatus?: (text: string) => void;
  /// Morning-brief data source (CI P2-9, P0-4). When provided, /brief
  /// and a morning-brief skill match build a real brief from it with
  /// buildMorningBrief. When omitted, the brief commands say so
  /// honestly: there is no Gmail/calendar fetch in the app yet, so
  /// no source is invented here.
  briefSource?: () => Promise<{
    emails: EmailItem[];
    events: CalendarEvent[];
  }>;
  /// Called with the rendered brief HTML when a brief is built, so
  /// the desktop UI can show the rich panel (chat-ui showBrief).
  /// The HTML is fully escaped by renderMorningBriefHtml.
  onBrief?: (html: string) => void;
  /// Skill matcher (Tauri follow-up 3, 19/11): an async function
  /// backed by the Rust SkillRegistry command (sen_skill_match),
  /// returning skill METADATA only — the frontend matches on
  /// metadata, never on a skill body. A plain message matching
  /// the morning-brief skill runs the local brief flow instead of
  /// a model turn; any other match, or no matcher, falls through
  /// to the model as before.
  skillMatcher?: (query: string) => Promise<SkillMeta | undefined>;
  /// MCP server configs by name (CI P2-9, P0-5). /mcp looks the
  /// server up here; an unknown name is an honest refusal, never a
  /// guessed config.
  mcpServers?: Record<string, McpServerConfig>;
  /// Delegate one MCP tool call to the CLI core (Tauri follow-up
  /// 1, 19/11): the Rust sen_mcp_delegate command, which spawns
  /// `zen -p --mcp-config <temp file>` on a fresh delegate bridge.
  /// Omitted in tests that do not inject it, so /mcp says so
  /// honestly until the delegate is wired.
  delegateToolCall?: (
    input: CliDelegateInput,
  ) => Promise<CliDelegateResult>;
}

interface PendingTurn {
  resolve: (result: { text: string; isError: boolean }) => void;
  reject: (error: Error) => void;
}

export class ChatSession {
  private readonly options: ChatSessionOptions;
  private readonly memory: MemoryClient;
  private entries: TranscriptEntry[] = [];
  private nextId = 1;
  private queue: Promise<void> = Promise.resolve();
  private sessionId: string | null = null;
  private started = false;
  private busy = false;
  private pending: PendingTurn | null = null;
  /// Set by end(): the session is over (Completed on the backend,
  /// via endLiveSession in the layer above). No new send runs after
  /// this — not a model turn, not a local command — and a send
  /// already queued but not started yet is refused at the head of
  /// sendNow instead of running a steer that fails
  /// ILLEGAL_TRANSITION on the backend. The backend call itself
  /// lives in the layer above (endLiveSession), never here: this
  /// class only knows steerSession/followUpSession, like the
  /// abort path, which also lives outside it.
  private ended = false;

  constructor(options: ChatSessionOptions) {
    this.options = options;
    this.memory = options.memory ?? realMemoryClient;
  }

  bind(sessionId: string): void {
    this.sessionId = sessionId;
  }

  getSessionId(): string | null {
    return this.sessionId;
  }

  /// End this chat locally after the backend ended the session
  /// (sen_session_end, Completed): no backend call here, no daily
  /// log entry, nothing sent to Telegram — one system entry in the
  /// in-memory transcript, for display only. Idempotent: a second
  /// end adds no second entry. After this, send() refuses every
  /// new message with the same system entry.
  end(): void {
    if (this.ended) {
      return;
    }
    this.ended = true;
    this.append("desktop", "system", "Phiên đã kết thúc.");
  }

  isEnded(): boolean {
    return this.ended;
  }

  getTranscript(): TranscriptEntry[] {
    return [...this.entries];
  }

  isBusy(): boolean {
    return this.busy;
  }

  /// Report the route a finished turn actually ran on (P0-3 rev2,
  /// F1). The steer/follow_up ack carries routeId/cost for the route
  /// that ran; the stored preference may still name the route that
  /// failed and fell back, so the ack — not a settings re-read —
  /// decides the line. When the router client is available the ack's
  /// route id is matched against the chain so the line keeps the
  /// "provider · model · cost" shape; otherwise (no router, unknown
  /// route, failed fetch) the line shows the route id and cost as
  /// returned. Never throws: the turn already landed, and a status
  /// update must not fail it. No-op when the ack carries no route
  /// (a backend from before P0-3).
  private async reportRanRoute(
    turnResult: BackendSessionResult | undefined,
  ): Promise<void> {
    const routeId = turnResult?.routeId;
    if (!routeId) {
      return;
    }
    const cost = turnResult.cost ?? "0 đồng";
    const router = this.options.router;
    if (router) {
      try {
        const contract = this.options.makeContract(
          "desktop",
          "router status",
        );
        const status = await router.getRouter(contract);
        if (status.routes.some((r) => r.id === routeId)) {
          this.options.onRouterStatus?.(
            formatRouterStatus({ ...status, activeRouteId: routeId }),
          );
          return;
        }
      } catch {
        // Fall through to the plain line below: the turn already ran,
        // the status line must not stay on the failed route.
      }
    }
    this.options.onRouterStatus?.("Model: " + routeId + " · " + cost);
  }

  /// Fetch the router status and report it on the status line
  /// (P0-3). Best effort: without a router client, or when the fetch
  /// fails, the line says the model is unknown instead of guessing.
  async refreshRouterStatus(): Promise<void> {
    const router = this.options.router;
    if (!router) {
      this.options.onRouterStatus?.("Model: đường miễn phí của CLI");
      return;
    }
    try {
      const contract = this.options.makeContract("desktop", "router status");
      const status = await router.getRouter(contract);
      this.options.onRouterStatus?.(formatRouterStatus(status));
    } catch {
      this.options.onRouterStatus?.("Model: chưa rõ");
    }
  }

  /// Feed a live session event into the chat. Only the "result" event
  /// matters here: it is the authoritative reply text, and it resolves
  /// the turn send() is waiting for. "message" events are intermediate
  /// (the result carries the full text); "messageUpdate" deltas are
  /// provisional. Events for another session are ignored.
  handleEvent(event: SenEvent): void {
    if (event.type !== "result") {
      return;
    }
    if (this.sessionId === null || event.sessionId !== this.sessionId) {
      return;
    }
    const pending = this.pending;
    if (!pending) {
      return;
    }
    this.pending = null;
    pending.resolve({ text: event.text, isError: event.isError });
  }

  /// Queue one send (desktop or Telegram). Sends run strictly in
  /// order; the returned promise resolves when this send is fully
  /// handled (reply in the transcript, Telegram reply sent). It never
  /// rejects: a failure becomes a system entry instead.
  send(channel: ChatChannel, text: string, telegramChatId?: number): Promise<void> {
    const run = this.queue.then(() =>
      this.sendNow(channel, text, telegramChatId),
    );
    this.queue = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  private append(
    channel: ChatChannel,
    role: TranscriptRole,
    text: string,
    telegramChatId?: number,
  ): TranscriptEntry {
    const entry: TranscriptEntry = {
      id: this.nextId,
      channel,
      role,
      text,
      ts: new Date().toISOString(),
      telegramChatId,
    };
    this.nextId += 1;
    this.entries.push(entry);
    this.options.onTranscript?.(this.getTranscript());
    return entry;
  }

  private setBusy(busy: boolean): void {
    if (this.busy === busy) {
      return;
    }
    this.busy = busy;
    this.options.onBusy?.(busy);
  }

  private async sendNow(
    channel: ChatChannel,
    text: string,
    telegramChatId?: number,
  ): Promise<void> {
    const trimmed = text.trim();
    if (!trimmed) {
      return;
    }
    // The session ended (end() above): refuse the send here, at the
    // head of the queued run, so a send queued before the end never
    // reaches a steer. No new entry: end() already wrote "Phiên đã
    // kết thúc." to the transcript, and a duplicate per refused
    // send would spam it. No backend call, no daily log entry,
    // nothing to Telegram.
    if (this.ended) {
      return;
    }
    if (this.sessionId === null) {
      this.append(channel, "system", "Chưa có phiên làm việc.", telegramChatId);
      return;
    }
    const command = parseChatCommand(trimmed);
    if (command) {
      await this.executeCommand(channel, trimmed, command, telegramChatId);
      return;
    }
    this.append(channel, "user", trimmed, telegramChatId);
    await this.logBestEffort("user", channel, trimmed);
    // CI P2-9 (P0-5): a plain message matching the morning-brief
    // skill runs the local brief flow instead of a model turn — the
    // skill body says the brief is built with buildMorningBrief.
    // Any other match, or no matcher, falls through to the model.
    // The match is async: the matcher asks the Rust registry
    // command (Tauri follow-up 3), which scans the skills dir.
    const skill = await this.options.skillMatcher?.(trimmed);
    if (skill?.name === "morning-brief") {
      await this.showMorningBrief(channel, telegramChatId);
      return;
    }
    await this.runModelTurn(channel, trimmed, telegramChatId);
  }

  private async executeCommand(
    channel: ChatChannel,
    rawText: string,
    command: ChatCommand,
    telegramChatId?: number,
  ): Promise<void> {
    const contract = this.options.makeContract(channel, "memory command");
    let reply: string;
    try {
      reply = await this.runCommand(contract, command, channel);
    } catch (error) {
      reply =
        "Không làm được: " +
        (error instanceof Error ? error.message : String(error));
    }
    this.append(channel, "system", reply, telegramChatId);
    await this.logBestEffort("memory", channel, rawText);
    if (telegramChatId !== undefined) {
      await this.sendTelegramBestEffort(telegramChatId, reply);
    }
  }

  private async runCommand(
    contract: ActionContract,
    command: ChatCommand,
    channel: ChatChannel,
  ): Promise<string> {
    switch (command.kind) {
      case "morning-brief": {
        const built = await this.buildBrief();
        if ("error" in built) {
          return built.error;
        }
        this.options.onBrief?.(built.html);
        return built.text;
      }
      case "mcp-call": {
        return this.runMcpCall(command);
      }
      case "memory-add": {
        const result = await this.memory.add(
          contract,
          command.scope,
          command.text,
        );
        return command.scope === "user"
          ? `Đã ghi về bạn: "${command.text}" (mục ${result.index}).`
          : `Đã ghi nhớ: "${command.text}" (mục ${result.index}).`;
      }
      case "memory-forget": {
        const result =
          "number" in command.target
            ? await this.memory.forgetByNumber(
                contract,
                command.scope,
                command.target.number,
              )
            : await this.memory.forgetByText(
                contract,
                command.scope,
                command.target.text,
              );
        return "number" in command.target
          ? `Đã quên mục ${command.target.number}: "${result.removed}".`
          : `Đã quên: "${result.removed}".`;
      }
      case "memory-edit": {
        await this.memory.edit(
          contract,
          command.scope,
          command.index,
          command.text,
        );
        return `Đã sửa mục ${command.index} thành: "${command.text}".`;
      }
      case "soul-set": {
        await this.memory.setSoul(contract, command.text);
        return "Đã cập nhật tính cách.";
      }
      case "memory-list": {
        const snapshot = await this.memory.read(contract);
        return formatMemoryList(snapshot);
      }
      case "memory-log": {
        const snapshot = await this.memory.read(contract);
        if (snapshot.today.length === 0) {
          return "Hôm nay chưa có sự kiện nào.";
        }
        return "Nhật ký hôm nay:\n" + snapshot.today.join("\n");
      }
      case "model-show": {
        const router = this.options.router;
        if (!router) {
          return "Bộ định tuyến chưa được nối. Mặc định: đường miễn phí của CLI.";
        }
        const status = await router.getRouter(contract);
        return formatRouterList(status);
      }
      case "model-set": {
        const router = this.options.router;
        if (!router) {
          return "Bộ định tuyến chưa được nối nên chưa đổi được đường.";
        }
        const status = await router.getRouter(contract);
        const wanted = status.routes.find((r) => r.id === command.routeId);
        if (!wanted) {
          const ids = status.routes.map((r) => r.id).join(", ");
          return `Không có đường "${command.routeId}". Các đường: ${ids}.`;
        }
        const next = await router.setPreferredRoute(contract, command.routeId);
        this.options.onRouterStatus?.(formatRouterStatus(next));
        return (
          `Đã chọn đường ${describeRouterRoute(wanted)}. ` +
          "Phiên đang chạy giữ đường cũ; phiên mới dùng đường này."
        );
      }
      case "unknown":
        return COMMAND_USAGE;
    }
  }

  /// Build the morning brief from the injected source (CI P2-9,
  /// P0-4): one buildMorningBrief call, rendered as plain text for
  /// the transcript and as escaped HTML for the UI panel. Returns an
  /// error string instead of a brief when no source is injected, the
  /// source fails, or the data is invalid — never a made-up brief.
  private async buildBrief(): Promise<
    | { text: string; html: string }
    | { error: string }
  > {
    const source = this.options.briefSource;
    if (!source) {
      return {
        error:
          "Chưa nối nguồn email/lịch nên chưa lập được bản tin sáng. " +
          "Cần một nguồn email + lịch thật trước khi /brief chạy.",
      };
    }
    let data: { emails: EmailItem[]; events: CalendarEvent[] };
    try {
      data = await source();
    } catch (error) {
      return {
        error:
          "Không lấy được email/lịch: " +
          (error instanceof Error ? error.message : String(error)),
      };
    }
    try {
      const brief = buildMorningBrief({
        emails: data.emails,
        events: data.events,
        now: new Date().toISOString(),
      });
      return {
        text: renderMorningBriefText(brief),
        html: renderMorningBriefHtml(brief),
      };
    } catch (error) {
      return {
        error:
          "Không lập được bản tin sáng: " +
          (error instanceof Error ? error.message : String(error)),
      };
    }
  }

  /// Show the brief for a skill-routed plain message: same build as
  /// /brief, appended as a system entry (and sent back to Telegram
  /// when the message came from there) instead of a model turn.
  private async showMorningBrief(
    channel: ChatChannel,
    telegramChatId?: number,
  ): Promise<void> {
    const built = await this.buildBrief();
    if ("error" in built) {
      this.append(channel, "system", built.error, telegramChatId);
      if (telegramChatId !== undefined) {
        await this.sendTelegramBestEffort(telegramChatId, built.error);
      }
      return;
    }
    this.options.onBrief?.(built.html);
    this.append(channel, "system", built.text, telegramChatId);
    if (telegramChatId !== undefined) {
      await this.sendTelegramBestEffort(telegramChatId, built.text);
    }
  }

  /// Run one MCP tool call through the injected delegate (CI P2-9,
  /// P0-5). The delegate spawns the `zen` CLI with the server
  /// config; the session id is the chat session id, which the CLI
  /// requires to be a UUID (P0-1). A missing delegate, an unknown
  /// server, or a delegate failure is an honest system reply — the
  /// tool never pretends to have run.
  private async runMcpCall(
    command: Extract<ChatCommand, { kind: "mcp-call" }>,
  ): Promise<string> {
    const delegate = this.options.delegateToolCall;
    if (!delegate) {
      return (
        "Đường ủy quyền MCP chưa được nối trong app nên tool chưa " +
        "chạy: cần một đường delegate tới CLI core trước khi /mcp chạy."
      );
    }
    const config = this.options.mcpServers?.[command.server];
    if (!config) {
      return (
        `Chưa cấu hình MCP server "${command.server}". ` +
        "Các server đã cấu hình: " +
        Object.keys(this.options.mcpServers ?? {}).join(", ") +
        "."
      );
    }
    const sessionId = this.sessionId;
    if (!sessionId) {
      return "Chưa có phiên làm việc.";
    }
    try {
      const result = await delegate({
        serverName: command.server,
        serverConfig: config,
        tool: command.tool,
        arguments: command.args,
        sessionId,
      });
      return (
        `Kết quả từ ${command.server}/${command.tool}:\n` + result.text
      );
    } catch (error) {
      return (
        `Không gọi được tool ${command.server}/${command.tool}: ` +
        (error instanceof Error ? error.message : String(error))
      );
    }
  }

  private async runModelTurn(
    channel: ChatChannel,
    text: string,
    telegramChatId?: number,
  ): Promise<void> {
    const sessionId = this.sessionId as string;
    const contract = this.options.makeContract(channel, "chat turn");
    this.setBusy(true);
    try {
      const waiter = this.armResultWaiter(contract.timeoutMs + 10000);
      // The waiter may reject (timeout) while the steer call below is
      // still awaited; mark the rejection handled now so it never
      // surfaces as an unhandled rejection — the await below still
      // throws the same error when it runs.
      waiter.catch(() => undefined);
      // The steer/follow_up ack carries the route that actually ran
      // (routeId/cost, P0-3). Keep it: after the reply lands, the
      // status line must show that route, not the preferred one —
      // after a fallback the two differ (rev2 F1).
      let turnResult: BackendSessionResult | undefined;
      try {
        if (!this.started) {
          turnResult = await steerSession(contract, sessionId, text);
          this.started = true;
        } else {
          turnResult = await followUpSession(contract, sessionId, text);
        }
      } catch (error) {
        // The turn failed before a result: release the waiter so the
        // await below throws instead of hanging. If the result already
        // arrived (pending is null), this is a no-op and the reply
        // below still lands.
        this.disarmResultWaiter(error);
      }
      let result: { text: string; isError: boolean };
      try {
        result = await waiter;
      } catch (error) {
        const message =
          "Sen không trả lời được: " +
          (error instanceof Error ? error.message : String(error));
        this.append(channel, "system", message, telegramChatId);
        if (telegramChatId !== undefined) {
          await this.sendTelegramBestEffort(telegramChatId, message);
        }
        return;
      }
      if (result.isError) {
        // P1-1: an error result is not the model's answer. The result
        // record is the only event the frontend gets for the turn
        // (cli_spawn.rs returns Ok(TurnResult) even when the record
        // carries is_error=true, and session.rs does not retry), so
        // the branch must happen here. The error text must not land
        // in the transcript as an assistant entry — both the desktop
        // UI and the Telegram bridge render assistant entries as real
        // replies — and it must not go to Telegram as a reply either.
        // Report it as a system entry instead, like the failed-steer
        // and timeout paths above, so both channels see the failure
        // for what it is. The error text is not logged as an
        // assistant line in the daily log either.
        const message = "Sen gặp lỗi: " + result.text;
        this.append(channel, "system", message, telegramChatId);
        if (telegramChatId !== undefined) {
          await this.sendTelegramBestEffort(telegramChatId, message);
        }
        return;
      }
      this.append(channel, "assistant", result.text, telegramChatId);
      await this.logBestEffort("assistant", channel, result.text);
      if (telegramChatId !== undefined) {
        await this.sendTelegramBestEffort(telegramChatId, result.text);
      }
      // The reply landed: the turn ran on turnResult's route (after
      // a fallback, the next route in the chain). Show that route on
      // the status line; never throws, never disturbs the transcript.
      await this.reportRanRoute(turnResult);
    } finally {
      this.setBusy(false);
    }
  }

  /// Arm the waiter for the turn's Result event. Resolves when the
  /// event arrives; rejects when the contract timeout + 10s passes
  /// without one (the bridge caps a turn at 120s, so this is only a
  /// backstop, never the normal path).
  private armResultWaiter(timeoutMs: number): Promise<{
    text: string;
    isError: boolean;
  }> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending) {
          this.pending = null;
          reject(
            new Error("hết thời gian chờ kết quả (quá timeout của phiên)."),
          );
        }
      }, timeoutMs);
      this.pending = {
        resolve: (result) => {
          clearTimeout(timer);
          resolve(result);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      };
    });
  }

  /// Release the waiter with an error, if it is still armed. No-op
  /// when the Result event already resolved it.
  private disarmResultWaiter(error: unknown): void {
    const pending = this.pending;
    if (!pending) {
      return;
    }
    this.pending = null;
    pending.reject(error instanceof Error ? error : new Error(String(error)));
  }

  private async logBestEffort(
    kind: LogKind,
    channel: ChatChannel,
    text: string,
  ): Promise<void> {
    try {
      const contract = this.options.makeContract(channel, "memory log");
      await this.memory.log(contract, kind, channel, text);
    } catch (error) {
      // The daily log is a side record: a failed write must not fail
      // the turn or the command. It is reported on the console, never
      // swallowed silently.
      // eslint-disable-next-line no-console
      console.error("Sen memory log failed: " + String(error));
    }
  }

  private async sendTelegramBestEffort(
    chatId: number,
    text: string,
  ): Promise<void> {
    const send = this.options.sendTelegram;
    if (!send) {
      return;
    }
    try {
      await send(chatId, text);
    } catch (error) {
      // The turn already succeeded and the transcript already has the
      // reply; a failed delivery is reported in the transcript so the
      // desktop user sees it instead of wondering why Telegram is quiet.
      // eslint-disable-next-line no-console
      console.error("Sen Telegram send failed: " + String(error));
      this.append(
        "telegram",
        "system",
        "Không gửi được trả lời về Telegram: " +
          (error instanceof Error ? error.message : String(error)),
        chatId,
      );
    }
  }
}

function formatMemoryList(snapshot: MemoryReadResult): string {
  const lines: string[] = [];
  if (snapshot.memory.length === 0) {
    lines.push("Bộ nhớ: (trống)");
  } else {
    lines.push("Bộ nhớ:");
    snapshot.memory.forEach((item, i) => {
      lines.push(`${i + 1}. ${item}`);
    });
  }
  if (snapshot.user.length === 0) {
    lines.push("Về bạn: (trống)");
  } else {
    lines.push("Về bạn:");
    snapshot.user.forEach((item, i) => {
      lines.push(`${i + 1}. ${item}`);
    });
  }
  return lines.join("\n");
}
