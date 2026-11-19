// Model router (Sen P0-3): choose a provider/model route for each
// agent-chat model call, with a free-only fallback chain.
//
// The model call itself still runs through the CLI bridge (zen CLI,
// stream-json): a route is only a (provider, model, base URL) triple
// handed to the bridge as --model / --base-url. This module holds no
// key, makes no network call, and never sees a key value: BYOK key
// presence is a boolean the caller supplies (the backend checks only
// whether the named env var is set and non-empty).
//
// Routes (default chain, preference order):
// 1. spark — Pi/Muse Spark, i.e. the zen CLI's own default endpoint
//    with the CLI's free default model. No base URL.
// 2. openrouter-free — OpenRouter free models, explicit base URL
//    https://openrouter.ai/api/v1, same free model id.
// 3. ollama — local Ollama, http://127.0.0.1:11434/v1. The default
//    model id ("llama3.1") is only a starting value: the user changes
//    it to a model actually pulled in their Ollama.
// BYOK is NOT in the default chain: a byok route is usable only when
// the user configured it explicitly with byokApproved=true AND the
// named key env var is present. A paid route (paid=true) is never
// usable: validation rejects it and selection skips it defensively,
// so the free core never depends on one paid provider.

export type ProviderKind =
  | "spark"
  | "openrouter-free"
  | "ollama"
  | "byok";

export interface Route {
  id: string;
  provider: ProviderKind;
  model: string;
  /** Provider base URL. Absent means the CLI's own default endpoint. */
  baseUrl?: string | null;
  /** True only for a route the user pays for themselves (BYOK). */
  paid?: boolean;
  /** BYOK only: the user approved using their own key for this route. */
  byokApproved?: boolean;
  /** BYOK only: name of the env var holding the user's key (never the value). */
  keyEnv?: string;
}

export interface RouterConfig {
  routes: Route[];
  /** Preferred route id. Absent means the first usable route. */
  preferredRoute?: string | null;
  /** When false, a failed turn is reported, never retried on another route. */
  fallbackEnabled?: boolean;
  /** BYOK only: the user approved using their own key (per-route flag wins). */
  byokApproved?: boolean;
}

export const OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1";
export const OLLAMA_BASE_URL = "http://127.0.0.1:11434/v1";
/** The CLI's free default model (cli_settings.rs DEFAULT_MODEL). */
export const FREE_DEFAULT_MODEL = "google/gemini-4-argon:free";
/** Starting value only: change to a model actually pulled in Ollama. */
export const OLLAMA_DEFAULT_MODEL = "llama3.1";
/** Max fallback attempts per turn (routes are at most this many). */
export const MAX_ATTEMPTS = 3;

export function defaultRoutes(): Route[] {
  return [
    { id: "spark", provider: "spark", model: FREE_DEFAULT_MODEL },
    {
      id: "openrouter-free",
      provider: "openrouter-free",
      model: FREE_DEFAULT_MODEL,
      baseUrl: OPENROUTER_BASE_URL,
    },
    {
      id: "ollama",
      provider: "ollama",
      model: OLLAMA_DEFAULT_MODEL,
      baseUrl: OLLAMA_BASE_URL,
    },
  ];
}

export function defaultRouterConfig(): RouterConfig {
  return { routes: defaultRoutes(), fallbackEnabled: true };
}

/** Cost label shown in log/UI. Free/local routes cost 0. */
export function routeCostLabel(route: Route): string {
  return route.provider === "byok" ? "key của bạn (BYOK)" : "0 đồng";
}

export function describeRoute(route: Route): string {
  return `${route.provider} · ${route.model} · ${routeCostLabel(route)}`;
}

const PROVIDERS: ProviderKind[] = [
  "spark",
  "openrouter-free",
  "ollama",
  "byok",
];

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * Validate a router config (unknown input: JSON from disk or IPC).
 * Returns error strings; empty means valid. A paid route is rejected
 * outright, as is a BYOK route without explicit approval.
 */
export function validateRouterConfig(config: unknown): string[] {
  const errors: string[] = [];
  if (!config || typeof config !== "object" || Array.isArray(config)) {
    return ["router config must be an object"];
  }
  const cfg = config as Record<string, unknown>;
  if (!Array.isArray(cfg.routes) || cfg.routes.length === 0) {
    errors.push("routes must be a non-empty array");
    return errors;
  }
  const ids = new Set<string>();
  for (const raw of cfg.routes as unknown[]) {
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
      errors.push("each route must be an object");
      continue;
    }
    const route = raw as Record<string, unknown>;
    if (typeof route.id !== "string" || !route.id.trim()) {
      errors.push("each route needs a non-empty id");
      continue;
    }
    if (ids.has(route.id)) {
      errors.push(`duplicate route id: ${route.id}`);
    }
    ids.add(route.id);
    if (
      typeof route.provider !== "string" ||
      !(PROVIDERS as string[]).includes(route.provider)
    ) {
      errors.push(`route ${route.id}: unknown provider ${String(route.provider)}`);
    }
    if (typeof route.model !== "string" || !route.model.trim()) {
      errors.push(`route ${route.id}: model must be non-empty`);
    }
    if (
      route.baseUrl !== undefined &&
      route.baseUrl !== null &&
      (typeof route.baseUrl !== "string" || !isHttpUrl(route.baseUrl))
    ) {
      errors.push(`route ${route.id}: baseUrl must be an http(s) URL`);
    }
    if (route.paid === true) {
      errors.push(
        `route ${route.id}: paid routes are rejected (free-only, BYOK needs approval)`,
      );
    }
    if (route.provider === "byok") {
      const approved =
        route.byokApproved === true || cfg.byokApproved === true;
      if (!approved) {
        errors.push(
          `route ${route.id}: BYOK needs explicit user approval (byokApproved)`,
        );
      }
      if (typeof route.keyEnv !== "string" || !route.keyEnv.trim()) {
        errors.push(`route ${route.id}: BYOK needs keyEnv (env var name, never a value)`);
      }
    }
  }
  if (
    cfg.preferredRoute !== undefined &&
    cfg.preferredRoute !== null &&
    typeof cfg.preferredRoute === "string" &&
    cfg.preferredRoute.trim() &&
    !ids.has(cfg.preferredRoute)
  ) {
    errors.push(`preferredRoute ${cfg.preferredRoute} is not a route id`);
  }
  return errors;
}

/** A route is usable for this turn (defensive: paid never usable). */
export function isRouteUsable(
  route: Route,
  config: RouterConfig,
  byokKeyPresent: boolean,
): boolean {
  if (route.paid === true) {
    return false;
  }
  if (route.provider === "byok") {
    const approved =
      route.byokApproved === true || config.byokApproved === true;
    return approved && byokKeyPresent;
  }
  return true;
}

/**
 * Ordered attempt list for one turn: the preferred usable route
 * first, then the remaining usable routes in chain order, deduped,
 * capped at MAX_ATTEMPTS. A turn with fallback disabled gets one
 * attempt only.
 */
export function resolveAttemptOrder(
  config: RouterConfig,
  byokKeyPresent = false,
): Route[] {
  const usable = config.routes.filter((route) =>
    isRouteUsable(route, config, byokKeyPresent),
  );
  const ordered: Route[] = [];
  const push = (route: Route) => {
    if (!ordered.some((r) => r.id === route.id)) {
      ordered.push(route);
    }
  };
  if (config.preferredRoute) {
    const preferred = usable.find((r) => r.id === config.preferredRoute);
    if (preferred) {
      push(preferred);
    }
  }
  for (const route of usable) {
    push(route);
  }
  const cap =
    config.fallbackEnabled === false ? 1 : MAX_ATTEMPTS;
  return ordered.slice(0, cap);
}

/** First route for a turn, or undefined when no route is usable. */
export function selectRoute(
  config: RouterConfig,
  byokKeyPresent = false,
): Route | undefined {
  return resolveAttemptOrder(config, byokKeyPresent)[0];
}

/**
 * Route after `failedId` in a caller-supplied attempt-order
 * snapshot, or undefined when the chain is exhausted or `failedId`
 * is not in the snapshot. A turn snapshots its order once at start
 * with resolveAttemptOrder (P2-2) and walks the snapshot, so a
 * config change mid-turn cannot move routes under the loop — the TS
 * mirror of `next_route_in` in model_router.rs (the retry loop in
 * registry.rs walks the snapshot the same way). The snapshot is a
 * plain Route[]: exactly what resolveAttemptOrder returns, no new
 * shape invented. An unknown id never restarts at the first route:
 * that route may already have been tried, and restarting it is how
 * the fallback loop ran forever when the config changed mid-turn
 * (PR #96). L2 follow-up to PR #96: nextRoute used to take the
 * config and re-resolve the order on every call, so a config change
 * between two calls could move routes under the loop — the same
 * root cause PR #96 fixed for unknown ids.
 */
export function nextRoute(
  order: Route[],
  failedId: string,
): Route | undefined {
  const at = order.findIndex((r) => r.id === failedId);
  if (at < 0) {
    return undefined;
  }
  return order[at + 1];
}

// CLI bridge error codes (cli_spawn.rs): which failures may be
// retried on the next route. Auth/rate-limit/no-result/timeout may
// be provider-specific, so another route can succeed; validation,
// forbidden, abort, and empty/invalid input are not provider
// problems and must never trigger a paid-looking retry loop.
const RETRYABLE_CODES = new Set([
  "CLI_AUTH",
  "CLI_RATE_LIMIT",
  "CLI_NO_RESULT",
  "CLI_TURN_TIMEOUT",
  "CLI_SPAWN_FAILED",
  "CLI_NOT_FOUND",
  "CLI_CONFLICT",
]);

export function isRetryableErrorCode(code: string): boolean {
  return RETRYABLE_CODES.has(code);
}

/** Split a "CODE: message" wire error into its code ("" when untyped). */
export function errorCodeOf(wireError: string): string {
  const at = wireError.indexOf(":");
  return at > 0 ? wireError.slice(0, at).trim() : "";
}
