// Frontend listener for CLI bridge events (Phase 2, tin 9530 quyết 4).
// The backend emits one Tauri event per stream-json record while a CLI
// turn runs (event name sen-session-event, see
// apps/desktop/src-tauri/src/commands/session.rs). This module is the
// only place the frontend subscribes to it.
//
// Event shape mirrors the Rust SenEvent enum (serde tag "type",
// camelCase): sessionStart, message, messageUpdate, toolResult, system,
// result, unknown. An "unknown" event is a CLI record type this bridge
// does not know: it is delivered like any other (the backend counted it
// and never crashed on it), and the UI must render it as an unknown
// record, never as a failure.
//
// The transport is injectable ONLY for tests (setEventListen): the app
// path always uses the real Tauri listen. A test listener stands in for
// the Rust backend, which cannot run under node.
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";

/// Tauri event carrying one Sen event (must match SEN_SESSION_EVENT in
/// commands/session.rs).
export const SEN_SESSION_EVENT = "sen-session-event";

export interface SenTurnUsage {
  inputTokens: number;
  outputTokens: number;
  cacheReadInputTokens: number;
}

export type SenEvent =
  | { type: "sessionStart"; sessionId: string; model: string }
  | {
      type: "message";
      sessionId: string;
      text: string;
      toolCalls?: unknown;
    }
  | { type: "messageUpdate"; sessionId: string; delta: string }
  | {
      type: "toolResult";
      sessionId: string;
      toolUseId: string;
      content: unknown;
      isError: boolean;
    }
  | { type: "system"; sessionId: string; subtype: string }
  | {
      type: "result";
      sessionId: string;
      subtype: string;
      isError: boolean;
      text: string;
      numTurns: number;
      usage: SenTurnUsage;
      durationMs: number;
      model: string;
    }
  | {
      type: "unknown";
      sessionId: string;
      recordType: string;
      subtype?: string;
    };

export type EventListen = <T>(
  event: string,
  handler: (e: Event<T>) => void,
) => Promise<UnlistenFn>;

const realListen: EventListen = (event, handler) => listen(event, handler);

let eventListen: EventListen = realListen;

// Test-only injection point. Pass null to restore the real Tauri listen.
// Production code never calls this; tests use it to stand in for the
// Rust backend, which cannot run under node.
export function setEventListen(next: EventListen | null): void {
  eventListen = next ?? realListen;
}

function isSenEvent(value: unknown): value is SenEvent {
  const ev = value as SenEvent | null;
  return (
    !!ev &&
    typeof ev.type === "string" &&
    typeof (ev as { sessionId?: unknown }).sessionId === "string"
  );
}

/// Listen for events of one session only. Events for other sessions are
/// dropped here, never delivered. Returns the unlisten function; the
/// caller owns unsubscribing (for example when the session closes).
export async function listenSessionEvents(
  sessionId: string,
  onEvent: (event: SenEvent) => void,
): Promise<UnlistenFn> {
  return eventListen<SenEvent>(SEN_SESSION_EVENT, (e) => {
    const payload = e.payload;
    if (!isSenEvent(payload)) {
      return;
    }
    if (payload.sessionId !== sessionId) {
      return;
    }
    onEvent(payload);
  });
}
