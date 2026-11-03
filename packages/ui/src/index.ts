// STUB - Renderer cho tool call va result, themes cap qua ResourceLoader, theo prep muc 1.
// CHUA LAP DAY: khong co renderer that, khong co theme that, khong co noi dung gia.
export interface ToolRenderInput {
  toolName: string;
  result: unknown;
}

// CHUA CO: ham render that.
export function renderToolCall(_input: ToolRenderInput): string {
  throw new Error("Renderer CHUA LAP DAY.");
}
