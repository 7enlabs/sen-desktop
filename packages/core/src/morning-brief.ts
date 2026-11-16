// Morning brief (Sen P0-4): read-only synthesis of email + calendar
// into a morning brief (schedule conflicts, deadlines, decisions
// needed). Tin 13971 (ZC Leader, 16/11): every morning Sen sums up
// email + calendar into a brief; the brief only reads, it never acts.
//
// This module is pure TS (no I/O, no network, no key, no model call).
// It takes email + calendar items as plain data — where those items
// come from (a Gmail fetch, a calendar fetch) is a later phase, not
// this one. Nothing here sends, drafts, archives, or schedules
// anything; turning a brief item into an action is the job of
// proactive-inbox.ts, behind the P0-2 approval gate.
//
// Dates are ISO strings and are compared in UTC (Date.parse). "Today"
// means the UTC calendar day of `now`. The window is [now,
// now + windowDays days], both ends inclusive.

export interface EmailItem {
  id: string;
  from: string;
  subject: string;
  body: string;
  receivedAt: string;
  needsDecision?: boolean;
  dueAt?: string;
}

export interface CalendarEvent {
  id: string;
  title: string;
  start: string;
  end: string;
  attendees?: string[];
  location?: string;
  description?: string;
}

export interface ScheduleConflict {
  firstId: string;
  secondId: string;
  firstTitle: string;
  secondTitle: string;
  overlapMinutes: number;
  overlapStart: string;
  overlapEnd: string;
}

export interface DeadlineItem {
  kind: "email" | "event";
  id: string;
  label: string;
  at: string;
}

export interface MorningBrief {
  generatedAt: string;
  windowDays: number;
  conflicts: ScheduleConflict[];
  deadlines: DeadlineItem[];
  needsDecision: EmailItem[];
  todayEvents: CalendarEvent[];
  unreadCount: number;
}

function isValidIso(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && !Number.isNaN(Date.parse(value));
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

/**
 * Validate email items, in the same style as validateToolPolicy /
 * validateRouterConfig: returns the error list, empty when valid.
 * Does not throw, so a caller can report every problem at once.
 */
export function validateEmails(emails: unknown): string[] {
  if (!Array.isArray(emails)) {
    return ["emails must be an array"];
  }
  const errors: string[] = [];
  const seen = new Set<string>();
  emails.forEach((item, index) => {
    const where = `emails[${index}]`;
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      errors.push(`${where} must be an object`);
      return;
    }
    const raw = item as Record<string, unknown>;
    if (!isNonEmptyString(raw.id)) {
      errors.push(`${where}.id must be a non-empty string`);
    } else if (seen.has(raw.id)) {
      errors.push(`${where}.id duplicates email id: ${raw.id}`);
    } else {
      seen.add(raw.id);
    }
    if (!isNonEmptyString(raw.from)) {
      errors.push(`${where}.from must be a non-empty string`);
    }
    if (typeof raw.subject !== "string") {
      errors.push(`${where}.subject must be a string`);
    }
    if (typeof raw.body !== "string") {
      errors.push(`${where}.body must be a string`);
    }
    if (!isValidIso(raw.receivedAt)) {
      errors.push(`${where}.receivedAt must be a valid ISO date string`);
    }
    if (raw.needsDecision !== undefined && typeof raw.needsDecision !== "boolean") {
      errors.push(`${where}.needsDecision must be a boolean when present`);
    }
    if (raw.dueAt !== undefined && !isValidIso(raw.dueAt)) {
      errors.push(`${where}.dueAt must be a valid ISO date string when present`);
    }
  });
  return errors;
}

/** Validate calendar events, same shape as validateEmails. */
export function validateEvents(events: unknown): string[] {
  if (!Array.isArray(events)) {
    return ["events must be an array"];
  }
  const errors: string[] = [];
  const seen = new Set<string>();
  events.forEach((item, index) => {
    const where = `events[${index}]`;
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      errors.push(`${where} must be an object`);
      return;
    }
    const raw = item as Record<string, unknown>;
    if (!isNonEmptyString(raw.id)) {
      errors.push(`${where}.id must be a non-empty string`);
    } else if (seen.has(raw.id)) {
      errors.push(`${where}.id duplicates event id: ${raw.id}`);
    } else {
      seen.add(raw.id);
    }
    if (!isNonEmptyString(raw.title)) {
      errors.push(`${where}.title must be a non-empty string`);
    }
    if (!isValidIso(raw.start)) {
      errors.push(`${where}.start must be a valid ISO date string`);
    }
    if (!isValidIso(raw.end)) {
      errors.push(`${where}.end must be a valid ISO date string`);
    }
    if (isValidIso(raw.start) && isValidIso(raw.end) && Date.parse(raw.start) >= Date.parse(raw.end)) {
      errors.push(`${where}.start must be before end`);
    }
    if (raw.attendees !== undefined) {
      if (!Array.isArray(raw.attendees) || raw.attendees.some((a) => typeof a !== "string")) {
        errors.push(`${where}.attendees must be an array of strings when present`);
      }
    }
    if (raw.location !== undefined && typeof raw.location !== "string") {
      errors.push(`${where}.location must be a string when present`);
    }
    if (raw.description !== undefined && typeof raw.description !== "string") {
      errors.push(`${where}.description must be a string when present`);
    }
  });
  return errors;
}

// Markers that make an email count as needing a decision, matched
// case-insensitively as substrings of subject + body. The list is
// explicit and closed: nothing outside it counts, so a plain
// informational email ("weekly report", "FYI") never becomes a
// decision item by accident. Vietnamese first, then English.
export const DECISION_MARKERS: readonly string[] = [
  // Vietnamese
  "duyệt",
  "phê duyệt",
  "quyết định",
  "quyết",
  "xác nhận",
  "xin ý kiến",
  "cho ý kiến",
  "đồng ý",
  "chốt",
  "cần bạn quyết",
  // English
  "approve",
  "approval",
  "confirm",
  "decision",
  "decide",
  "sign off",
  "action required",
  "please review",
];

export function emailNeedsDecision(email: EmailItem): boolean {
  if (email.needsDecision === true) {
    return true;
  }
  const text = `${email.subject} ${email.body}`.toLowerCase();
  return DECISION_MARKERS.some((marker) => text.includes(marker.toLowerCase()));
}

function byTimeThenId(aTime: string, bTime: string, aId: string, bId: string): number {
  const diff = Date.parse(aTime) - Date.parse(bTime);
  if (diff !== 0) {
    return diff;
  }
  return aId < bId ? -1 : aId > bId ? 1 : 0;
}

export interface BuildMorningBriefInput {
  emails: EmailItem[];
  events: CalendarEvent[];
  now: string;
  windowDays?: number;
}

/**
 * Build the brief. Throws (FAIL-loud) on invalid input — bad dates
 * must never silently become a wrong brief. Output is deterministic:
 * every list is sorted by time, then id, so the same input always
 * renders the same brief.
 *
 * - Conflicts: pairs of events that really overlap (overlap > 0
 *   minutes). Events that only touch — one ends exactly when the
 *   next starts — are NOT a conflict.
 * - Deadlines: an email dueAt, or an event start, inside the window
 *   [now, now + windowDays days], sorted by time then id.
 * - needsDecision: emails flagged, or matching DECISION_MARKERS.
 * - todayEvents: events starting on the UTC calendar day of `now`.
 * - unreadCount: the number of emails handed in. The input list is
 *   the unread inbox snapshot, so there is no separate read flag to
 *   count from; the count is exactly the list length, nothing more.
 */
export function buildMorningBrief(input: BuildMorningBriefInput): MorningBrief {
  const emailErrors = validateEmails(input.emails);
  const eventErrors = validateEvents(input.events);
  const errors = [...emailErrors, ...eventErrors];
  if (!isValidIso(input.now)) {
    errors.push("now must be a valid ISO date string");
  }
  const windowDays = input.windowDays ?? 7;
  if (typeof windowDays !== "number" || !Number.isFinite(windowDays) || windowDays < 0) {
    errors.push("windowDays must be a non-negative finite number when present");
  }
  if (errors.length > 0) {
    throw new Error(`Invalid morning brief input: ${errors.join("; ")}`);
  }
  const nowMs = Date.parse(input.now);
  const windowEndMs = nowMs + windowDays * 24 * 60 * 60 * 1000;
  const today = input.now.slice(0, 10);

  const sortedEvents = [...input.events].sort((a, b) =>
    byTimeThenId(a.start, b.start, a.id, b.id),
  );

  const conflicts: ScheduleConflict[] = [];
  for (let i = 0; i < sortedEvents.length; i += 1) {
    for (let j = i + 1; j < sortedEvents.length; j += 1) {
      const first = sortedEvents[i];
      const second = sortedEvents[j];
      const overlapStartMs = Math.max(Date.parse(first.start), Date.parse(second.start));
      const overlapEndMs = Math.min(Date.parse(first.end), Date.parse(second.end));
      const overlapMinutes = (overlapEndMs - overlapStartMs) / 60000;
      if (overlapMinutes > 0) {
        conflicts.push({
          firstId: first.id,
          secondId: second.id,
          firstTitle: first.title,
          secondTitle: second.title,
          overlapMinutes,
          overlapStart: new Date(overlapStartMs).toISOString(),
          overlapEnd: new Date(overlapEndMs).toISOString(),
        });
      }
    }
  }
  conflicts.sort((a, b) => byTimeThenId(a.overlapStart, b.overlapStart, a.firstId + a.secondId, b.firstId + b.secondId));

  const deadlines: DeadlineItem[] = [];
  for (const email of input.emails) {
    if (email.dueAt !== undefined) {
      const atMs = Date.parse(email.dueAt);
      if (atMs >= nowMs && atMs <= windowEndMs) {
        deadlines.push({ kind: "email", id: email.id, label: email.subject, at: email.dueAt });
      }
    }
  }
  for (const event of input.events) {
    const atMs = Date.parse(event.start);
    if (atMs >= nowMs && atMs <= windowEndMs) {
      deadlines.push({ kind: "event", id: event.id, label: event.title, at: event.start });
    }
  }
  deadlines.sort((a, b) => byTimeThenId(a.at, b.at, a.id, b.id));

  const needsDecision = input.emails
    .filter((email) => emailNeedsDecision(email))
    .sort((a, b) => byTimeThenId(a.receivedAt, b.receivedAt, a.id, b.id));

  const todayEvents = sortedEvents.filter((event) => event.start.slice(0, 10) === today);

  return {
    generatedAt: new Date(nowMs).toISOString(),
    windowDays,
    conflicts,
    deadlines,
    needsDecision,
    todayEvents,
    unreadCount: input.emails.length,
  };
}

/** Render the brief as plain text. No placeholder, no fake content:
// empty sections say so in plain words ("Không có ..."). */
export function renderMorningBriefText(brief: MorningBrief): string {
  const lines: string[] = [];
  lines.push(`Bản tin sáng — ${brief.generatedAt}`);
  lines.push(`Cửa sổ: ${brief.windowDays} ngày. Email chưa đọc: ${brief.unreadCount}.`);
  lines.push("");
  lines.push("Hôm nay:");
  if (brief.todayEvents.length === 0) {
    lines.push("- Không có sự kiện nào hôm nay.");
  } else {
    for (const event of brief.todayEvents) {
      lines.push(`- ${event.start} – ${event.end}: ${event.title}`);
    }
  }
  lines.push("");
  lines.push("Xung đột lịch:");
  if (brief.conflicts.length === 0) {
    lines.push("- Không có xung đột lịch.");
  } else {
    for (const conflict of brief.conflicts) {
      lines.push(
        `- ${conflict.firstTitle} và ${conflict.secondTitle} chồng lấn ` +
          `${conflict.overlapMinutes} phút (${conflict.overlapStart} – ${conflict.overlapEnd}).`,
      );
    }
  }
  lines.push("");
  lines.push("Deadline:");
  if (brief.deadlines.length === 0) {
    lines.push("- Không có deadline trong cửa sổ.");
  } else {
    for (const deadline of brief.deadlines) {
      lines.push(`- ${deadline.at}: ${deadline.label} (${deadline.kind})`);
    }
  }
  lines.push("");
  lines.push("Cần quyết định:");
  if (brief.needsDecision.length === 0) {
    lines.push("- Không có email nào cần quyết định.");
  } else {
    for (const email of brief.needsDecision) {
      lines.push(`- ${email.subject} — từ ${email.from}`);
    }
  }
  return lines.join("\n");
}
