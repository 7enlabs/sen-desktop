// Telegram bridge, frontend half (P0-1). The Rust backend owns the
// Bot API client, the token and the poll offset; this module only
// drives poll rounds and sends replies through IPC.
//
// The poll loop is caller-driven: pollTelegramOnce does one getUpdates
// round (long-poll, up to 30s) and returns the new text updates; the
// loop calls it again after each round, with a 1s backoff after an
// error. There is no background thread in the backend, so stopping is
// just not calling again. Each update is routed into the shared chat
// session (chat.send with channel "telegram"), which queues it behind
// any turn already running and sends the reply back to the same chat.
//
// Wire shape (same lesson as settings-client R2B-05 rev2): the Rust
// handlers declare `args: <Args>`, so send parameters travel nested
// under `args`.
import { invoke } from "../ipc/bridge.ts";
import type { ActionContract } from "../ipc/types.ts";
import type { ChatSession } from "./chat.ts";

export interface TelegramUpdate {
  updateId: number;
  chatId: number;
  text: string;
  fromUsername: string | null;
}

export interface TelegramStatusResult {
  configured: boolean;
  allowedChatIdSet: boolean;
  allowAnyChat: boolean;
}

export interface TelegramPollResult {
  updates: TelegramUpdate[];
}

export interface TelegramSendResult {
  sent: boolean;
}

function isTelegramUpdate(value: unknown): value is TelegramUpdate {
  const update = value as TelegramUpdate | null;
  return (
    !!update &&
    typeof update.updateId === "number" &&
    typeof update.chatId === "number" &&
    typeof update.text === "string" &&
    (update.fromUsername === null ||
      typeof update.fromUsername === "string")
  );
}

function asTelegramStatusResult(value: unknown): TelegramStatusResult {
  const res = value as TelegramStatusResult | null;
  if (
    !res ||
    typeof res.configured !== "boolean" ||
    typeof res.allowedChatIdSet !== "boolean" ||
    typeof res.allowAnyChat !== "boolean"
  ) {
    throw new Error(
      "Backend telegram status malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asTelegramPollResult(value: unknown): TelegramPollResult {
  const res = value as TelegramPollResult | null;
  if (!res || !Array.isArray(res.updates) || !res.updates.every(isTelegramUpdate)) {
    throw new Error(
      "Backend telegram poll malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asTelegramSendResult(value: unknown): TelegramSendResult {
  const res = value as TelegramSendResult | null;
  if (!res || typeof res.sent !== "boolean") {
    throw new Error(
      "Backend telegram send malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// Whether a bot token is configured (and whether an allowlist chat id
/// is set, or the explicit any-chat opt-in is on). Never returns the
/// token.
export async function getTelegramStatus(
  contract: ActionContract,
): Promise<TelegramStatusResult> {
  const raw = await invoke("sen_telegram_status", { contract });
  return asTelegramStatusResult(raw);
}

/// One getUpdates round (long-poll, up to 30s). Returns the new
/// allowed text updates; the offset is kept in the backend, so the
/// caller just calls again. An error leaves the offset untouched.
export async function pollTelegramOnce(
  contract: ActionContract,
): Promise<TelegramPollResult> {
  const raw = await invoke("sen_telegram_poll_once", { contract });
  return asTelegramPollResult(raw);
}

/// Send one reply to a Telegram chat. Long text is split into
/// 4096-UTF-16-unit messages by the backend, never truncated.
export async function sendTelegramMessage(
  contract: ActionContract,
  chatId: number,
  text: string,
): Promise<TelegramSendResult> {
  const raw = await invoke("sen_telegram_send", {
    contract,
    args: { chatId, text },
  });
  return asTelegramSendResult(raw);
}

export interface TelegramLoopOptions {
  /// One poll round. Defaults to the real pollTelegramOnce; tests
  /// inject a fake.
  pollOnce?: (contract: ActionContract) => Promise<TelegramPollResult>;
  /// Sleep between rounds after an error. Defaults to setTimeout.
  sleep?: (ms: number) => Promise<void>;
}

/// Start the poll loop. Returns a stop function: stopping just ends
/// the loop after the current round (a long-poll round in flight
/// returns its updates first, and they are still routed). The loop
/// never throws: a failed round logs and retries after 1s, and a
/// failed update logs and moves on to the next one.
export function startTelegramLoop(
  chat: ChatSession,
  makeContract: (channel: "telegram", purpose: string) => ActionContract,
  options: TelegramLoopOptions = {},
): () => void {
  let stopped = false;
  const pollOnce = options.pollOnce ?? pollTelegramOnce;
  const sleep =
    options.sleep ??
    ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  void (async () => {
    while (!stopped) {
      try {
        const contract = makeContract("telegram", "telegram poll");
        const { updates } = await pollOnce(contract);
        for (const update of updates) {
          if (stopped) {
            break;
          }
          // chat.send never rejects in production (a failure becomes a
          // system entry), so this await only orders the turns. The
          // catch is belt-and-braces: one failed update must not fail
          // the round — the backend offset already advanced past it,
          // so a round failure here would lose the update silently.
          try {
            await chat.send("telegram", update.text, update.chatId);
          } catch (error) {
            // eslint-disable-next-line no-console
            console.error("Sen Telegram update failed: " + String(error));
          }
        }
      } catch (error) {
        // A failed round must not kill the loop: log and retry after
        // 1s. The backend offset is untouched on error, so the next
        // round re-fetches the same updates instead of losing them.
        // eslint-disable-next-line no-console
        console.error("Sen Telegram poll failed: " + String(error));
        await sleep(1000);
      }
    }
  })();
  return () => {
    stopped = true;
  };
}
