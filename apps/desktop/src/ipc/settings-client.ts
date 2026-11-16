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

// --- Model router (Sen P0-3) ---
// The router picks a (provider, model, base URL) route per turn from
// the free-only chain spark -> openrouter-free -> ollama, with BYOK
// only on explicit user approval. No key field ever travels here:
// byokKeyPresent is a boolean the backend computes from the env var
// name alone.

export type RouterProvider =
  | "spark"
  | "openrouter-free"
  | "ollama"
  | "byok";

export interface RouterRoute {
  id: string;
  provider: RouterProvider;
  model: string;
  baseUrl?: string | null;
  paid?: boolean;
  byokApproved?: boolean;
  keyEnv?: string;
}

export interface RouterSettingResult {
  routes: RouterRoute[];
  preferredRoute: string | null;
  fallbackEnabled: boolean;
  byokApproved: boolean;
  byokKeyPresent: boolean;
  activeRouteId: string;
}

function isRoute(value: unknown): value is RouterRoute {
  const route = value as RouterRoute | null;
  return (
    !!route &&
    typeof route.id === "string" &&
    !!route.id &&
    typeof route.provider === "string" &&
    typeof route.model === "string" &&
    !!route.model
  );
}

function asRouterSettingResult(value: unknown): RouterSettingResult {
  const res = value as RouterSettingResult | null;
  if (
    !res ||
    !Array.isArray(res.routes) ||
    res.routes.length === 0 ||
    !res.routes.every(isRoute) ||
    typeof res.fallbackEnabled !== "boolean" ||
    typeof res.byokApproved !== "boolean" ||
    typeof res.byokKeyPresent !== "boolean" ||
    typeof res.activeRouteId !== "string" ||
    !res.activeRouteId ||
    (res.preferredRoute !== null && typeof res.preferredRoute !== "string")
  ) {
    throw new Error(
      "Backend router setting malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// The stored router, or the default free-only chain when nothing is
/// stored. The active route id is the route new sessions start on.
export async function getRouterSetting(
  contract: ActionContract,
): Promise<RouterSettingResult> {
  const raw = await invoke("sen_settings_get_router", { contract });
  return asRouterSettingResult(raw);
}

export interface SetRouterArgs {
  preferredRoute?: string | null;
  fallbackEnabled?: boolean;
  byokApproved?: boolean;
  routes?: RouterRoute[];
}

/// Store a router change (partial: only the fields given change).
/// A paid route or a BYOK route without approval is refused by the
/// backend, never stored. Applies to sessions created after the
/// change; a running session keeps its route.
export async function setRouterSetting(
  contract: ActionContract,
  args: SetRouterArgs,
): Promise<RouterSettingResult> {
  const raw = await invoke("sen_settings_set_router", {
    contract,
    args,
  });
  return asRouterSettingResult(raw);
}

/// Cost label for one route, frontend-side (mirrors the router's
/// rule: free/local routes cost 0, BYOK costs the user's own key).
export function routerRouteCostLabel(route: RouterRoute): string {
  return route.provider === "byok" ? "key của bạn (BYOK)" : "0 đồng";
}

export function describeRouterRoute(route: RouterRoute): string {
  return `${route.provider} · ${route.model} · ${routerRouteCostLabel(route)}`;
}
