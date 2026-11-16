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

/// One explicit approval: granted by a person, for one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub tool_name: String,
    pub idempotency_key: String,
    pub granted_by: String,
    pub granted_at: String,
}

/// In-memory approval grants. A grant matches one tool + one key.
#[derive(Debug, Clone, Default)]
pub struct ApprovalStore {
    grants: HashMap<(String, String), Approval>,
}

fn epoch_seconds_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch-{}", secs)
}

impl ApprovalStore {
    pub fn new() -> ApprovalStore {
        ApprovalStore {
            grants: HashMap::new(),
        }
    }

    pub fn grant(&mut self, tool_name: &str, idempotency_key: &str, granted_by: &str) -> Approval {
        let approval = Approval {
            tool_name: tool_name.to_string(),
            idempotency_key: idempotency_key.to_string(),
            granted_by: granted_by.to_string(),
            granted_at: epoch_seconds_now(),
        };
        self.grants.insert(
            (tool_name.to_string(), idempotency_key.to_string()),
            approval.clone(),
        );
        approval
    }

    pub fn find(&self, tool_name: &str, idempotency_key: &str) -> Option<Approval> {
        self.grants
            .get(&(tool_name.to_string(), idempotency_key.to_string()))
            .cloned()
    }

    /// Revoking a grant removes the grant, never a log entry.
    pub fn revoke(&mut self, tool_name: &str, idempotency_key: &str) -> bool {
        self.grants
            .remove(&(tool_name.to_string(), idempotency_key.to_string()))
            .is_some()
    }

    pub fn list(&self) -> Vec<Approval> {
        let mut approvals: Vec<Approval> = self.grants.values().cloned().collect();
        approvals.sort_by(|a, b| {
            a.tool_name
                .cmp(&b.tool_name)
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

/// Append-only activity log. There is deliberately no delete, no
/// update, and no clear: entries only ever get appended, seq never
/// reuses a number, and readers get clones.
#[derive(Debug, Clone, Default)]
pub struct ActivityLog {
    entries: Vec<ActivityEntry>,
    next_seq: u64,
}

impl ActivityLog {
    pub fn new() -> ActivityLog {
        ActivityLog {
            entries: Vec::new(),
            next_seq: 1,
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
/// ask-first tools. No side effect, no logging: use
/// gate_tool_call for the evaluated + recorded path.
pub fn evaluate_tool_call(
    policy: &ToolPolicy,
    approvals: &ApprovalStore,
    tool: &str,
    idempotency_key: &str,
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
        match approvals.find(tool, idempotency_key) {
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
pub fn gate_tool_call(
    policy: &ToolPolicy,
    approvals: &ApprovalStore,
    log: &mut ActivityLog,
    actor: &str,
    tool: &str,
    idempotency_key: &str,
) -> GateEvaluation {
    let evaluation = evaluate_tool_call(policy, approvals, tool, idempotency_key);
    log.record(
        actor,
        tool,
        evaluation.tier,
        evaluation.decision,
        &evaluation.reason,
    );
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
        let approvals = ApprovalStore::new();
        for tool in ["read_file", "send_message", "trade_securities"] {
            let eval = evaluate_tool_call(&policy, &approvals, tool, "k1");
            assert_eq!(eval.decision, GateDecision::Deny, "{tool}");
        }
    }

    #[test]
    fn auto_tool_allowed_only_with_an_explicit_rule() {
        let approvals = ApprovalStore::new();
        let policy = policy_allowing(&["read_file"]);
        let eval = evaluate_tool_call(&policy, &approvals, "read_file", "k1");
        assert_eq!(eval.tier, ApprovalTier::Auto);
        assert_eq!(eval.decision, GateDecision::Allow);
        assert!(eval.reason.contains("auto-tier and explicitly allowed"));
    }

    #[test]
    fn ask_first_blocked_without_approval_allowed_with_it() {
        let policy = policy_allowing(&["send_message"]);
        let mut approvals = ApprovalStore::new();
        let eval = evaluate_tool_call(&policy, &approvals, "send_message", "k1");
        assert_eq!(eval.decision, GateDecision::AskFirst);
        approvals.grant("send_message", "k1", "zen");
        let eval = evaluate_tool_call(&policy, &approvals, "send_message", "k1");
        assert_eq!(eval.decision, GateDecision::Allow);
        assert!(eval.reason.contains("approved by zen"));
        // A different key is a different call: still waiting.
        let eval = evaluate_tool_call(&policy, &approvals, "send_message", "k2");
        assert_eq!(eval.decision, GateDecision::AskFirst);
    }

    #[test]
    fn ask_first_approval_without_a_policy_rule_still_denies() {
        let policy = ToolPolicy::new();
        let mut approvals = ApprovalStore::new();
        approvals.grant("delete_file", "k1", "zen");
        let eval = evaluate_tool_call(&policy, &approvals, "delete_file", "k1");
        assert_eq!(eval.decision, GateDecision::Deny);
        assert!(eval.reason.contains("default-deny"));
    }

    #[test]
    fn never_tier_denied_even_with_rule_and_approval() {
        let policy =
            policy_allowing(&["trade_securities", "transfer_money", "access_other_account"]);
        let mut approvals = ApprovalStore::new();
        for tool in ["trade_securities", "transfer_money", "access_other_account"] {
            approvals.grant(tool, "k1", "zen");
            let eval = evaluate_tool_call(&policy, &approvals, tool, "k1");
            assert_eq!(eval.tier, ApprovalTier::Never, "{tool}");
            assert_eq!(eval.decision, GateDecision::Deny, "{tool}");
            assert!(eval.reason.contains("no approval path"), "{tool}");
        }
    }

    #[test]
    fn approval_store_grant_find_revoke_list() {
        let mut store = ApprovalStore::new();
        assert_eq!(store.find("send_message", "k1"), None);
        let approval = store.grant("send_message", "k1", "zen");
        assert_eq!(approval.tool_name, "send_message");
        assert_eq!(approval.granted_by, "zen");
        assert!(!approval.granted_at.is_empty());
        assert_eq!(store.find("send_message", "k1"), Some(approval.clone()));
        assert_eq!(store.find("send_message", "k2"), None);
        assert_eq!(store.list().len(), 1);
        assert!(store.revoke("send_message", "k1"));
        assert!(!store.revoke("send_message", "k1"));
        assert_eq!(store.find("send_message", "k1"), None);
        assert!(store.list().is_empty());
    }

    #[test]
    fn activity_log_records_every_decision_with_monotonic_seq() {
        let mut log = ActivityLog::new();
        assert!(log.is_empty());
        let policy = policy_allowing(&["read_file", "send_message"]);
        let approvals = ApprovalStore::new();
        let a = gate_tool_call(&policy, &approvals, &mut log, "tester", "read_file", "k1");
        assert_eq!(a.decision, GateDecision::Allow);
        let b = gate_tool_call(
            &policy,
            &approvals,
            &mut log,
            "tester",
            "send_message",
            "k1",
        );
        assert_eq!(b.decision, GateDecision::AskFirst);
        let c = gate_tool_call(
            &policy,
            &approvals,
            &mut log,
            "tester",
            "trade_securities",
            "k1",
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

    #[test]
    fn policy_validation_rejects_blank_entries() {
        let mut policy = ToolPolicy::new();
        policy.allow.insert("".to_string());
        assert!(!policy.validate().is_empty());
        let policy = policy_allowing(&["read_file"]);
        assert!(policy.validate().is_empty());
    }
}
