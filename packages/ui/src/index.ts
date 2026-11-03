// STUB - Renderer for tool calls and results, themes served via
// ResourceLoader, per prep section 1.
// NOT FILLED IN YET: no real renderer, no real theme, no fake content.
export interface ToolRenderInput {
  toolName: string;
  result: unknown;
}

// NOT HERE YET: real render function.
export function renderToolCall(_input: ToolRenderInput): string {
  throw new Error("Renderer NOT FILLED IN YET.");
}
