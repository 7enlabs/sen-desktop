// STUB - Pi version lock lives here.
// The locked Pi version is 1.5.0, per tin 4212 (whole Pi stack follows newest).
// Default model: NONE, per decision message 4070 (B-1 anti-job rule: never
// name a real model/API).
// packages/providers is only a channel-declaration shell; no model attached,
// no key, no network call.
// No SDK import here: only packages/core may import the SDK, per prep
// section 3 point 4.
export const PINNED_PI_VERSION = "1.5.0";

export interface ModelRuntime {
  pinnedPiVersion: string;
}

// NOT HERE YET: real ModelRuntime. The function below only reports that it is
// not filled in; it never returns a fake runtime.
export function createModelRuntime(): ModelRuntime {
  throw new Error("ModelRuntime NOT FILLED IN YET: no default model per message 4070.");
}
