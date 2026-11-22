// Model router (Sen P0-3), Rust mirror of packages/providers/src/router.ts.
// PURE RUST, no Tauri dependency: compiles and tests standalone.
//
// The two files must stay in sync: same default chain, same order,
// same validation rules, same retryable codes. If they drift, the
// frontend display and the backend route disagree about which route
// a turn runs on. The TS suite (apps/desktop/tests/model-router.test.ts)
// and this module's tests pin the same cases on both sides.
//
// A route is only a (provider, model, base URL) triple handed to the
// CLI bridge as --model / --base-url. This module holds no key and
// makes no network call. BYOK key presence is only a boolean: whether
// the env var named in `key_env` is set and non-empty (the value is
// never read into a string we keep).
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://api.experientiallabs.ai/api/v1";
pub const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434/v1";
/// The CLI's free default model (cli_settings.rs DEFAULT_MODEL).
pub const FREE_DEFAULT_MODEL: &str = "deepseek-v4.1-flash";
/// Starting value only: change to a model actually pulled in Ollama.
pub const OLLAMA_DEFAULT_MODEL: &str = "llama3.1";
/// Max fallback attempts per turn.
pub const MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "spark")]
    Spark,
    #[serde(rename = "free")]
    Free,
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "byok")]
    Byok,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Spark => "spark",
            ProviderKind::Free => "free",
            ProviderKind::Ollama => "ollama",
            ProviderKind::Byok => "byok",
        }
    }

    // Named from_id, not from_str: a from_str here would shadow the
    // std::str::FromStr::from_str name (clippy::should_implement_trait)
    // without implementing the trait.
    pub fn from_id(value: &str) -> Option<ProviderKind> {
        match value {
            "spark" => Some(ProviderKind::Spark),
            "free" => Some(ProviderKind::Free),
            "ollama" => Some(ProviderKind::Ollama),
            "byok" => Some(ProviderKind::Byok),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub provider: ProviderKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub paid: bool,
    #[serde(default)]
    pub byok_approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
}

impl Route {
    /// Cost label shown in log/UI. Free/local routes cost 0.
    pub fn cost_label(&self) -> &'static str {
        match self.provider {
            ProviderKind::Byok => "key của bạn (BYOK)",
            _ => "0 đồng",
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} · {} · {}",
            self.provider.as_str(),
            self.model,
            self.cost_label()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterConfig {
    pub routes: Vec<Route>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_route: Option<String>,
    #[serde(default = "default_fallback_enabled")]
    pub fallback_enabled: bool,
    #[serde(default)]
    pub byok_approved: bool,
}

fn default_fallback_enabled() -> bool {
    true
}

impl Default for RouterConfig {
    fn default() -> Self {
        RouterConfig {
            routes: default_routes(),
            preferred_route: None,
            fallback_enabled: true,
            byok_approved: false,
        }
    }
}

/// Default chain, preference order: spark, free, ollama.
/// BYOK is NOT in the default chain.
pub fn default_routes() -> Vec<Route> {
    vec![
        Route {
            id: "spark".to_string(),
            provider: ProviderKind::Spark,
            model: FREE_DEFAULT_MODEL.to_string(),
            base_url: None,
            paid: false,
            byok_approved: false,
            key_env: None,
        },
        Route {
            id: "free".to_string(),
            provider: ProviderKind::Free,
            model: FREE_DEFAULT_MODEL.to_string(),
            base_url: Some(DEFAULT_BASE_URL.to_string()),
            paid: false,
            byok_approved: false,
            key_env: None,
        },
        Route {
            id: "ollama".to_string(),
            provider: ProviderKind::Ollama,
            model: OLLAMA_DEFAULT_MODEL.to_string(),
            base_url: Some(OLLAMA_BASE_URL.to_string()),
            paid: false,
            byok_approved: false,
            key_env: None,
        },
    ]
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// Validate a router config. Empty errors means valid. A paid route
/// is rejected outright, as is a BYOK route without explicit approval.
pub fn validate_router_config(config: &RouterConfig) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    if config.routes.is_empty() {
        errors.push("routes must be a non-empty array".to_string());
        return errors;
    }
    let mut ids: Vec<&str> = Vec::new();
    for route in &config.routes {
        if route.id.trim().is_empty() {
            errors.push("each route needs a non-empty id".to_string());
            continue;
        }
        if ids.contains(&route.id.as_str()) {
            errors.push(format!("duplicate route id: {}", route.id));
        }
        ids.push(route.id.as_str());
        if route.model.trim().is_empty() {
            errors.push(format!("route {}: model must be non-empty", route.id));
        }
        if let Some(url) = &route.base_url {
            if !is_http_url(url) {
                errors.push(format!(
                    "route {}: baseUrl must be an http(s) URL",
                    route.id
                ));
            }
        }
        if route.paid {
            errors.push(format!(
                "route {}: paid routes are rejected (free-only, BYOK needs approval)",
                route.id
            ));
        }
        if route.provider == ProviderKind::Byok {
            let approved = route.byok_approved || config.byok_approved;
            if !approved {
                errors.push(format!(
                    "route {}: BYOK needs explicit user approval (byokApproved)",
                    route.id
                ));
            }
            match &route.key_env {
                Some(env) if !env.trim().is_empty() => {}
                _ => errors.push(format!(
                    "route {}: BYOK needs keyEnv (env var name, never a value)",
                    route.id
                )),
            }
        }
    }
    if let Some(preferred) = &config.preferred_route {
        if !preferred.trim().is_empty() && !ids.contains(&preferred.as_str()) {
            errors.push(format!("preferredRoute {} is not a route id", preferred));
        }
    }
    errors
}

/// Whether the named env var is set and non-empty. The value is never
/// kept: only the boolean leaves this function.
pub fn key_env_present(key_env: &str) -> bool {
    match std::env::var_os(key_env) {
        Some(value) => !value.is_empty(),
        None => false,
    }
}

/// A route is usable for this turn (defensive: paid never usable).
pub fn is_route_usable(route: &Route, config: &RouterConfig, byok_key_present: bool) -> bool {
    if route.paid {
        return false;
    }
    if route.provider == ProviderKind::Byok {
        let approved = route.byok_approved || config.byok_approved;
        return approved && byok_key_present;
    }
    true
}

/// BYOK key presence for a route: true only when its key_env names a
/// set, non-empty env var. Non-BYOK routes need no key.
fn route_key_present(route: &Route) -> bool {
    match &route.key_env {
        Some(env) => key_env_present(env),
        None => false,
    }
}

/// Ordered attempt list for one turn: preferred usable route first,
/// then the rest in chain order, deduped, capped at MAX_ATTEMPTS.
/// Fallback disabled means one attempt only.
pub fn resolve_attempt_order(config: &RouterConfig) -> Vec<Route> {
    let usable: Vec<Route> = config
        .routes
        .iter()
        .filter(|route| {
            let key_present = if route.provider == ProviderKind::Byok {
                route_key_present(route)
            } else {
                false
            };
            is_route_usable(route, config, key_present)
        })
        .cloned()
        .collect();
    let mut ordered: Vec<Route> = Vec::new();
    if let Some(preferred) = &config.preferred_route {
        if let Some(route) = usable.iter().find(|r| &r.id == preferred) {
            ordered.push(route.clone());
        }
    }
    for route in &usable {
        if !ordered.iter().any(|r| r.id == route.id) {
            ordered.push(route.clone());
        }
    }
    let cap = if config.fallback_enabled {
        MAX_ATTEMPTS
    } else {
        1
    };
    ordered.truncate(cap);
    ordered
}

/// First route for a turn, or None when no route is usable.
pub fn select_route(config: &RouterConfig) -> Option<Route> {
    resolve_attempt_order(config).into_iter().next()
}

/// Route after `failed_id` in the attempt order, or None when the
/// chain is exhausted or `failed_id` is not in the order. An unknown
/// id never restarts at the first route: that route may already have
/// been tried, and restarting it is how the fallback loop ran
/// forever when the config changed mid-turn (P2-2).
pub fn next_route(config: &RouterConfig, failed_id: &str) -> Option<Route> {
    next_route_in(&resolve_attempt_order(config), failed_id)
}

/// Route after `failed_id` in a caller-supplied order snapshot, or
/// None. A turn snapshots its order once at start (P2-2) and walks
/// the snapshot, so a config change mid-turn cannot move routes
/// under the loop.
pub fn next_route_in(order: &[Route], failed_id: &str) -> Option<Route> {
    match order.iter().position(|r| r.id == failed_id) {
        Some(at) => order.get(at + 1).cloned(),
        None => None,
    }
}

/// CLI bridge error codes (cli_spawn.rs) that may be retried on the
/// next route. Auth/rate-limit/no-result/timeout may be
/// provider-specific; validation, forbidden, abort, and empty/invalid
/// input are not provider problems and must never trigger a retry.
pub fn is_retryable_error_code(code: &str) -> bool {
    matches!(
        code,
        "CLI_AUTH"
            | "CLI_RATE_LIMIT"
            | "CLI_NO_RESULT"
            | "CLI_TURN_TIMEOUT"
            | "CLI_SPAWN_FAILED"
            | "CLI_NOT_FOUND"
            | "CLI_CONFLICT"
    )
}

/// Split a "CODE: message" wire error into its code ("" when untyped).
pub fn error_code_of(wire_error: &str) -> &str {
    match wire_error.find(':') {
        Some(at) if at > 0 => wire_error[..at].trim(),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byok_route() -> Route {
        Route {
            id: "byok".to_string(),
            provider: ProviderKind::Byok,
            model: "deepseek-v4.1-flash".to_string(),
            base_url: Some(DEFAULT_BASE_URL.to_string()),
            paid: false,
            byok_approved: true,
            key_env: Some("SEN_TEST_BYOK_KEY_PRESENT".to_string()),
        }
    }

    #[test]
    fn default_chain_is_free_only_in_preference_order() {
        let routes = default_routes();
        assert_eq!(
            routes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["spark", "free", "ollama"]
        );
        for route in &routes {
            assert!(!route.paid);
            assert_ne!(route.provider, ProviderKind::Byok);
            assert_eq!(route.cost_label(), "0 đồng");
        }
        assert_eq!(routes[0].base_url, None);
        assert_eq!(routes[1].base_url.as_deref(), Some(DEFAULT_BASE_URL));
        assert_eq!(routes[2].base_url.as_deref(), Some(OLLAMA_BASE_URL));
        assert!(validate_router_config(&RouterConfig::default()).is_empty());
    }

    #[test]
    fn select_prefers_the_preferred_route_and_falls_back_in_order() {
        let config = RouterConfig {
            preferred_route: Some("ollama".to_string()),
            ..Default::default()
        };
        let order = resolve_attempt_order(&config);
        assert_eq!(order[0].id, "ollama");
        assert_eq!(order[1].id, "spark");
        assert_eq!(order[2].id, "free");
        assert_eq!(next_route(&config, "ollama").unwrap().id, "spark");
        assert_eq!(next_route(&config, "free"), None);
    }

    #[test]
    fn next_route_never_restarts_at_the_first_route() {
        // P2-2: an unknown failed id used to return the first route,
        // which may be the route that just failed — the fallback loop
        // then retried it forever when the config changed mid-turn.
        let config = RouterConfig::default();
        assert_eq!(next_route(&config, "no-such-route"), None);
        assert_eq!(next_route(&config, ""), None);
    }

    #[test]
    fn next_route_in_walks_a_snapshot_order() {
        let order = resolve_attempt_order(&RouterConfig::default());
        assert_eq!(order.len(), 3);
        assert_eq!(next_route_in(&order, "spark").unwrap().id, "free");
        assert_eq!(next_route_in(&order, "free").unwrap().id, "ollama");
        assert_eq!(next_route_in(&order, "ollama"), None);
        assert_eq!(next_route_in(&order, "no-such-route"), None);
        // A snapshot taken before a reorder still walks the old
        // order: the caller, not next_route_in, owns the snapshot.
        let mut reordered = order.clone();
        reordered.reverse();
        assert_eq!(next_route_in(&reordered, "ollama").unwrap().id, "free");
    }

    #[test]
    fn fallback_disabled_means_one_attempt() {
        let config = RouterConfig {
            fallback_enabled: false,
            ..Default::default()
        };
        assert_eq!(resolve_attempt_order(&config).len(), 1);
    }

    #[test]
    fn paid_route_is_rejected_and_never_usable() {
        let mut config = RouterConfig::default();
        let mut paid = config.routes[0].clone();
        paid.id = "paid-one".to_string();
        paid.paid = true;
        config.routes.push(paid);
        let errors = validate_router_config(&config);
        assert!(errors
            .iter()
            .any(|e| e.contains("paid routes are rejected")));
        assert_eq!(resolve_attempt_order(&config).len(), 3);
    }

    #[test]
    fn byok_needs_approval_and_a_present_key() {
        // No approval: validation fails and selection skips it.
        let mut config = RouterConfig::default();
        let mut route = byok_route();
        route.byok_approved = false;
        config.routes.push(route);
        assert!(validate_router_config(&config)
            .iter()
            .any(|e| e.contains("BYOK needs explicit user approval")));
        assert_eq!(resolve_attempt_order(&config).len(), 3);
        // Approved but key absent: validation passes, selection skips.
        let mut config = RouterConfig::default();
        config.routes.push(byok_route());
        assert!(validate_router_config(&config).is_empty());
        assert_eq!(resolve_attempt_order(&config).len(), 3);
        // Approved and key present (env var set only for this test):
        // the BYOK route joins the order last.
        unsafe {
            std::env::set_var("SEN_TEST_BYOK_KEY_PRESENT", "x");
        }
        let order = resolve_attempt_order(&config);
        assert_eq!(order.len(), 3, "cap is MAX_ATTEMPTS=3");
        unsafe {
            std::env::remove_var("SEN_TEST_BYOK_KEY_PRESENT");
        }
        let config = RouterConfig {
            routes: vec![byok_route()],
            ..Default::default()
        };
        unsafe {
            std::env::set_var("SEN_TEST_BYOK_KEY_PRESENT", "x");
        }
        assert_eq!(resolve_attempt_order(&config).len(), 1);
        unsafe {
            std::env::remove_var("SEN_TEST_BYOK_KEY_PRESENT");
        }
        assert_eq!(resolve_attempt_order(&config).len(), 0);
    }

    #[test]
    fn validation_rejects_bad_config() {
        let config = RouterConfig {
            routes: vec![],
            ..Default::default()
        };
        assert!(validate_router_config(&config)
            .iter()
            .any(|e| e.contains("non-empty array")));
        let mut config = RouterConfig::default();
        config.routes.push(config.routes[0].clone());
        assert!(validate_router_config(&config)
            .iter()
            .any(|e| e.contains("duplicate route id")));
        let config = RouterConfig {
            preferred_route: Some("no-such-route".to_string()),
            ..Default::default()
        };
        assert!(validate_router_config(&config)
            .iter()
            .any(|e| e.contains("not a route id")));
        let mut config = RouterConfig::default();
        config.routes[1].base_url = Some("not-a-url".to_string());
        assert!(validate_router_config(&config)
            .iter()
            .any(|e| e.contains("http(s) URL")));
    }

    #[test]
    fn retryable_codes_match_the_bridge_contract() {
        for code in [
            "CLI_AUTH",
            "CLI_RATE_LIMIT",
            "CLI_NO_RESULT",
            "CLI_TURN_TIMEOUT",
            "CLI_SPAWN_FAILED",
            "CLI_NOT_FOUND",
            "CLI_CONFLICT",
        ] {
            assert!(is_retryable_error_code(code), "{code} must retry");
        }
        for code in [
            "CLI_VALIDATION",
            "CLI_FORBIDDEN",
            "CLI_TURN_ABORTED",
            "CLI_EMPTY_TEXT",
            "CLI_INVALID_RECORD",
            "CLI_TRUNCATED",
            "DUPLICATE_STEER",
            "",
        ] {
            assert!(!is_retryable_error_code(code), "{code} must not retry");
        }
        assert_eq!(error_code_of("CLI_AUTH: bad key"), "CLI_AUTH");
        assert_eq!(error_code_of("no colon here"), "");
    }
}
