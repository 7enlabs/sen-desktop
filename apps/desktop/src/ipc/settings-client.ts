// Frontend client for CLI bridge settings (Phase 2, tin 9530 quyết 2).
// The settings hold the MODEL id and, since R2B-05, the optional BASE
// URL of the provider endpoint. There is no key function here:
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
///
/// Wire shape (R2B-05 rev2): the Rust handler declares
/// `args: SetModelArgs`, and Tauri decodes each parameter from the
/// payload key named after it, so the model travels nested under
/// `args` - a flat `{ contract, model }` payload is rejected with
/// "missing required key args" before any handler logic runs.
export async function setModelSetting(
  contract: ActionContract,
  model: string,
): Promise<ModelSettingResult> {
  const raw = await invoke("sen_settings_set_model", {
    contract,
    args: { model },
  });
  return asModelSettingResult(raw);
}

export interface BaseUrlSettingResult {
  baseUrl: string | null;
}

function asBaseUrlSettingResult(value: unknown): BaseUrlSettingResult {
  const res = value as BaseUrlSettingResult | null;
  if (!res || (res.baseUrl !== null && typeof res.baseUrl !== "string")) {
    throw new Error(
      "Backend base URL setting malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// The stored base URL, or null when nothing is stored (R2B-05). A
/// session created with no stored base URL passes no --base-url flag.
export async function getBaseUrlSetting(
  contract: ActionContract,
): Promise<BaseUrlSettingResult> {
  const raw = await invoke("sen_settings_get_base_url", { contract });
  return asBaseUrlSettingResult(raw);
}

/// Store a provider base URL for later turns (R2B-05). A running
/// session keeps the base URL it started with; the new URL applies to
/// sessions created after the change. A blank value clears the stored
/// base URL on the backend.
///
/// Wire shape (R2B-05 rev2): the Rust handler declares
/// `args: SetBaseUrlArgs` (`base_url` renamed to `baseUrl` by serde
/// camelCase), so the URL travels nested under `args` - a flat
/// `{ contract, baseUrl }` payload is rejected with "missing required
/// key args" before any handler logic runs.
export async function setBaseUrlSetting(
  contract: ActionContract,
  baseUrl: string,
): Promise<BaseUrlSettingResult> {
  const raw = await invoke("sen_settings_set_base_url", {
    contract,
    args: { baseUrl },
  });
  return asBaseUrlSettingResult(raw);
}
