// Frontend client for CLI bridge settings (Phase 2, tin 9530 quyết 2).
// The settings hold the MODEL id only. There is no key function here:
// the key travels only through the process environment as ZEN_API_KEY,
// which the CLI reads itself. These calls can never return, store, or
// log a key.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";

export interface ModelSettingResult {
  model: string;
}

function asModelSettingResult(value: unknown): ModelSettingResult {
  const res = value as ModelSettingResult | null;
  if (!res || typeof res.model !== "string" || !res.model) {
    throw new Error(
      "Backend model setting malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// The stored model, or the CLI's free default when nothing is stored.
export async function getModelSetting(
  contract: ActionContract,
): Promise<ModelSettingResult> {
  const raw = await invoke("sen_settings_get_model", { contract });
  return asModelSettingResult(raw);
}

/// Store a model id for later turns. A running session keeps the model
/// it started with; the new model applies to sessions created after the
/// change. Refuses a blank model on the backend.
export async function setModelSetting(
  contract: ActionContract,
  model: string,
): Promise<ModelSettingResult> {
  const raw = await invoke("sen_settings_set_model", {
    contract,
    model,
  });
  return asModelSettingResult(raw);
}
