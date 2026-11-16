// Frontend client for file memory Phase 1 (P0-1). The memory holds
// durable facts (MEMORY.md), facts about the user (USER.md), the
// persona text (SOUL.md) and the daily event log, all under the app
// data dir. No key, no token, no credential ever passes through here:
// the memory files hold user facts only.
//
// Wire shape (same lesson as settings-client R2B-05 rev2): the Rust
// handlers declare `args: <Args>`, so the command's own parameters
// travel nested under `args` — a flat payload is rejected with
// "missing required key args" before any handler logic runs.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";

export interface MemoryReadResult {
  memory: string[];
  user: string[];
  soul: string;
  today: string[];
}

export type MemoryScope = "memory" | "user";

export interface MemoryItemResult {
  scope: string;
  index: number;
  item: string;
}

export interface MemoryForgetResult {
  scope: string;
  removed: string;
}

export interface SoulResult {
  soul: string;
}

export interface LogMemoryResult {
  logged: boolean;
}

export type LogKind = "user" | "assistant" | "memory";
export type LogChannel = "desktop" | "telegram" | "local";

function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) && value.every((item) => typeof item === "string")
  );
}

function asMemoryReadResult(value: unknown): MemoryReadResult {
  const res = value as MemoryReadResult | null;
  if (
    !res ||
    !isStringArray(res.memory) ||
    !isStringArray(res.user) ||
    typeof res.soul !== "string" ||
    !isStringArray(res.today)
  ) {
    throw new Error(
      "Backend memory read malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asMemoryItemResult(value: unknown): MemoryItemResult {
  const res = value as MemoryItemResult | null;
  if (
    !res ||
    typeof res.scope !== "string" ||
    typeof res.index !== "number" ||
    typeof res.item !== "string"
  ) {
    throw new Error(
      "Backend memory item malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asMemoryForgetResult(value: unknown): MemoryForgetResult {
  const res = value as MemoryForgetResult | null;
  if (
    !res ||
    typeof res.scope !== "string" ||
    typeof res.removed !== "string"
  ) {
    throw new Error(
      "Backend memory forget malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asSoulResult(value: unknown): SoulResult {
  const res = value as SoulResult | null;
  if (!res || typeof res.soul !== "string") {
    throw new Error(
      "Backend soul result malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asLogMemoryResult(value: unknown): LogMemoryResult {
  const res = value as LogMemoryResult | null;
  if (!res || typeof res.logged !== "boolean") {
    throw new Error(
      "Backend memory log malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// Read everything: both item lists, the persona text, today's log.
export async function readMemory(
  contract: ActionContract,
): Promise<MemoryReadResult> {
  const raw = await invoke("sen_memory_read", { contract });
  return asMemoryReadResult(raw);
}

/// Append one fact. Returns its 1-based item number.
export async function addMemory(
  contract: ActionContract,
  scope: MemoryScope,
  text: string,
): Promise<MemoryItemResult> {
  const raw = await invoke("sen_memory_add", {
    contract,
    args: { scope, text },
  });
  return asMemoryItemResult(raw);
}

/// Delete one item by its 1-based number.
export async function forgetMemoryByNumber(
  contract: ActionContract,
  scope: MemoryScope,
  number: number,
): Promise<MemoryForgetResult> {
  const raw = await invoke("sen_memory_forget", {
    contract,
    args: { scope, target: { number } },
  });
  return asMemoryForgetResult(raw);
}

/// Delete one item by text. The backend refuses unless exactly one
/// item matches, so a vague text never deletes the wrong fact.
export async function forgetMemoryByText(
  contract: ActionContract,
  scope: MemoryScope,
  text: string,
): Promise<MemoryForgetResult> {
  const raw = await invoke("sen_memory_forget", {
    contract,
    args: { scope, target: { text } },
  });
  return asMemoryForgetResult(raw);
}

/// Replace one item's text (1-based number).
export async function editMemory(
  contract: ActionContract,
  scope: MemoryScope,
  index: number,
  text: string,
): Promise<MemoryItemResult> {
  const raw = await invoke("sen_memory_edit", {
    contract,
    args: { scope, index, text },
  });
  return asMemoryItemResult(raw);
}

/// Replace the whole persona text.
export async function setSoul(
  contract: ActionContract,
  text: string,
): Promise<SoulResult> {
  const raw = await invoke("sen_memory_set_soul", {
    contract,
    args: { text },
  });
  return asSoulResult(raw);
}

/// Append one event to today's daily log. The backend stamps the local
/// time; the caller only says what happened and where.
export async function logMemory(
  contract: ActionContract,
  kind: LogKind,
  channel: LogChannel,
  text: string,
): Promise<LogMemoryResult> {
  const raw = await invoke("sen_memory_log", {
    contract,
    args: { kind, channel, text },
  });
  return asLogMemoryResult(raw);
}
