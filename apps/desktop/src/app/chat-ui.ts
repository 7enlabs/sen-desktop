// Desktop chat UI (P0-1): vanilla DOM, no framework. Renders the
// shared transcript (desktop + Telegram entries), a typing indicator
// while a turn runs, and a status line (session + Telegram state).
// Entry text is set with textContent, never innerHTML, so a message
// can never inject markup. The message list starts empty: no fake
// content, no placeholder greeting.
import type { ChatSession, TranscriptEntry } from "../chat/chat.ts";

export interface ChatUI {
  render(entries: TranscriptEntry[]): void;
  setBusy(busy: boolean): void;
  setSession(sessionId: string): void;
  setTelegramStatus(text: string): void;
  // P0-3: the active model route + cost, e.g. "Model: spark · ... · 0 đồng".
  setRouterStatus(text: string): void;
  // CI P2-9: show a built morning brief in the panel below the
  // transcript. The HTML comes only from renderMorningBriefHtml,
  // which escapes every email/calendar value, so setting it as
  // innerHTML cannot inject markup — this is the one innerHTML in
  // the UI, and the wiring-p2-9 suite proves the escaping.
  showBrief(html: string): void;
}

export function startChatUI(root: HTMLElement, chat: ChatSession): ChatUI {
  const header = document.createElement("header");
  const title = document.createElement("h1");
  title.textContent = "Sen";
  const status = document.createElement("div");
  status.className = "status";
  const sessionStatus = document.createElement("span");
  const telegramStatus = document.createElement("span");
  const routerStatus = document.createElement("span");
  status.append(sessionStatus, telegramStatus, routerStatus);
  header.append(title, status);

  const messages = document.createElement("div");
  messages.className = "messages";

  // CI P2-9: the morning-brief panel, hidden until the first brief
  // is built (showBrief). Its content is set only from
  // renderMorningBriefHtml, which escapes every value.
  const briefPanel = document.createElement("section");
  briefPanel.className = "morning-brief";
  briefPanel.hidden = true;

  const typing = document.createElement("div");
  typing.className = "typing";
  typing.textContent = "Sen đang trả lời…";
  typing.hidden = true;

  const form = document.createElement("form");
  form.className = "composer";
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = "Nhắn cho Sen…";
  input.setAttribute("aria-label", "Nhắn cho Sen");
  const send = document.createElement("button");
  send.type = "submit";
  send.textContent = "Gửi";
  form.append(input, send);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = input.value;
    if (!text.trim()) {
      return;
    }
    input.value = "";
    void chat.send("desktop", text);
  });

  root.append(header, messages, briefPanel, typing, form);

  function render(entries: TranscriptEntry[]): void {
    while (messages.firstChild) {
      messages.removeChild(messages.firstChild);
    }
    for (const entry of entries) {
      const node = document.createElement("div");
      node.className = "msg " + entry.role;
      if (entry.channel === "telegram") {
        const tag = document.createElement("span");
        tag.className = "channel";
        tag.textContent = "Telegram";
        node.append(tag);
      }
      const body = document.createElement("span");
      body.className = "text";
      body.textContent = entry.text;
      node.append(body);
      messages.append(node);
    }
    messages.scrollTop = messages.scrollHeight;
  }

  return {
    render,
    setBusy(busy: boolean): void {
      typing.hidden = !busy;
    },
    setSession(sessionId: string): void {
      sessionStatus.textContent = "Phiên: " + sessionId.slice(0, 8) + "…";
    },
    setTelegramStatus(text: string): void {
      telegramStatus.textContent = text;
    },
    setRouterStatus(text: string): void {
      routerStatus.textContent = text;
    },
    showBrief(html: string): void {
      briefPanel.innerHTML = html;
      briefPanel.hidden = false;
    },
  };
}
