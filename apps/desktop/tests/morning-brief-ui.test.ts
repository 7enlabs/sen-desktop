// Morning brief UI tests (Sen P0-4).
// Run with: node --test apps/desktop/tests/morning-brief-ui.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// These tests prove the render escapes untrusted email/calendar text
// and names the real brief items. They do NOT prove the brief is
// shown in the app: the module is not wired into main.ts in P0-4
// (wiring is a follow-up after review, see docs/morning-brief.md).
import test from "node:test";
import assert from "node:assert/strict";
import { buildMorningBrief } from "../../../packages/core/src/morning-brief.ts";
import { escapeHtml, renderMorningBriefHtml } from "../src/app/morning-brief-ui.ts";

const NOW = "2026-11-16T07:00:00.000Z";

test("ui: escapeHtml escapes all five breakout characters", () => {
  assert.equal(escapeHtml(`<a href="x">&'y</a>`), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;y&lt;/a&gt;");
  assert.equal(escapeHtml("plain text"), "plain text");
});

test("ui: render names the real items from the brief", () => {
  const brief = buildMorningBrief({
    emails: [
      {
        id: "m1",
        from: "sep@example.com",
        subject: "Vui lòng phê duyệt ngân sách",
        body: "Xem file đính kèm.",
        receivedAt: "2026-11-16T06:00:00.000Z",
        dueAt: "2026-11-18T12:00:00.000Z",
      },
    ],
    events: [
      {
        id: "e1",
        title: "Họp đội",
        start: "2026-11-16T09:00:00.000Z",
        end: "2026-11-16T10:00:00.000Z",
      },
    ],
    now: NOW,
  });
  const html = renderMorningBriefHtml(brief);
  assert.match(html, /Bản tin sáng/);
  assert.match(html, /Họp đội/);
  assert.match(html, /Vui lòng phê duyệt ngân sách/);
  assert.match(html, /sep@example\.com/);
  assert.match(html, /Email chưa đọc: 1/);
});

test("ui: hostile email and calendar text is escaped, never raw markup", () => {
  const hostile = `<script>alert("x")</script><img src=x onerror=alert(1)>`;
  const brief = buildMorningBrief({
    emails: [
      {
        id: "m1",
        from: `evil@example.com"><b>`,
        subject: hostile,
        body: hostile,
        receivedAt: "2026-11-16T06:00:00.000Z",
        needsDecision: true,
      },
    ],
    events: [
      {
        id: "e1",
        title: hostile,
        start: "2026-11-16T09:00:00.000Z",
        end: "2026-11-16T10:00:00.000Z",
      },
    ],
    now: NOW,
  });
  const html = renderMorningBriefHtml(brief);
  assert.doesNotMatch(html, /<script>/);
  assert.doesNotMatch(html, /<img /);
  // "onerror=" only survives inside escaped text (&lt;...&gt;), never
  // as a real attribute: the "<" that would open the tag is escaped.
  assert.doesNotMatch(html, /<[^>]*onerror=/);
  assert.match(html, /&lt;script&gt;/);
  assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.match(html, /evil@example\.com&quot;&gt;&lt;b&gt;/);
});

test("ui: empty brief renders plain empty-state words, no fake content", () => {
  const brief = buildMorningBrief({ emails: [], events: [], now: NOW });
  const html = renderMorningBriefHtml(brief);
  assert.match(html, /Không có sự kiện nào hôm nay/);
  assert.match(html, /Không có xung đột lịch/);
  assert.match(html, /Không có deadline trong cửa sổ/);
  assert.match(html, /Không có email nào cần quyết định/);
  assert.doesNotMatch(html, /Lorem|placeholder|TODO/);
});
