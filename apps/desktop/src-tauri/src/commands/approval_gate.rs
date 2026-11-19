// Approval gate (Sen P0-2), Rust mirror of
// packages/core/src/approval-gate.ts. PURE RUST, no Tauri dependency:
// compiles and tests standalone.
//
// The two files must stay in sync: same tier tables, same evaluation
// order, same reasons. If they drift, the frontend hook and the
// backend gate disagree about whether a tool may run.
//
// Tiers (tin 13895): auto (read-only + drafts, runs at once when the
// policy allows it), ask-first (send / buy / sign / delete, needs an
// explicit approval), never (securities trading, transfers between
// accounts, access to another account — always refused, no approval
// path). Default-deny: the policy is an explicit allow-list, empty by
// default; a tool runs only with a rule AND its tier's condition.
// An unclassified tool is ask-first, never auto.
//
// The activity log records EVERY evaluation (allowed, denied,
// awaiting-approval). It is append-only: no delete, no update, no
// clear; seq never reuses a number; readers get clones. Timestamps
// are seconds since the Unix epoch (no chrono in this crate); seq is
// the ordering authority, the timestamp is informational.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalTier {
    Auto,
    AskFirst,
    Never,
}

impl ApprovalTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalTier::Auto => "auto",
            ApprovalTier::AskFirst => "ask-first",
            ApprovalTier::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateDecision {
    Allow,
    AskFirst,
    Deny,
}

impl GateDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateDecision::Allow => "allow",
            GateDecision::AskFirst => "ask-first",
            GateDecision::Deny => "deny",
        }
    }
}

/// Tier 1: read-only work and drafts. Nothing here sends, pays,
/// signs, or deletes.
pub const AUTO_TOOLS: &[&str] = &[
    "read_file",
    "read_status",
    "list_files",
    "list_sessions",
    "search_files",
    "search_web",
    "read_memory",
    "get_status",
    "draft_message",
    "draft_email",
    "draft_post",
    "draft_note",
];

/// Tier 2: send, buy, sign, delete. Needs an explicit approval for
/// the exact tool + idempotency key before it runs.
pub const ASK_FIRST_TOOLS: &[&str] = &[
    "send_message",
    "send_email",
    "send_telegram",
    "publish_post",
    "purchase",
    "buy",
    "pay",
    "sign",
    "sign_document",
    "delete_file",
    "delete_message",
    "delete_session",
    "delete_note",
];

/// Tier 3: never. No approval and no policy rule can override this.
pub const NEVER_TOOLS: &[&str] = &[
    "trade_securities",
    "buy_stock",
    "sell_stock",
    "stock_trade",
    "transfer_between_accounts",
    "transfer_money",
    "access_other_account",
    "impersonate_account",
    "login_as_other_user",
];

/// Which tier a tool belongs to. Exact name match only; an unknown
/// tool is ask-first, never auto: new tools fail closed until someone
/// classifies them and writes an explicit allow rule.
pub fn classify_tool(tool_name: &str) -> ApprovalTier {
    if NEVER_TOOLS.contains(&tool_name) {
        return ApprovalTier::Never;
    }
    if AUTO_TOOLS.contains(&tool_name) {
        return ApprovalTier::Auto;
    }
    ApprovalTier::AskFirst
}

pub fn describe_tier(tier: ApprovalTier) -> &'static str {
    match tier {
        ApprovalTier::Auto => "auto: read-only or draft, runs at once when allowed",
        ApprovalTier::AskFirst => "ask-first: send / buy / sign / delete, needs approval",
        ApprovalTier::Never => "never: always refused, no approval path",
    }
}

/// Tool policy: an explicit allow-list of tool names. Empty by
/// default, which denies everything (default-deny). A rule for a
/// never-tier tool is inert: it never makes that tool runnable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicy {
    #[serde(default)]
    pub allow: HashSet<String>,
}

impl ToolPolicy {
    pub fn new() -> ToolPolicy {
        ToolPolicy {
            allow: HashSet::new(),
        }
    }

    pub fn allow_tool(&mut self, tool_name: &str) {
        self.allow.insert(tool_name.to_string());
    }

    pub fn allows(&self, tool_name: &str) -> bool {
        self.allow.contains(tool_name)
    }

    /// Validate a policy: every entry must be a non-empty name.
    pub fn validate(&self) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        for name in &self.allow {
            if name.trim().is_empty() {
                errors.push("tool policy allow entries must be non-empty strings".to_string());
            }
        }
        errors
    }
}

/// One explicit approval: granted by a person, for one tool call,
/// in one live session. A grant is bounded three ways (Sen P1-4,
/// same as the TS side): it belongs to exactly one session (the
/// live session's id — the SessionManager key / CLI --session-id
/// UUID, never invented by the gate), it expires APPROVAL_TTL_SECS
/// after it is granted, and it is spent the first time a gate allows
/// a call with it — a second call with the same tool + key waits for
/// a fresh approval, and a call in another session never sees this
/// grant at all. Expiry is epoch seconds (no chrono in this crate),
/// matching the `epoch-<seconds>` shape of granted_at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub tool_name: String,
    pub idempotency_key: String,
    pub granted_by: String,
    pub granted_at: String,
    /// Epoch seconds after which the grant no longer counts.
    pub expires_at_secs: u64,
    /// The live session this grant belongs to. A grant never
    /// authorizes a call in another session, even with the same
    /// tool + idempotency key.
    pub session_id: String,
}

/// Default lifetime of a grant: 15 minutes, the same window as
/// APPROVAL_TTL_MS on the TS side. Long enough for a person to
/// approve in the UI and the call to run at once; short enough
/// that an approval from long ago can never authorize a later run.
pub const APPROVAL_TTL_SECS: u64 = 15 * 60;

/// In-memory approval grants. A grant matches one session + one
/// tool + one key, lives at most APPROVAL_TTL_SECS, and is spent on
/// first use. The session is the first key part on purpose: the
/// same tool + key in another session is a different grant, so a
/// session can never spend another session's approval.
#[derive(Debug, Clone, Default)]
pub struct ApprovalStore {
    grants: HashMap<(String, String, String), Approval>,
}

fn epoch_seconds_now_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn epoch_seconds_now() -> String {
    format!("epoch-{}", epoch_seconds_now_u64())
}

impl ApprovalStore {
    pub fn new() -> ApprovalStore {
        ApprovalStore {
            grants: HashMap::new(),
        }
    }

    /// Grant one approval for one tool + key in one session. The
    /// session id is the live session's id (SessionManager key /
    /// CLI --session-id UUID), never invented here: callers pass the
    /// session they are granting for, and the Tauri boundary
    /// (Registry::approve_tool) rejects an empty one before this
    /// runs. Argument order mirrors the TS side:
    /// grant(tool, key, by, sessionId).
    pub fn grant(
        &mut self,
        tool_name: &str,
        idempotency_key: &str,
        granted_by: &str,
        session_id: &str,
    ) -> Approval {
        self.grant_with_ttl(
            tool_name,
            idempotency_key,
            granted_by,
            session_id,
            APPROVAL_TTL_SECS,
        )
    }

    /// Grant with an explicit lifetime in seconds. A ttl of 0
    /// expires at once: the grant is dead on arrival and reads as
    /// absent (fail closed, no panic).
    pub fn grant_with_ttl(
        &mut self,
        tool_name: &str,
        idempotency_key: &str,
        granted_by: &str,
        session_id: &str,
        ttl_secs: u64,
    ) -> Approval {
        let granted_secs = epoch_seconds_now_u64();
        self.grant_with_expiry(
            tool_name,
            idempotency_key,
            granted_by,
            session_id,
            granted_secs.saturating_add(ttl_secs),
        )
    }

    /// Grant with a pinned expiry (epoch seconds). The test path
    /// for an already-expired grant, and the only way to set an
    /// expiry that is not "now + ttl".
    pub fn grant_with_expiry(
        &mut self,
        tool_name: &str,
        idempotency_key: &str,
        granted_by: &str,
        session_id: &str,
        expires_at_secs: u64,
    ) -> Approval {
        let approval = Approval {
            tool_name: tool_name.to_string(),
            idempotency_key: idempotency_key.to_string(),
            granted_by: granted_by.to_string(),
            granted_at: epoch_seconds_now(),
            expires_at_secs,
            session_id: session_id.to_string(),
        };
        self.grants.insert(
            (
                session_id.to_string(),
                tool_name.to_string(),
                idempotency_key.to_string(),
            ),
            approval.clone(),
        );
        approval
    }

    fn is_expired(approval: &Approval, now_secs: u64) -> bool {
        approval.expires_at_secs <= now_secs
    }

    /// Look a grant up by session + tool + key. A grant from
    /// another session reads as absent here, exactly like a grant
    /// that was never made.
    pub fn find(
        &mut self,
        tool_name: &str,
        idempotency_key: &str,
        session_id: &str,
    ) -> Option<Approval> {
        let key = (
            session_id.to_string(),
            tool_name.to_string(),
            idempotency_key.to_string(),
        );
        let found = self.grants.get(&key).cloned()?;
        // An expired grant counts as absent, and is dropped so it
        // can never be honored later: expiry is enforced on read,
        // not only on grant.
        if Self::is_expired(&found, epoch_seconds_now_u64()) {
            self.grants.remove(&key);
            return None;
        }
        Some(found)
    }

    /// Spend a grant: return it only if it is still live, and
    /// remove it either way, so one approval authorizes exactly
    /// one allowed call. Returns None for an absent or expired
    /// grant — including a grant that belongs to another session.
    pub fn consume(
        &mut self,
        tool_name: &str,
        idempotency_key: &str,
        session_id: &str,
    ) -> Option<Approval> {
        let found = self.find(tool_name, idempotency_key, session_id)?;
        self.grants.remove(&(
            session_id.to_string(),
            tool_name.to_string(),
            idempotency_key.to_string(),
        ));
        Some(found)
    }

    /// Revoking a grant removes the grant, never a log entry.
    /// Scoped to the session: revoking in the wrong session removes
    /// nothing and returns false.
    pub fn revoke(&mut self, tool_name: &str, idempotency_key: &str, session_id: &str) -> bool {
        self.grants
            .remove(&(
                session_id.to_string(),
                tool_name.to_string(),
                idempotency_key.to_string(),
            ))
            .is_some()
    }

    /// Revoke every live grant of one session (sen_session_end): a
    /// session that ends must leave no unused approval behind for
    /// it, or the grant would outlive its session by up to the TTL.
    /// Only live grants are removed and counted: a grant already
    /// spent is not in the store, and an expired grant is dead
    /// already (find/list drop it on read), so it is neither
    /// counted nor removed here. Returns the number revoked.
    /// Grants of other sessions are untouched.
    pub fn revoke_all_for_session(&mut self, session_id: &str) -> u64 {
        let now_secs = epoch_seconds_now_u64();
        let before = self.grants.len();
        self.grants.retain(|_, approval| {
            approval.session_id != session_id || Self::is_expired(approval, now_secs)
        });
        // The retained set lost only this session's live grants:
        // every other entry (other sessions, plus this session's
        // already-expired grants) is still there, so the drop in
        // size is exactly the revoked count.
        (before - self.grants.len()) as u64
    }

    pub fn list(&mut self) -> Vec<Approval> {
        let now_secs = epoch_seconds_now_u64();
        // Drop expired grants first, so a dead grant never shows
        // up as usable again.
        self.grants
            .retain(|_, approval| !Self::is_expired(approval, now_secs));
        let mut approvals: Vec<Approval> = self.grants.values().cloned().collect();
        approvals.sort_by(|a, b| {
            a.session_id
                .cmp(&b.session_id)
                .then(a.tool_name.cmp(&b.tool_name))
                .then(a.idempotency_key.cmp(&b.idempotency_key))
        });
        approvals
    }
}

/// One recorded tool-call evaluation. Seq is the ordering authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub seq: u64,
    pub timestamp: String,
    pub actor: String,
    pub tool: String,
    pub tier: ApprovalTier,
    pub decision: GateDecision,
    pub reason: String,
    pub result: String,
}

/// Retention bound for the activity log (P2-5, same bound as the
/// TS ActivityLog and RawLog's MAX_RAW_LOG_EVENTS). Why 10,000: one
/// entry is recorded per evaluated tool call, and a session makes
/// only a handful of tool calls per turn, so even a 1,000-turn
/// session is a few thousand entries; 10,000 sits an order of
/// magnitude above any ordinary session, so no useful history of a
/// normal session is lost. Why a bound at all: entries carry full
/// reason strings, so an unbounded log grows with session length
/// without limit. Eviction is FIFO: the oldest entry goes first, the
/// surviving entries keep their append order, and seq never reuses a
/// number, so a gap in seq is the observable trace that eviction
/// happened. The cap is not a delete function and does not change
/// the no-delete rule: no caller can delete anything, there is
/// still no public delete API, and eviction happens only inside
/// record, only to the oldest entry, only when the fixed bound is
/// exceeded (INFO-1 precedent: eviction under a cap is not
/// deletion).
pub const MAX_ACTIVITY_LOG_ENTRIES: usize = 10_000;

/// Append-only activity log. There is deliberately no delete, no
/// update, and no clear: entries only ever get appended, seq never
/// reuses a number, and readers get clones. The one exception is
/// the retention cap above: when the log holds more than
/// MAX_ACTIVITY_LOG_ENTRIES entries, record drops the oldest one
/// (FIFO). That is eviction under a cap, not deletion — see the
/// MAX_ACTIVITY_LOG_ENTRIES note.
#[derive(Debug, Clone)]
pub struct ActivityLog {
    entries: Vec<ActivityEntry>,
    next_seq: u64,
    max_entries: usize,
}

impl Default for ActivityLog {
    /// A default log keeps the production cap: a derived Default
    /// would set max_entries to 0 and evict every entry on record,
    /// so Default delegates to new() instead.
    fn default() -> ActivityLog {
        ActivityLog::new()
    }
}

impl ActivityLog {
    pub fn new() -> ActivityLog {
        ActivityLog {
            entries: Vec::new(),
            next_seq: 1,
            max_entries: MAX_ACTIVITY_LOG_ENTRIES,
        }
    }

    /// Test-only: a log with an explicit bound, so the cap is
    /// exercised through a small bound and the tests stay fast. The
    /// production default (MAX_ACTIVITY_LOG_ENTRIES) is asserted,
    /// not filled, because recording 10,000 entries in a test
    /// would prove nothing the small-cap tests do not already
    /// prove. Panics on a zero bound: a log that keeps nothing
    /// cannot record, fail closed at construction.
    #[cfg(test)]
    pub fn with_max_entries(max_entries: usize) -> ActivityLog {
        assert!(max_entries >= 1, "max_entries must be at least 1, FAIL");
        ActivityLog {
            entries: Vec::new(),
            next_seq: 1,
            max_entries,
        }
    }

    pub fn record(
        &mut self,
        actor: &str,
        tool: &str,
        tier: ApprovalTier,
        decision: GateDecision,
        reason: &str,
    ) -> ActivityEntry {
        let result = match decision {
            GateDecision::Allow => "allowed",
            GateDecision::AskFirst => "awaiting-approval",
            GateDecision::Deny => "denied",
        };
        let entry = ActivityEntry {
            seq: self.next_seq,
            timestamp: epoch_seconds_now(),
            actor: actor.to_string(),
            tool: tool.to_string(),
            tier,
            decision,
            reason: reason.to_string(),
            result: result.to_string(),
        };
        self.next_seq += 1;
        self.entries.push(entry.clone());
        // P2-5 retention cap: FIFO eviction under MAX_ACTIVITY_LOG_ENTRIES,
        // same as RawLog. Seq never reuses a number, so the gap left by
        // an evicted entry stays visible.
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
        entry
    }

    pub fn entries(&self) -> Vec<ActivityEntry> {
        self.entries.clone()
    }

    pub fn entries_for(&self, tool: &str) -> Vec<ActivityEntry> {
        self.entries
            .iter()
            .filter(|e| e.tool == tool)
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvaluation {
    pub tier: ApprovalTier,
    pub decision: GateDecision,
    pub reason: String,
}

/// Pure evaluation, in fixed order: never-tier first (no override),
/// then the default-deny policy, then the approval check for
/// ask-first tools. No side effect on live grants, no logging: use
/// gate_tool_call for the evaluated + recorded + consuming path.
/// (find() drops grants that already expired, so an expired grant
/// reads as absent here too; a live grant is never spent by this
/// function — only the caller that acts on an allow spends it.)
/// Takes &mut approvals because find() drops expired grants.
/// The session id scopes the approval lookup: a grant from another
/// session is no grant at all, so the call waits for its own
/// approval.
pub fn evaluate_tool_call(
    policy: &ToolPolicy,
    approvals: &mut ApprovalStore,
    tool: &str,
    idempotency_key: &str,
    session_id: &str,
) -> GateEvaluation {
    let tier = classify_tool(tool);
    if tier == ApprovalTier::Never {
        return GateEvaluation {
            tier,
            decision: GateDecision::Deny,
            reason: format!(
                "tool \"{}\" is never-tier (securities trading / transfer between accounts / access to another account): always denied, no approval path",
                tool
            ),
        };
    }
    if !policy.allows(tool) {
        return GateEvaluation {
            tier,
            decision: GateDecision::Deny,
            reason: format!("default-deny: no explicit allow rule for tool \"{}\"", tool),
        };
    }
    if tier == ApprovalTier::AskFirst {
        match approvals.find(tool, idempotency_key, session_id) {
            None => {
                return GateEvaluation {
                    tier,
                    decision: GateDecision::AskFirst,
                    reason: format!("tool \"{}\" needs explicit approval before it runs", tool),
                };
            }
            Some(approval) => {
                return GateEvaluation {
                    tier,
                    decision: GateDecision::Allow,
                    reason: format!(
                        "tool \"{}\" is ask-first, explicitly allowed and approved by {}",
                        tool, approval.granted_by
                    ),
                };
            }
        }
    }
    GateEvaluation {
        tier,
        decision: GateDecision::Allow,
        reason: format!("tool \"{}\" is auto-tier and explicitly allowed", tool),
    }
}

/// Evaluate one tool call AND record it in the activity log: allowed,
/// denied, and awaiting-approval are all recorded, exactly once each.
/// "Allow" here means permission granted, not executed.
/// An allow for an ask-first tool spends the approval that allowed
/// it (ApprovalStore::consume): one grant authorizes exactly one
/// call, so replaying the same tool + key waits for a fresh
/// approval. Auto-tier allows spend nothing (no approval exists).
pub fn gate_tool_call(
    policy: &ToolPolicy,
    approvals: &mut ApprovalStore,
    log: &mut ActivityLog,
    actor: &str,
    tool: &str,
    idempotency_key: &str,
    session_id: &str,
) -> GateEvaluation {
    let evaluation = evaluate_tool_call(policy, approvals, tool, idempotency_key, session_id);
    log.record(
        actor,
        tool,
        evaluation.tier,
        evaluation.decision,
        &evaluation.reason,
    );
    if evaluation.decision == GateDecision::Allow && evaluation.tier == ApprovalTier::AskFirst {
        approvals.consume(tool, idempotency_key, session_id);
    }
    evaluation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_allowing(tools: &[&str]) -> ToolPolicy {
        let mut policy = ToolPolicy::new();
        for tool in tools {
            policy.allow_tool(tool);
        }
        policy
    }

    #[test]
    fn classify_read_only_and_drafts_are_auto() {
        for tool in [
            "read_file",
            "read_status",
            "list_files",
            "list_sessions",
            "search_files",
            "search_web",
            "read_memory",
            "get_status",
            "draft_message",
            "draft_email",
            "draft_post",
            "draft_note",
        ] {
            assert_eq!(classify_tool(tool), ApprovalTier::Auto, "{tool}");
        }
    }

    #[test]
    fn classify_send_buy_sign_delete_are_ask_first() {
        for tool in [
            "send_message",
            "send_email",
            "send_telegram",
            "publish_post",
            "purchase",
            "buy",
            "pay",
            "sign",
            "sign_document",
            "delete_file",
            "delete_message",
            "delete_session",
            "delete_note",
        ] {
            assert_eq!(classify_tool(tool), ApprovalTier::AskFirst, "{tool}");
        }
    }

    #[test]
    fn classify_forbidden_three_are_never() {
        for tool in [
            "trade_securities",
            "buy_stock",
            "sell_stock",
            "stock_trade",
            "transfer_between_accounts",
            "transfer_money",
            "access_other_account",
            "impersonate_account",
            "login_as_other_user",
        ] {
            assert_eq!(classify_tool(tool), ApprovalTier::Never, "{tool}");
        }
    }

    #[test]
    fn classify_unknown_tool_is_ask_first_never_auto() {
        assert_eq!(classify_tool("some-new-tool"), ApprovalTier::AskFirst);
        assert_eq!(classify_tool("edit_file"), ApprovalTier::AskFirst);
        assert_eq!(classify_tool(""), ApprovalTier::AskFirst);
    }

    #[test]
    fn default_policy_denies_everything() {
        let policy = ToolPolicy::new();
        let mut approvals = ApprovalStore::new();
        for tool in ["read_file", "send_message", "trade_securities"] {
            let eval = evaluate_tool_call(&policy, &mut approvals, tool, "k1", "s1");
            assert_eq!(eval.decision, GateDecision::Deny, "{tool}");
        }
    }

    #[test]
    fn auto_tool_allowed_only_with_an_explicit_rule() {
        let mut approvals = ApprovalStore::new();
        let policy = policy_allowing(&["read_file"]);
        let eval = evaluate_tool_call(&policy, &mut approvals, "read_file", "k1", "s1");
        assert_eq!(eval.tier, ApprovalTier::Auto);
        assert_eq!(eval.decision, GateDecision::Allow);
        assert!(eval.reason.contains("auto-tier and explicitly allowed"));
    }

    #[test]
    fn ask_first_blocked_without_approval_allowed_with_it() {
        let policy = policy_allowing(&["send_message"]);
        let mut approvals = ApprovalStore::new();
        let eval = evaluate_tool_call(&policy, &mut approvals, "send_message", "k1", "s1");
        assert_eq!(eval.decision, GateDecision::AskFirst);
        approvals.grant("send_message", "k1", "zen", "s1");
        let eval = evaluate_tool_call(&policy, &mut approvals, "send_message", "k1", "s1");
        assert_eq!(eval.decision, GateDecision::Allow);
        assert!(eval.reason.contains("approved by zen"));
        // A different key is a different call: still waiting.
        let eval = evaluate_tool_call(&policy, &mut approvals, "send_message", "k2", "s1");
        assert_eq!(eval.decision, GateDecision::AskFirst);
    }

    #[test]
    fn ask_first_approval_without_a_policy_rule_still_denies() {
        let policy = ToolPolicy::new();
        let mut approvals = ApprovalStore::new();
        approvals.grant("delete_file", "k1", "zen", "s1");
        let eval = evaluate_tool_call(&policy, &mut approvals, "delete_file", "k1", "s1");
        assert_eq!(eval.decision, GateDecision::Deny);
        assert!(eval.reason.contains("default-deny"));
    }

    #[test]
    fn never_tier_denied_even_with_rule_and_approval() {
        let policy =
            policy_allowing(&["trade_securities", "transfer_money", "access_other_account"]);
        let mut approvals = ApprovalStore::new();
        for tool in ["trade_securities", "transfer_money", "access_other_account"] {
            approvals.grant(tool, "k1", "zen", "s1");
            let eval = evaluate_tool_call(&policy, &mut approvals, tool, "k1", "s1");
            assert_eq!(eval.tier, ApprovalTier::Never, "{tool}");
            assert_eq!(eval.decision, GateDecision::Deny, "{tool}");
            assert!(eval.reason.contains("no approval path"), "{tool}");
        }
    }

    #[test]
    fn approval_store_grant_find_revoke_list() {
        let mut store = ApprovalStore::new();
        assert_eq!(store.find("send_message", "k1", "s1"), None);
        let approval = store.grant("send_message", "k1", "zen", "s1");
        assert_eq!(approval.tool_name, "send_message");
        assert_eq!(approval.granted_by, "zen");
        assert!(!approval.granted_at.is_empty());
        // A grant expires APPROVAL_TTL_SECS after it is granted.
        assert!(approval.expires_at_secs > 0);
        assert_eq!(
            store.find("send_message", "k1", "s1"),
            Some(approval.clone())
        );
        assert_eq!(store.find("send_message", "k2", "s1"), None);
        assert_eq!(store.list().len(), 1);
        assert!(store.revoke("send_message", "k1", "s1"));
        assert!(!store.revoke("send_message", "k1", "s1"));
        assert_eq!(store.find("send_message", "k1", "s1"), None);
        assert!(store.list().is_empty());
    }

    // --- P1-4: grants expire and are single-use ---

    fn epoch_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn grant_carries_an_expiry_default_15_minutes() {
        assert_eq!(APPROVAL_TTL_SECS, 15 * 60);
        let mut store = ApprovalStore::new();
        let before = epoch_now();
        let approval = store.grant("send_message", "k1", "zen", "s1");
        let after = epoch_now();
        // Expiry is granted-at + TTL, within the seconds this test took.
        assert!(approval.expires_at_secs >= before + APPROVAL_TTL_SECS);
        assert!(approval.expires_at_secs <= after + APPROVAL_TTL_SECS);
        // A custom TTL shifts the expiry by exactly that many seconds.
        let before_short = epoch_now();
        let short = store.grant_with_ttl("send_message", "k2", "zen", "s1", 60);
        let after_short = epoch_now();
        assert!(short.expires_at_secs >= before_short + 60);
        assert!(short.expires_at_secs <= after_short + 60);
    }

    #[test]
    fn expired_grant_reads_as_absent_and_never_allows_again() {
        let policy = policy_allowing(&["send_message"]);
        let mut approvals = ApprovalStore::new();
        // Granted with an expiry already in the past: dead on arrival.
        approvals.grant_with_expiry("send_message", "k-exp", "boss", "s1", epoch_now() - 10);
        assert_eq!(approvals.find("send_message", "k-exp", "s1"), None);
        assert!(approvals.list().is_empty());
        let eval = evaluate_tool_call(&policy, &mut approvals, "send_message", "k-exp", "s1");
        assert_eq!(eval.decision, GateDecision::AskFirst);
    }

    // --- P1-4 Rust session (19/11, tin 16891): a grant belongs to
    // one live session. Same tool + key in another session is a
    // different grant; session A can never spend session B's grant.

    #[test]
    fn grant_in_one_session_cannot_authorize_another_session() {
        let policy = policy_allowing(&["send_message"]);
        let mut approvals = ApprovalStore::new();
        let granted = approvals.grant("send_message", "k1", "zen", "s-A");
        assert_eq!(granted.session_id, "s-A");
        // Store level: the other session sees nothing, spends
        // nothing, revokes nothing.
        assert_eq!(approvals.find("send_message", "k1", "s-B"), None);
        assert_eq!(approvals.consume("send_message", "k1", "s-B"), None);
        assert!(!approvals.revoke("send_message", "k1", "s-B"));
        assert!(approvals.find("send_message", "k1", "s-A").is_some());
        // Gate level: session B waits, session A passes.
        let eval_b = evaluate_tool_call(&policy, &mut approvals, "send_message", "k1", "s-B");
        assert_eq!(eval_b.decision, GateDecision::AskFirst);
        let eval_a = evaluate_tool_call(&policy, &mut approvals, "send_message", "k1", "s-A");
        assert_eq!(eval_a.decision, GateDecision::Allow);
        // Session B still waits after A was allowed (evaluate spends
        // nothing); the grant is still A's until the gate spends it.
        assert!(approvals.find("send_message", "k1", "s-A").is_some());
    }

    #[test]
    fn same_tool_and_key_in_two_sessions_are_two_grants() {
        let mut store = ApprovalStore::new();
        let a = store.grant("send_message", "k1", "zen", "s-A");
        let b = store.grant("send_message", "k1", "zen", "s-B");
        assert_ne!(a.session_id, b.session_id);
        assert_eq!(store.list().len(), 2);
        // Spending A's grant leaves B's untouched.
        assert!(store.consume("send_message", "k1", "s-A").is_some());
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].session_id, "s-B");
        assert!(store.find("send_message", "k1", "s-B").is_some());
    }

    #[test]
    fn approval_is_single_use_through_consume_and_gate_tool_call() {
        let policy = policy_allowing(&["send_message"]);
        let mut approvals = ApprovalStore::new();
        approvals.grant("send_message", "k1", "zen", "s1");
        // consume() spends the grant: the second consume finds nothing.
        let spent = approvals.consume("send_message", "k1", "s1");
        assert!(spent.is_some());
        assert_eq!(spent.unwrap().granted_by, "zen");
        assert_eq!(approvals.consume("send_message", "k1", "s1"), None);
        // gate_tool_call spends it too: first call allows, the same
        // tool + key again waits for a fresh approval.
        approvals.grant("send_message", "k1", "zen", "s1");
        let mut log = ActivityLog::new();
        let first = gate_tool_call(
            &policy,
            &mut approvals,
            &mut log,
            "tester",
            "send_message",
            "k1",
            "s1",
        );
        assert_eq!(first.decision, GateDecision::Allow);
        assert_eq!(approvals.find("send_message", "k1", "s1"), None);
        let second = gate_tool_call(
            &policy,
            &mut approvals,
            &mut log,
            "tester",
            "send_message",
            "k1",
            "s1",
        );
        assert_eq!(second.decision, GateDecision::AskFirst);
        assert_eq!(
            log.entries()
                .iter()
                .map(|e| e.result.as_str())
                .collect::<Vec<_>>(),
            vec!["allowed", "awaiting-approval"]
        );
    }

    #[test]
    fn evaluate_tool_call_does_not_spend_a_live_grant() {
        // Pure evaluation reads the grant but never spends it: only
        // the caller that acts on an allow (gate_tool_call) spends it.
        let policy = policy_allowing(&["send_message"]);
        let mut approvals = ApprovalStore::new();
        approvals.grant("send_message", "k1", "zen", "s1");
        let eval = evaluate_tool_call(&policy, &mut approvals, "send_message", "k1", "s1");
        assert_eq!(eval.decision, GateDecision::Allow);
        assert!(approvals.find("send_message", "k1", "s1").is_some());
    }

    #[test]
    fn activity_log_records_every_decision_with_monotonic_seq() {
        let mut log = ActivityLog::new();
        assert!(log.is_empty());
        let policy = policy_allowing(&["read_file", "send_message"]);
        let mut approvals = ApprovalStore::new();
        let a = gate_tool_call(
            &policy,
            &mut approvals,
            &mut log,
            "tester",
            "read_file",
            "k1",
            "s1",
        );
        assert_eq!(a.decision, GateDecision::Allow);
        let b = gate_tool_call(
            &policy,
            &mut approvals,
            &mut log,
            "tester",
            "send_message",
            "k1",
            "s1",
        );
        assert_eq!(b.decision, GateDecision::AskFirst);
        let c = gate_tool_call(
            &policy,
            &mut approvals,
            &mut log,
            "tester",
            "trade_securities",
            "k1",
            "s1",
        );
        assert_eq!(c.decision, GateDecision::Deny);
        assert_eq!(log.len(), 3);
        let entries = log.entries();
        assert_eq!(
            entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(entries[0].result, "allowed");
        assert_eq!(entries[1].result, "awaiting-approval");
        assert_eq!(entries[2].result, "denied");
        assert_eq!(entries[2].tier, ApprovalTier::Never);
        assert_eq!(log.entries_for("send_message").len(), 1);
        assert_eq!(log.entries_for("no-such-tool").len(), 0);
        // Readers get clones: mutating the returned list changes
        // nothing in the log.
        let mut stolen = log.entries();
        stolen.clear();
        assert_eq!(log.len(), 3);
    }

    // --- P2-5: the activity log is capped, FIFO, seq never reused ---

    #[test]
    fn activity_log_default_cap_is_10000_entries() {
        assert_eq!(MAX_ACTIVITY_LOG_ENTRIES, 10_000);
        let mut log = ActivityLog::new();
        for i in 0..3 {
            log.record(
                "tester",
                "read_file",
                ApprovalTier::Auto,
                GateDecision::Allow,
                &format!("ok {i}"),
            );
        }
        assert_eq!(log.len(), 3);
        assert_eq!(
            log.entries().iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn activity_log_zero_cap_is_rejected_at_construction() {
        let result = std::panic::catch_unwind(|| ActivityLog::with_max_entries(0));
        assert!(result.is_err());
    }

    #[test]
    fn activity_log_evicts_oldest_first_under_a_small_cap() {
        let mut log = ActivityLog::with_max_entries(3);
        for i in 0..5 {
            log.record(
                "tester",
                "read_file",
                ApprovalTier::Auto,
                GateDecision::Allow,
                &format!("ok {i}"),
            );
        }
        assert_eq!(log.len(), 3);
        // Oldest two (seq 1, 2) evicted; newest three kept in order.
        assert_eq!(
            log.entries().iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert_eq!(
            log.entries()
                .iter()
                .map(|e| e.reason.clone())
                .collect::<Vec<_>>(),
            vec!["ok 2", "ok 3", "ok 4"]
        );
    }

    #[test]
    fn activity_log_seq_never_reused_after_eviction() {
        let mut log = ActivityLog::with_max_entries(3);
        for i in 0..3 {
            log.record(
                "tester",
                "read_file",
                ApprovalTier::Auto,
                GateDecision::Allow,
                &format!("ok {i}"),
            );
        }
        log.record(
            "tester",
            "read_file",
            ApprovalTier::Auto,
            GateDecision::Allow,
            "ok 3",
        );
        log.record(
            "tester",
            "read_file",
            ApprovalTier::Auto,
            GateDecision::Allow,
            "ok 4",
        );
        assert_eq!(
            log.entries().iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        // The next entry continues at 6, not back at 1: the seq gap
        // (1-2 gone) is what makes eviction observable instead of
        // silent.
        let next = log.record(
            "tester",
            "read_file",
            ApprovalTier::Auto,
            GateDecision::Allow,
            "ok 5",
        );
        assert_eq!(next.seq, 6);
    }

    #[test]
    fn policy_validation_rejects_blank_entries() {
        let mut policy = ToolPolicy::new();
        policy.allow.insert("".to_string());
        assert!(!policy.validate().is_empty());
        let policy = policy_allowing(&["read_file"]);
        assert!(policy.validate().is_empty());
    }
}
