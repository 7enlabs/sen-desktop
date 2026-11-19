// Frontend client for the Rust brief source (Tauri follow-up 2,
// ZC Leader tin 17093): one call, through the bridge like every
// other client, to sen_brief_source. The backend fetches Gmail +
// Google Calendar with the SEN_GMAIL_ACCESS_TOKEN from its own
// environment and returns EmailItem / CalendarEvent data; this
// client only validates the wire shape (with the same validators
// buildMorningBrief uses) and hands it to the chat layer. A
// backend failure surfaces here untouched — never mapped to a
// fake brief. When the token is not configured the backend fails
// as BRIEF_SOURCE_NOT_CONFIGURED, and /brief keeps refusing
// honestly. No key, token, or credential travels through this
// call: the token never leaves the backend.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";
import {
  validateEmails,
  validateEvents,
  type CalendarEvent,
  type EmailItem,
} from "../../../../packages/core/src/morning-brief.ts";

export interface BriefSourceData {
  emails: EmailItem[];
  events: CalendarEvent[];
}

function asBriefSourceData(value: unknown): BriefSourceData {
  const res = value as BriefSourceData | null;
  if (!res || typeof res !== "object" || Array.isArray(res)) {
    throw new Error(
      "Backend brief source malformed, FAIL: " + JSON.stringify(value),
    );
  }
  const emailErrors = validateEmails(res.emails);
  const eventErrors = validateEvents(res.events);
  const errors = [...emailErrors, ...eventErrors];
  if (errors.length > 0) {
    throw new Error(
      "Backend brief source malformed, FAIL: " + errors.join("; "),
    );
  }
  return { emails: res.emails, events: res.events };
}

/** Fetch the morning-brief source data from the Rust command. */
export async function fetchBriefSource(
  contract: ActionContract,
): Promise<BriefSourceData> {
  const raw = await invoke("sen_brief_source", { contract });
  return asBriefSourceData(raw);
}
