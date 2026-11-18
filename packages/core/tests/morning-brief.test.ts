// Morning brief tests (Sen P0-4).
// Run with: node --test packages/core/tests/morning-brief.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// Fixtures are explicit test data in this file — no real mailbox or
// calendar is read. These tests prove the pure logic only: conflicts,
// deadlines, decision markers, deterministic sort, validation, and
// the text render. They do NOT prove a real Gmail/calendar fetch:
// there is no fetcher wired yet (see docs/morning-brief.md).
import test from "node:test";
import assert from "node:assert/strict";
import {
  buildMorningBrief,
  renderMorningBriefText,
  validateEmails,
  validateEvents,
  type CalendarEvent,
  type EmailItem,
} from "../src/morning-brief.ts";

const NOW = "2026-11-16T07:00:00.000Z";

function email(over: Partial<EmailItem> & { id: string }): EmailItem {
  return {
    from: "a@example.com",
    subject: "Subject",
    body: "Body.",
    receivedAt: "2026-11-16T06:00:00.000Z",
    ...over,
  };
}

function event(over: Partial<CalendarEvent> & { id: string }): CalendarEvent {
  return {
    title: "Event",
    start: "2026-11-16T09:00:00.000Z",
    end: "2026-11-16T10:00:00.000Z",
    ...over,
  };
}

// --- Conflicts ---

test("brief: overlapping events conflict with the overlap in minutes", () => {
  const brief = buildMorningBrief({
    emails: [],
    events: [
      event({ id: "e1", title: "Họp đội", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
      event({ id: "e2", title: "Review PR", start: "2026-11-16T09:30:00.000Z", end: "2026-11-16T10:30:00.000Z" }),
    ],
    now: NOW,
  });
  assert.equal(brief.conflicts.length, 1);
  assert.equal(brief.conflicts[0].firstId, "e1");
  assert.equal(brief.conflicts[0].secondId, "e2");
  assert.equal(brief.conflicts[0].overlapMinutes, 30);
  assert.equal(brief.conflicts[0].overlapStart, "2026-11-16T09:30:00.000Z");
  assert.equal(brief.conflicts[0].overlapEnd, "2026-11-16T10:00:00.000Z");
});

test("brief: events that only touch are NOT a conflict", () => {
  const brief = buildMorningBrief({
    emails: [],
    events: [
      event({ id: "e1", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
      event({ id: "e2", start: "2026-11-16T10:00:00.000Z", end: "2026-11-16T11:00:00.000Z" }),
    ],
    now: NOW,
  });
  assert.deepEqual(brief.conflicts, []);
});

test("brief: three overlapping events give three conflict pairs", () => {
  const brief = buildMorningBrief({
    emails: [],
    events: [
      event({ id: "e1", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
      event({ id: "e2", start: "2026-11-16T09:30:00.000Z", end: "2026-11-16T10:30:00.000Z" }),
      event({ id: "e3", start: "2026-11-16T09:45:00.000Z", end: "2026-11-16T10:15:00.000Z" }),
    ],
    now: NOW,
  });
  assert.equal(brief.conflicts.length, 3);
  assert.deepEqual(
    brief.conflicts.map((c) => [c.firstId, c.secondId, c.overlapMinutes]),
    [
      ["e1", "e2", 30],
      ["e1", "e3", 15],
      ["e2", "e3", 30],
    ],
  );
});

test("brief: a one-minute overlap still counts", () => {
  const brief = buildMorningBrief({
    emails: [],
    events: [
      event({ id: "e1", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
      event({ id: "e2", start: "2026-11-16T09:59:00.000Z", end: "2026-11-16T10:30:00.000Z" }),
    ],
    now: NOW,
  });
  assert.equal(brief.conflicts.length, 1);
  assert.equal(brief.conflicts[0].overlapMinutes, 1);
});

// --- Deadlines + today ---

test("brief: deadlines inside the window, out-of-window excluded", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Hợp đồng", dueAt: "2026-11-18T12:00:00.000Z" }),
      email({ id: "m2", subject: "Muộn", dueAt: "2026-12-01T12:00:00.000Z" }),
    ],
    events: [
      event({ id: "e1", title: "Ngày mai", start: "2026-11-17T09:00:00.000Z", end: "2026-11-17T10:00:00.000Z" }),
      event({ id: "e2", title: "Cuối tháng", start: "2026-11-30T09:00:00.000Z", end: "2026-11-30T10:00:00.000Z" }),
    ],
    now: NOW,
  });
  assert.deepEqual(
    brief.deadlines.map((d) => [d.kind, d.id, d.at]),
    [
      ["event", "e1", "2026-11-17T09:00:00.000Z"],
      ["email", "m1", "2026-11-18T12:00:00.000Z"],
    ],
  );
});

test("brief: a deadline exactly at the window end is inside", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", dueAt: "2026-11-23T07:00:00.000Z" }),
    ],
    events: [],
    now: NOW,
    windowDays: 7,
  });
  assert.equal(brief.deadlines.length, 1);
});

test("brief: todayEvents are only the events starting today", () => {
  const brief = buildMorningBrief({
    emails: [],
    events: [
      event({ id: "e1", title: "Hôm nay", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
      event({ id: "e2", title: "Ngày mai", start: "2026-11-17T09:00:00.000Z", end: "2026-11-17T10:00:00.000Z" }),
      event({ id: "e3", title: "Tối nay", start: "2026-11-16T20:00:00.000Z", end: "2026-11-16T21:00:00.000Z" }),
    ],
    now: NOW,
  });
  assert.deepEqual(brief.todayEvents.map((e) => e.id), ["e1", "e3"]);
});

// --- needsDecision ---

test("brief: needsDecision from the flag, a Vietnamese marker, and an English marker", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Nhắc việc", needsDecision: true }),
      email({ id: "m2", subject: "Vui lòng phê duyệt ngân sách", body: "Xem file đính kèm." }),
      email({ id: "m3", subject: "Launch date", body: "Please confirm the launch date." }),
      email({ id: "m4", subject: "Báo cáo tuần", body: "FYI, không có gì cần làm." }),
    ],
    events: [],
    now: NOW,
  });
  assert.deepEqual(brief.needsDecision.map((e) => e.id), ["m1", "m2", "m3"]);
});

test("brief: a plain informational email is not a decision", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Bản tin tuần", body: "Tin tức trong tuần." }),
    ],
    events: [],
    now: NOW,
  });
  assert.deepEqual(brief.needsDecision, []);
});

// --- Deterministic sort + counts ---

test("brief: output is deterministic and sorted by time then id", () => {
  const input = {
    emails: [
      email({ id: "m-b", subject: "B", receivedAt: "2026-11-16T06:00:00.000Z", dueAt: "2026-11-18T12:00:00.000Z", needsDecision: true }),
      email({ id: "m-a", subject: "A", receivedAt: "2026-11-16T06:00:00.000Z", dueAt: "2026-11-18T12:00:00.000Z", needsDecision: true }),
    ],
    events: [
      event({ id: "e2", title: "Sau", start: "2026-11-16T10:00:00.000Z", end: "2026-11-16T11:00:00.000Z" }),
      event({ id: "e1", title: "Trước", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
    ],
    now: NOW,
  };
  const first = buildMorningBrief(input);
  const second = buildMorningBrief(input);
  assert.deepEqual(first, second);
  // Same receivedAt: needsDecision falls back to id order.
  assert.deepEqual(first.needsDecision.map((e) => e.id), ["m-a", "m-b"]);
  // Same dueAt/start: deadlines fall back to id order.
  assert.deepEqual(first.deadlines.map((d) => d.id), ["e1", "e2", "m-a", "m-b"]);
  assert.equal(first.generatedAt, "2026-11-16T07:00:00.000Z");
  assert.equal(first.windowDays, 7);
  assert.equal(first.unreadCount, 2);
});

// --- Validation ---

test("validate: bad dates, empty ids, duplicates, start after end", () => {
  assert.ok(validateEmails("nope").some((e) => e.includes("must be an array")));
  assert.ok(validateEmails([email({ id: "" })]).some((e) => e.includes(".id must be a non-empty string")));
  assert.ok(
    validateEmails([email({ id: "m1" }), email({ id: "m1" })]).some((e) => e.includes("duplicates email id")),
  );
  assert.ok(
    validateEmails([email({ id: "m1", receivedAt: "not-a-date" })]).some((e) => e.includes("receivedAt")),
  );
  assert.ok(
    validateEmails([email({ id: "m1", dueAt: "not-a-date" })]).some((e) => e.includes("dueAt")),
  );
  assert.ok(
    validateEmails([email({ id: "m1", needsDecision: "yes" as unknown as boolean })]).some((e) => e.includes("needsDecision")),
  );
  assert.deepEqual(validateEmails([email({ id: "m1" })]), []);

  assert.ok(validateEvents("nope").some((e) => e.includes("must be an array")));
  assert.ok(
    validateEvents([event({ id: "e1", start: "2026-11-16T10:00:00.000Z", end: "2026-11-16T09:00:00.000Z" })]).some(
      (e) => e.includes("start must be before end"),
    ),
  );
  assert.ok(
    validateEvents([event({ id: "e1", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T09:00:00.000Z" })]).some(
      (e) => e.includes("start must be before end"),
    ),
  );
  assert.ok(
    validateEvents([event({ id: "e1" }), event({ id: "e1" })]).some((e) => e.includes("duplicates event id")),
  );
  assert.deepEqual(validateEvents([event({ id: "e1" })]), []);
});

test("buildMorningBrief: invalid input throws instead of building a wrong brief", () => {
  assert.throws(
    () =>
      buildMorningBrief({
        emails: [email({ id: "m1", receivedAt: "not-a-date" })],
        events: [],
        now: NOW,
      }),
    /Invalid morning brief input/,
  );
  assert.throws(
    () => buildMorningBrief({ emails: [], events: [], now: "not-a-date" }),
    /now must be a valid ISO/,
  );
  assert.throws(
    () => buildMorningBrief({ emails: [], events: [], now: NOW, windowDays: -1 }),
    /windowDays/,
  );
});

// --- P2-3: dates must be real ISO, compared by UTC day ---

test("brief P2-3: non-ISO now is rejected instead of silently emptying todayEvents", () => {
  assert.throws(
    () =>
      buildMorningBrief({
        emails: [],
        events: [event({ id: "e1" })],
        now: "17 Nov 2026 09:00",
      }),
    /now must be a valid ISO/,
  );
});

test("brief P2-3: an event with a timezone offset counts as today by its UTC day", () => {
  const brief = buildMorningBrief({
    emails: [],
    events: [
      // 01:00+07:00 on the 17th is 18:00Z on the 16th: "today" for a
      // 19:00Z now on the 16th, even though the written date says 17th.
      event({ id: "e1", title: "Tối 16", start: "2026-11-17T01:00:00+07:00", end: "2026-11-17T02:00:00+07:00" }),
      // Really tomorrow in UTC: must stay out of todayEvents.
      event({ id: "e2", title: "Sáng 17", start: "2026-11-17T01:00:00.000Z", end: "2026-11-17T02:00:00.000Z" }),
    ],
    now: "2026-11-16T19:00:00.000Z",
  });
  assert.deepEqual(brief.todayEvents.map((e) => e.id), ["e1"]);
});

test("validate P2-3: a non-ISO date string is not a valid ISO date", () => {
  assert.ok(
    validateEvents([
      event({ id: "e1", start: "17 Nov 2026 09:00", end: "2026-11-17T10:00:00.000Z" }),
    ]).some((e) => e.includes("start must be a valid ISO")),
  );
  assert.ok(
    validateEmails([email({ id: "m1", receivedAt: "17 Nov 2026 09:00" })]).some((e) =>
      e.includes("receivedAt must be a valid ISO"),
    ),
  );
  assert.ok(
    validateEvents([event({ id: "e1", start: "2026-02-30T09:00:00.000Z", end: "2026-03-01T10:00:00.000Z" })]).some(
      (e) => e.includes("start must be a valid ISO"),
    ),
  );
});

// --- P2-4: explicit false wins, markers match standalone words only ---

test("brief P2-4: an explicit needsDecision false is respected even with a marker in the body", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Quyết toán tháng 11", body: "Quyết toán chi phí tháng 11.", needsDecision: false }),
    ],
    events: [],
    now: NOW,
  });
  assert.deepEqual(brief.needsDecision, []);
});

test("brief P2-4: a marker inside a compound word or boilerplate does not count", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Báo cáo", body: "Chốt sổ cuối tháng, gửi báo cáo." }),
      email({ id: "m2", subject: "Invoice", body: "Please confirm receipt of the invoice." }),
      email({ id: "m3", subject: "Reconfirmation", body: "Your booking is reconfirmed." }),
    ],
    events: [],
    now: NOW,
  });
  assert.deepEqual(brief.needsDecision, []);
});

test("brief P2-4: a standalone marker still counts", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Cần bạn quyết ngân sách", body: "Xem file đính kèm." }),
      email({ id: "m2", subject: "Chốt phương án triển khai", body: "Chốt trước thứ sáu." }),
      email({ id: "m3", subject: "Quyết toán tháng 11", body: "Quyết toán chi phí.", needsDecision: true }),
    ],
    events: [],
    now: NOW,
  });
  assert.deepEqual(brief.needsDecision.map((e) => e.id), ["m1", "m2", "m3"]);
});

// --- Render ---

test("render: text names the real items, with plain words for empty sections", () => {
  const brief = buildMorningBrief({
    emails: [
      email({ id: "m1", subject: "Vui lòng phê duyệt ngân sách", from: "sep@example.com", dueAt: "2026-11-18T12:00:00.000Z" }),
    ],
    events: [
      event({ id: "e1", title: "Họp đội", start: "2026-11-16T09:00:00.000Z", end: "2026-11-16T10:00:00.000Z" }),
      event({ id: "e2", title: "Review PR", start: "2026-11-16T09:30:00.000Z", end: "2026-11-16T10:30:00.000Z" }),
    ],
    now: NOW,
  });
  const text = renderMorningBriefText(brief);
  assert.match(text, /Bản tin sáng — 2026-11-16T07:00:00\.000Z/);
  assert.match(text, /Họp đội/);
  assert.match(text, /Review PR/);
  assert.match(text, /chồng lấn 30 phút/);
  assert.match(text, /Vui lòng phê duyệt ngân sách — từ sep@example\.com/);
  assert.match(text, /Email chưa đọc: 1/);
  assert.doesNotMatch(text, /Lorem|placeholder|TODO|XXX/);

  const empty = renderMorningBriefText(
    buildMorningBrief({ emails: [], events: [], now: NOW }),
  );
  assert.match(empty, /Không có sự kiện nào hôm nay/);
  assert.match(empty, /Không có xung đột lịch/);
  assert.match(empty, /Không có deadline trong cửa sổ/);
  assert.match(empty, /Không có email nào cần quyết định/);
});
