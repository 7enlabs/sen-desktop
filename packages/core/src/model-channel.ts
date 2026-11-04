// OpenRouter free model channel for the Sen Desktop live session round.
// Only packages/core may import the Pi SDK, per prep B-2 section 3 point 4.
// This module is the ONLY place that builds a ModelRuntime for Sen.
//
// Channel decision (tin 4528 ZC Leader, tin 4542 SD Leader): the default
// free channel is the OpenRouter free route, as in the CLI, unless SD
// Leader proposes a separate channel with reasons. No separate channel
// was proposed, so this module uses OpenRouter only.
//
// Hard rules, kept from the fill-in round and extended here:
// - The provider is always "openrouter". No other provider is used.
// - The model id is ALWAYS caller-supplied, never defaulted, never
//   fuzzy-matched, never replaced by "first available". An empty or
//   unknown model id refuses with a clear error, no fallback.
// - The API key comes ONLY from the OPENROUTER_API_KEY environment
//   variable at runtime (or an explicit caller-supplied key, used by
//   tests). It is never read from a file in this repo, never stored in
//   any file, never logged. No key, no live session.
// - The credential store is in-memory: this runtime never reads the
//   user's ~/.pi/agent/auth.json and never writes any credential file.
//   The key lives only as a runtime override for this process.
// - No network call happens in this module: runtime creation uses the
//   builtin static catalog only (refreshOnCreate false, allowModelNetwork
//   false). The only network step is a later steer/followUp on a live
//   session, which runs only after the explicit model + key checks pass.
// - This module names NO model id as a default. Model ids appearing in
//   tests are explicit test inputs, not defaults.
import { ModelRuntime } from "@earendil-works/pi-coding-agent";

export const OPENROUTER_PROVIDER_ID = "openrouter";
export const OPENROUTER_API_KEY_ENV = "OPENROUTER_API_KEY";

// Minimal structural view of the SDK credential store interface, kept
// local so this file does not import a transitive SDK package. The shape
// (read/list/modify/delete) is the whole interface the runtime needs.
export interface InMemoryCredentialEntry {
  providerId: string;
  type: string;
  key?: string;
}

export interface InMemoryCredentialStore {
  read(
    providerId: string,
    options?: unknown,
  ): Promise<InMemoryCredentialEntry | undefined>;
  list(options?: unknown): Promise<readonly InMemoryCredentialEntry[]>;
  modify(
    providerId: string,
    fn: (
      current: InMemoryCredentialEntry | undefined,
    ) => Promise<InMemoryCredentialEntry | undefined>,
    options?: unknown,
  ): Promise<InMemoryCredentialEntry | undefined>;
  delete(providerId: string, options?: unknown): Promise<void>;
}

export function createInMemoryCredentialStore(): InMemoryCredentialStore {
  const entries = new Map<string, InMemoryCredentialEntry>();
  return {
    async read(providerId: string) {
      return entries.get(providerId);
    },
    async list() {
      return [...entries.values()].map((entry) => ({
        providerId: entry.providerId,
        type: entry.type,
      }));
    },
    async modify(providerId: string, fn) {
      const next = await fn(entries.get(providerId));
      if (next !== undefined) {
        entries.set(providerId, next);
      }
      return next;
    },
    async delete(providerId: string) {
      entries.delete(providerId);
    },
  };
}

// Resolves the OpenRouter API key from an environment record. The default
// record is process.env. A missing or blank key refuses: Sen never falls
// back to another key, model, or provider, and never reads a key file.
export function resolveOpenRouterApiKey(
  env: Record<string, string | undefined> = process.env,
): string {
  const key = env[OPENROUTER_API_KEY_ENV]?.trim();
  if (!key) {
    throw new Error(
      "OpenRouter API key is not configured: set the " +
        OPENROUTER_API_KEY_ENV +
        " environment variable before starting a live session. " +
        "Sen never falls back to another key, model, or provider.",
    );
  }
  return key;
}

export interface OpenRouterRuntimeOptions {
  // Explicit key. When omitted, the key is resolved from the environment
  // record (process.env by default). Tests pass an explicit key so they
  // never touch the real environment.
  apiKey?: string;
  env?: Record<string, string | undefined>;
}

// Builds a ModelRuntime for OpenRouter only, with an in-memory credential
// store and the builtin static catalog. No network call, no file read
// from the user's Pi settings, no credential file written. The key is
// attached as a runtime-only override for this process.
export async function createOpenRouterRuntime(
  options: OpenRouterRuntimeOptions = {},
): Promise<ModelRuntime> {
  const apiKey = options.apiKey ?? resolveOpenRouterApiKey(options.env);
  if (!apiKey.trim()) {
    throw new Error(
      "OpenRouter API key is not configured: an explicit key must not be blank.",
    );
  }
  const runtime = await ModelRuntime.create({
    credentials: createInMemoryCredentialStore() as never,
    modelsPath: null,
    refreshOnCreate: false,
    allowModelNetwork: false,
  });
  await runtime.setRuntimeApiKey(OPENROUTER_PROVIDER_ID, apiKey);
  return runtime;
}

// Resolves one model from the OpenRouter catalog by exact id. Empty and
// unknown ids refuse: no default, no fuzzy match, no first-available.
export function resolveOpenRouterModel(
  runtime: ModelRuntime,
  modelId: string,
): unknown {
  if (!modelId || !modelId.trim()) {
    throw new Error(
      "No model specified: refusing to resolve a default model for a live " +
        "session (no default model per message 4070, no first-available " +
        "fallback). Pass an explicit OpenRouter model id.",
    );
  }
  const model = runtime.getModel(OPENROUTER_PROVIDER_ID, modelId);
  if (!model) {
    throw new Error(
      "Model not found in the OpenRouter catalog: " +
        modelId +
        ". No fallback is applied; pass an explicit model id that the " +
        "OpenRouter catalog carries.",
    );
  }
  return model;
}

// Lists the free OpenRouter model ids in the builtin catalog. Listing
// only: this function never selects a model, and its output is never
// used as a default by any caller.
export function listOpenRouterFreeModels(runtime: ModelRuntime): string[] {
  return runtime
    .getModels(OPENROUTER_PROVIDER_ID)
    .map((model) => model.id)
    .filter((id) => id.endsWith(":free"));
}
