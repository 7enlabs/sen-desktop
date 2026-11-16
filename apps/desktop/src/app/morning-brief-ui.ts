// Morning brief UI (Sen P0-4): render a MorningBrief as an HTML
// string. Email subjects, senders, and calendar titles are untrusted
// input, so every value is escaped before it lands in the string —
// the result is never fed to innerHTML unescaped anywhere in this
// module. This module only renders; it is NOT wired into main.ts in
// P0-4 (wiring is a follow-up after review, see
// docs/morning-brief.md).
import type { MorningBrief } from "../../../packages/core/src/morning-brief.ts";

/** Escape the five characters that can break out of HTML text. */
export function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function section(title: string, items: string[]): string {
  const body = items.length > 0 ? items.map((item) => `<li>${item}</li>`).join("") : "";
  return `<section><h2>${escapeHtml(title)}</h2><ul>${body}</ul></section>`;
}

export function renderMorningBriefHtml(brief: MorningBrief): string {
  const today = brief.todayEvents.map(
    (event) =>
      `${escapeHtml(event.start)} – ${escapeHtml(event.end)}: ${escapeHtml(event.title)}`,
  );
  const conflicts = brief.conflicts.map(
    (conflict) =>
      `${escapeHtml(conflict.firstTitle)} và ${escapeHtml(conflict.secondTitle)} ` +
      `chồng lấn ${conflict.overlapMinutes} phút ` +
      `(${escapeHtml(conflict.overlapStart)} – ${escapeHtml(conflict.overlapEnd)}).`,
  );
  const deadlines = brief.deadlines.map(
    (deadline) => `${escapeHtml(deadline.at)}: ${escapeHtml(deadline.label)} (${escapeHtml(deadline.kind)})`,
  );
  const decisions = brief.needsDecision.map(
    (email) => `${escapeHtml(email.subject)} — từ ${escapeHtml(email.from)}`,
  );
  const parts = [
    `<article class="morning-brief">`,
    `<h1>Bản tin sáng</h1>`,
    `<p>${escapeHtml(brief.generatedAt)} · Cửa sổ: ${brief.windowDays} ngày · Email chưa đọc: ${brief.unreadCount}</p>`,
    section("Hôm nay", today.length > 0 ? today : ["Không có sự kiện nào hôm nay."]),
    section("Xung đột lịch", conflicts.length > 0 ? conflicts : ["Không có xung đột lịch."]),
    section("Deadline", deadlines.length > 0 ? deadlines : ["Không có deadline trong cửa sổ."]),
    section("Cần quyết định", decisions.length > 0 ? decisions : ["Không có email nào cần quyết định."]),
    `</article>`,
  ];
  return parts.join("");
}
