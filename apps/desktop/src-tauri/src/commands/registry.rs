// Backend session registry for the Tauri command handlers (CLI bridge
// round). PURE RUST, no Tauri dependency: this module compiles and tests
// standalone with `rustc --test`, so the handler logic has real test
// evidence even on a box without the Tauri system libraries (gdk-3.0,
// webkit2gtk). The thin #[tauri::command] wrappers in session.rs, tools.rs
// and events.rs only convert wire args and delegate here; they hold no
// logic of their own.
//
// Design, per prep B-2 section 2 and the decided point-854 statechart:
// - The backend owns the command gate and the session record. The model
//   runs in the backend through the CLI core + stream-json bridge
//   (Phase 2, tin 9530): each session owns one CliBridge handle, and
//   steer/follow_up run the `zen` CLI for that session. The backend never
//   calls the Pi SDK and never holds a key: the SDK is out of the webview
//   entirely (tin 9328/9459).
// - Session state transitions mirror packages/core/src/session.ts exactly:
//   Running -> AwaitingApproval | Error | Completed | Cancelled | Failed,
//   AwaitingApproval -> Running | Cancelled (rejection never hangs),
//   Error -> Running | Failed | Cancelled (P2-9: the third path is
//   the abort of an errored session — without it Error is a trap:
//   abort refuses and retry is the only exit, so a user who wants
//   to close an errored session cannot),
//   terminal states (Completed, Cancelled, Failed) have no outgoing path.
// - A turn's outcome moves the state (P2-9): a successful turn leaves
//   the session Running — one live session serves every chat turn, so
//   Completed after a turn would terminal the session after the first
//   message; an error turn moves Running -> Error, and a second
//   consecutive error turn moves Error -> Failed (terminal). A
//   successful turn from Error recovers to Running. A turn that
//   fails before any result record (no result, timeout, validation)
//   leaves the state untouched: the session stays usable and the
//   retry still runs.
// - Every command validates the full IPC action contract first (same
//   required fields as apps/desktop/src/ipc/types.ts). A missing field
//   means FAIL, never a silent default.
// - Tool execution is NOT filled in here: the contract is validated,
//   the 6-step gate order is asserted, and the P0-2 approval gate
//   decides (auto / ask-first / never + default-deny policy), then
//   the call fails loudly with TOOL_EXECUTION_NOT_WIRED when the gate
//   passes, because the gate execute step itself is a later phase.
//   A gate refusal (APPROVAL_REQUIRED / APPROVAL_NEVER_DENIED /
//   TOOL_POLICY_DENIED) fails before any execution is attempted.
//   Every evaluation is recorded in the append-only activity log,
//   including refusals.
//
// Locking, read before touching steer: a turn runs for seconds while the
// Registry mutex is held by the caller, so steer is split in three —
// begin_steer (validate + hand out the session's bridge), the turn itself
// (runs with NO registry lock held, so abort can reach the child), and
// complete_steer (record the turn, only after the model ran). A wrapper
// that held the registry lock across the whole turn would deadlock abort:
// abort needs the same lock to reach the bridge.
//
// Abort is split the same way (R2B-07): begin_abort (validate the
// transition + hand out the session's bridge, no state change), the
// bridge.abort() escalation (runs with NO registry lock held — it polls
// up to the 5s grace when the child ignores SIGINT, and holding the
// lock across it blocked every other Tauri command), and complete_abort
// (re-check the transition, then Cancelled + log). The one-call abort()
// below is test-only for the same reason steer() is: production code
// must use the three steps, never hold the lock across bridge.abort().
//
// Idempotency (BUG-10, #25): the idempotency key is enforced, not just
// checked for presence. begin_steer returns Duplicate when the same
// (idempotency key, contract id, kind, text) already completed on this
// session, and the caller returns the recorded turn result WITHOUT
// running the model again: no new turn, no new log entry, no new events.
// Only a turn that ran to a result is recorded (complete_steer); a failed
// turn records nothing, so a retry after a failure still executes.
// Concurrent duplicate (BUG-10 extra, tin 9645/9650): the same steer
// arriving WHILE the first one still runs is refused too (InFlight):
// begin_steer records the key as in-flight the moment the turn begins,
// so a second identical steer sees it before any model call starts.
// There is no recorded result to replay yet, so the refusal is a
// distinct InFlight answer, not a faked Duplicate turn.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::approval_gate::{
    gate_tool_call, ActivityEntry, ActivityLog, Approval, ApprovalStore, GateDecision, ToolPolicy,
};
use super::cli_settings::{CliSettings, DEFAULT_MODEL};
use super::cli_spawn::{AbortOutcome, BridgeError, CliBridge, TurnResult};
use super::model_router::{next_route_in, resolve_attempt_order, Route, RouterConfig};
use super::stream_json::SenEvent;

// --- Session statechart (point 854, mirror of packages/core) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Running,
    AwaitingApproval,
    Error,
    Completed,
    Cancelled,
    Failed,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Running => "Running",
            SessionState::AwaitingApproval => "AwaitingApproval",
            SessionState::Error => "Error",
            SessionState::Completed => "Completed",
            SessionState::Cancelled => "Cancelled",
            SessionState::Failed => "Failed",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SessionState::Completed | SessionState::Cancelled | SessionState::Failed
        )
    }
}

pub fn can_transition(from: SessionState, to: SessionState) -> bool {
    use SessionState::*;
    match from {
        Running => matches!(
            to,
            AwaitingApproval | Error | Completed | Cancelled | Failed
        ),
        AwaitingApproval => matches!(to, Running | Cancelled),
        Error => matches!(to, Running | Failed | Cancelled),
        Completed | Cancelled | Failed => false,
    }
}

// --- Command errors: typed codes, never swallowed, never faked ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new(code: &str, message: &str) -> CommandError {
        CommandError {
            code: code.to_string(),
            message: message.to_string(),
        }
    }

    /// Wire form "CODE: message", parseable by the frontend, which surfaces
    /// it as-is instead of mapping it to a fake success.
    pub fn to_wire(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }
}

pub const ERR_UNKNOWN_SESSION: &str = "UNKNOWN_SESSION";
pub const ERR_DUPLICATE_SESSION: &str = "DUPLICATE_SESSION";
pub const ERR_DUPLICATE_STEER: &str = "DUPLICATE_STEER";
pub const ERR_ILLEGAL_TRANSITION: &str = "ILLEGAL_TRANSITION";
pub const ERR_INVALID_CONTRACT: &str = "INVALID_CONTRACT";
pub const ERR_TOOL_EXECUTION_NOT_WIRED: &str = "TOOL_EXECUTION_NOT_WIRED";
pub const ERR_APPROVAL_REQUIRED: &str = "APPROVAL_REQUIRED";
pub const ERR_APPROVAL_NEVER_DENIED: &str = "APPROVAL_NEVER_DENIED";
pub const ERR_TOOL_POLICY_DENIED: &str = "TOOL_POLICY_DENIED";

// --- IPC action contract (mirror of apps/desktop/src/ipc/types.ts) ---

#[derive(Debug, Clone)]
pub struct ActionContract {
    pub id: String,
    pub capability: String,
    pub capability_version: String,
    pub actor: String,
    pub purpose: String,
    pub phase: String,
    pub tool_kind: String,
    pub side_effect: String,
    pub risk: String,
    pub idempotency_key: String,
    pub timeout_ms: u64,
    pub approval_required: bool,
    pub audit_required: bool,
}

pub fn validate_contract(contract: &ActionContract) -> Result<(), CommandError> {
    let missing: Vec<&str> = [
        ("id", contract.id.is_empty()),
        ("capability", contract.capability.is_empty()),
        ("capability_version", contract.capability_version.is_empty()),
        ("actor", contract.actor.is_empty()),
        ("purpose", contract.purpose.is_empty()),
        ("phase", contract.phase.is_empty()),
        ("risk", contract.risk.is_empty()),
        ("idempotency_key", contract.idempotency_key.is_empty()),
    ]
    .iter()
    .filter(|(_, empty)| *empty)
    .map(|(name, _)| *name)
    .collect();
    if !missing.is_empty() {
        return Err(CommandError::new(
            ERR_INVALID_CONTRACT,
            &format!("IPC contract missing fields, FAIL: {}", missing.join(", ")),
        ));
    }
    if contract.timeout_ms == 0 {
        return Err(CommandError::new(
            ERR_INVALID_CONTRACT,
            "IPC contract timeout_ms must be positive, FAIL",
        ));
    }
    match contract.tool_kind.as_str() {
        "model-tool" | "registered-tool" | "authorized-execution-tool" => {}
        _ => {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                "IPC contract illegal tool kind, FAIL",
            ));
        }
    }
    match contract.side_effect.as_str() {
        "no-side-effect" | "reversible" | "irreversible" => {}
        _ => {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                "IPC contract illegal side effect class, FAIL",
            ));
        }
    }
    Ok(())
}

// --- Execution gate order (mirror of packages/core/src/execution-gate.ts) ---

pub const GATE_ORDER: [&str; 6] = [
    "protect",
    "lock-check",
    "irreversible-and-consent-check",
    "execute",
    "post-check",
    "record",
];

pub fn assert_gate_order(steps: &[&str]) -> Result<(), CommandError> {
    if steps.len() != GATE_ORDER.len() {
        return Err(CommandError::new(
            ERR_INVALID_CONTRACT,
            "Execution gate missing step, FAIL",
        ));
    }
    for (i, step) in steps.iter().enumerate() {
        if *step != GATE_ORDER[i] {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                &format!("Execution gate wrong order, FAIL at step {}", i),
            ));
        }
    }
    Ok(())
}

// --- Session record and registry ---

/// Retention caps (P2-5): the registry never evicted, so sessions,
/// completed steers, and the activity log grew with runtime. Eviction
/// is FIFO under a fixed cap (INFO-1 precedent: eviction under a cap
/// is NOT deletion — there is still no delete API, no caller can
/// delete anything, and eviction happens only inside the insert path,
/// only to the oldest entry, only when the cap is exceeded). Caps sit
/// an order of magnitude above ordinary use: a desktop assistant holds
/// a handful of live sessions, and a session replays at most its recent
/// steers, so no useful state of a normal session is lost. The
/// activity log has its own cap, MAX_ACTIVITY_LOG_ENTRIES in
/// approval_gate.rs (10,000, same as RawLog and the TS ActivityLog).
pub const MAX_SESSIONS: usize = 100;
pub const MAX_COMPLETED_STEERS_PER_SESSION: usize = 100;

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub state: SessionState,
    pub log: Vec<String>,
    pub subscribers: u32,
    /// Active model-router route id for this session (P0-3). Changes
    /// when a turn falls back to the next route.
    pub route_id: String,
}

#[derive(Default)]
pub struct Registry {
    sessions: HashMap<String, SessionRecord>,
    /// Creation order of session ids (P2-5): FIFO eviction drops the
    /// oldest session first when the registry holds more than
    /// MAX_SESSIONS. Kept in step with `sessions`: every create
    /// pushes, every eviction pops.
    session_order: VecDeque<String>,
    /// One CLI bridge handle per session (Phase 2). The handle is
    /// created with the session and outlives single turns; the process
    /// itself is per-turn (see cli_spawn.rs for why no process stays
    /// alive between turns).
    bridges: HashMap<String, Arc<CliBridge>>,
    /// Completed steers per session, for idempotency (BUG-10, #25):
    /// session id -> (steer key -> recorded turn result). Only turns
    /// that ran to a result land here, via complete_steer.
    completed_steers: HashMap<String, HashMap<SteerKey, TurnResult>>,
    /// Insertion order of completed-steer keys per session (P2-5):
    /// FIFO eviction drops the oldest recorded steer first when a
    /// session holds more than MAX_COMPLETED_STEERS_PER_SESSION.
    /// Kept in step with `completed_steers`: a key is pushed only
    /// the first time it is recorded.
    completed_steer_order: HashMap<String, VecDeque<SteerKey>>,
    /// Steers currently running per session (BUG-10 extra, tin
    /// 9645/9650): session id -> keys whose turn began but has not
    /// finished yet. begin_steer inserts the key the moment the turn
    /// begins; complete_steer removes it before recording the
    /// completed turn, and release_steer removes it when the turn ends
    /// without a result. A key is never in both sets at once: the move
    /// happens under one &mut borrow, and the registry lock is never
    /// held while the turn runs (see the Locking note above), so this
    /// cannot deadlock abort.
    in_flight_steers: HashMap<String, HashSet<SteerKey>>,
    /// App config dir holding cli-settings.json (None in tests and when
    /// the app config dir is unavailable: the default model applies).
    settings_dir: Option<PathBuf>,
    /// Explicit binary path (tests / operator override). None means the
    /// bridge resolves per decision 1 at first turn (ZEN_BIN, then PATH).
    zen_bin_override: Option<PathBuf>,
    /// P0-2 approval gate state: the default-deny tool policy
    /// (empty allow-list denies everything), the granted approvals,
    /// and the append-only activity log. The policy starts empty on
    /// purpose: no tool runs until a rule allows it by name.
    tool_policy: ToolPolicy,
    approvals: ApprovalStore,
    activity_log: ActivityLog,
    /// Test-only abort grace for bridges created after the call (R2B-07):
    /// shortens the SIGKILL escalation in tests so the lock-free abort
    /// can be proven without waiting the full 5s production grace.
    #[cfg(test)]
    abort_grace_override: Option<Duration>,
}

/// The identity of one steer for idempotency (BUG-10, #25). A duplicate
/// is the SAME idempotency key AND the SAME contract AND the SAME text:
/// a different key re-executes (a new request), a different contract id
/// re-executes (a different action that happens to reuse a key), and a
/// different text re-executes (a different request body). Kind is part
/// of the key because steer and follow_up are different operations even
/// when they share one contract (the app reuses one contract for the
/// whole live session, see apps/desktop/src/app/main.ts). The map is
/// per-session, so the same key on another session is independent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SteerKey {
    pub idempotency_key: String,
    pub contract_id: String,
    pub kind: String,
    pub text: String,
}

impl SteerKey {
    pub fn new(contract: &ActionContract, kind: &str, text: &str) -> SteerKey {
        SteerKey {
            idempotency_key: contract.idempotency_key.clone(),
            contract_id: contract.id.clone(),
            kind: kind.to_string(),
            text: text.to_string(),
        }
    }
}

/// What begin_steer hands back (BUG-10, #25). Run means the turn must
/// still execute on the bridge; Duplicate means this exact steer already
/// completed on this session, and the caller returns the recorded turn
/// result without running the model or pushing a log entry, replaying
/// the cached result event (CI P2-2) so a caller waiting for it
/// resolves instead of timing out. InFlight (BUG-10 extra, tin
/// 9645/9650) means the same steer
/// is ALREADY RUNNING on this session right now: there is no recorded
/// result to replay yet, so the caller refuses it outright
/// (DUPLICATE_STEER) instead of starting a second model call for the
/// same request. It is a separate variant from Duplicate on purpose:
/// replaying would need a TurnResult that does not exist yet, and
/// inventing one would fake a result.
#[derive(Debug, Clone)]
pub enum SteerStart {
    Run(Arc<CliBridge>),
    Duplicate {
        state: SessionState,
        turn: TurnResult,
    },
    InFlight {
        state: SessionState,
    },
}

fn bridge_error_to_command(error: BridgeError) -> CommandError {
    CommandError::new(&error.code, &error.message)
}

impl Registry {
    pub fn new() -> Registry {
        Registry {
            sessions: HashMap::new(),
            session_order: VecDeque::new(),
            bridges: HashMap::new(),
            completed_steers: HashMap::new(),
            completed_steer_order: HashMap::new(),
            in_flight_steers: HashMap::new(),
            settings_dir: None,
            zen_bin_override: None,
            #[cfg(test)]
            abort_grace_override: None,
            tool_policy: ToolPolicy::new(),
            approvals: ApprovalStore::new(),
            activity_log: ActivityLog::new(),
        }
    }

    pub fn with_settings_dir(dir: PathBuf) -> Registry {
        let mut registry = Registry::new();
        registry.settings_dir = Some(dir);
        registry
    }

    pub fn set_settings_dir(&mut self, dir: PathBuf) {
        self.settings_dir = Some(dir);
    }

    pub fn set_zen_bin(&mut self, path: PathBuf) {
        self.zen_bin_override = Some(path);
    }

    /// Test-only (R2B-07): the abort grace for bridges created after
    /// this call. Production always uses the 5s ABORT_GRACE in
    /// cli_spawn.rs; tests shorten it so the SIGKILL escalation can be
    /// proven without waiting the full grace.
    #[cfg(test)]
    pub fn set_abort_grace(&mut self, grace: Duration) {
        self.abort_grace_override = Some(grace);
    }

    /// Build the session's bridge, applying the test-only abort grace
    /// when one is set (R2B-07). Production always takes the default.
    /// `base_url` is the stored provider base URL (R2B-05): passed
    /// through to CliBridge::new so a new session keeps using the
    /// saved base URL.
    fn new_bridge(&self, session_id: &str, model: &str, base_url: Option<&str>) -> CliBridge {
        let bridge = CliBridge::new(self.zen_bin_override.clone(), session_id, model, base_url);
        #[cfg(test)]
        {
            if let Some(grace) = self.abort_grace_override {
                return bridge.with_abort_grace(grace);
            }
        }
        bridge
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionRecord> {
        self.sessions.get(session_id)
    }

    /// The session's CLI bridge handle, if the session exists.
    pub fn bridge(&self, session_id: &str) -> Option<Arc<CliBridge>> {
        self.bridges.get(session_id).cloned()
    }

    /// Settings for a new session: the stored settings when a
    /// settings dir is set, else the defaults (CLI's free default
    /// model, no base URL). Settings never fail session creation (a
    /// bad file falls back to the defaults).
    fn settings_for_new_session(&self) -> CliSettings {
        match &self.settings_dir {
            Some(dir) => CliSettings::load(dir),
            None => CliSettings::default(),
        }
    }

    fn get_mut_or_unknown(&mut self, session_id: &str) -> Result<&mut SessionRecord, CommandError> {
        self.sessions.get_mut(session_id).ok_or_else(|| {
            CommandError::new(
                ERR_UNKNOWN_SESSION,
                &format!("Session does not exist: {}", session_id),
            )
        })
    }

    /// sen_session_create: backend record only, state Running, plus one
    /// CLI bridge handle for the session. P0-3: the bridge runs the
    /// router's active route (model and base URL from the resolved
    /// route; legacy stored model/base_url when no router is stored).
    /// No model call happens here: the handle spawns nothing until
    /// the first steer. P2-5: creating past MAX_SESSIONS evicts the
    /// oldest session (FIFO, with everything it owns) so the
    /// registry stays bounded over a long runtime.
    pub fn create(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<SessionState, CommandError> {
        validate_contract(contract)?;
        if session_id.is_empty() {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                "session_id must be non-empty, FAIL",
            ));
        }
        if self.sessions.contains_key(session_id) {
            return Err(CommandError::new(
                ERR_DUPLICATE_SESSION,
                &format!("Session already exists: {}", session_id),
            ));
        }
        let settings = self.settings_for_new_session();
        let route = settings.active_route();
        self.sessions.insert(
            session_id.to_string(),
            SessionRecord {
                id: session_id.to_string(),
                state: SessionState::Running,
                log: vec!["session_create".to_string()],
                subscribers: 0,
                route_id: route.id.clone(),
            },
        );
        self.bridges.insert(
            session_id.to_string(),
            Arc::new(self.new_bridge(session_id, &route.model, route.base_url.as_deref())),
        );
        // P2-5: remember creation order, then evict the oldest
        // sessions while the registry holds more than MAX_SESSIONS.
        self.session_order.push_back(session_id.to_string());
        self.evict_oldest_sessions();
        Ok(SessionState::Running)
    }

    /// Drop the oldest sessions while the registry holds more than
    /// MAX_SESSIONS (P2-5). FIFO: the session created first goes
    /// first. Everything the session owns goes with it — bridge,
    /// completed steers, their order queue, and in-flight keys — so
    /// no map keeps growing behind the sessions map. A later call
    /// for an evicted session fails loudly with UNKNOWN_SESSION; a
    /// turn still running on one fails loudly at complete_steer,
    /// never silently. This is retention eviction under a cap, not
    /// a delete API (INFO-1): no caller can evict anything on
    /// demand, and there is still no close/delete session command.
    fn evict_oldest_sessions(&mut self) {
        while self.session_order.len() > MAX_SESSIONS {
            let Some(oldest) = self.session_order.pop_front() else {
                break;
            };
            self.sessions.remove(&oldest);
            self.bridges.remove(&oldest);
            self.completed_steers.remove(&oldest);
            self.completed_steer_order.remove(&oldest);
            self.in_flight_steers.remove(&oldest);
        }
    }

    /// Drop the oldest recorded steers of one session while it holds
    /// more than MAX_COMPLETED_STEERS_PER_SESSION (P2-5). FIFO: the
    /// steer recorded first goes first. A later retry of an evicted
    /// key runs the model again instead of replaying — the result
    /// was already delivered, so nothing is lost, only the replay
    /// shortcut for that old key.
    fn evict_oldest_completed_steers(&mut self, session_id: &str) {
        let Some(order) = self.completed_steer_order.get_mut(session_id) else {
            return;
        };
        while order.len() > MAX_COMPLETED_STEERS_PER_SESSION {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            if let Some(done) = self.completed_steers.get_mut(session_id) {
                done.remove(&oldest);
            }
        }
    }

    /// Test-only: how many sessions the registry holds (P2-5 cap).
    #[cfg(test)]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Test-only: how many completed steers one session holds (P2-5 cap).
    #[cfg(test)]
    pub fn completed_steer_count(&self, session_id: &str) -> usize {
        self.completed_steers
            .get(session_id)
            .map(|done| done.len())
            .unwrap_or(0)
    }

    /// The session's active route id (P0-3), for the session result
    /// the frontend shows. None when the session does not exist.
    pub fn route_id(&self, session_id: &str) -> Option<String> {
        self.sessions.get(session_id).map(|r| r.route_id.clone())
    }

    /// The router config behind this registry's settings (P0-3):
    /// the stored router when a settings dir is set, else the
    /// default chain. The caller never gets a paid route here:
    /// validation already rejected paid routes at load/store time.
    pub fn router_config(&self) -> RouterConfig {
        match &self.settings_dir {
            Some(dir) => CliSettings::load(dir).router.unwrap_or_default(),
            None => RouterConfig::default(),
        }
    }

    /// The attempt order for this registry's router (P0-3), first
    /// route first. Used by the production wrapper and the test-only
    /// steer() so both retry the same chain in the same order.
    pub fn attempt_order(&self) -> Vec<Route> {
        resolve_attempt_order(&self.router_config())
    }

    /// Swap a session's bridge to the next route after `failed_route_id`
    /// (P0-3 fallback). Returns the new bridge and route, or None when
    /// the chain is exhausted or fallback is disabled. `order` is the
    /// attempt order the turn snapshotted at its start (P2-2): this
    /// function never re-reads the router config, so a config change
    /// mid-turn cannot move routes under the loop, and it never
    /// returns a route already tried — the next route always comes
    /// after `failed_route_id` in that snapshot, or there is none.
    /// Fallback disabled needs no separate check here: a disabled turn
    /// snapshots a one-route order, so there is no next route. When the failed
    /// bridge already ran a turn to a result, the new bridge resumes
    /// the same CLI session with -r, so the turn keeps its history;
    /// when the failed bridge never started (e.g. auth failed on the
    /// first turn), the new bridge starts a fresh CLI session and the
    /// turn runs on the prompt alone (+ memory block). The session
    /// record's route_id moves to the new route either way, so the
    /// frontend always shows the route that actually ran.
    pub fn fallback_bridge(
        &mut self,
        session_id: &str,
        failed_route_id: &str,
        order: &[Route],
    ) -> Option<(Arc<CliBridge>, Route)> {
        let next = next_route_in(order, failed_route_id)?;
        let resume = self
            .bridges
            .get(session_id)
            .map(|b| b.has_started())
            .unwrap_or(false);
        let bridge = self.new_bridge(session_id, &next.model, next.base_url.as_deref());
        if resume {
            bridge.mark_session_started();
        }
        let bridge = Arc::new(bridge);
        self.bridges
            .insert(session_id.to_string(), Arc::clone(&bridge));
        // The session record's route_id moves to the new route, so
        // the frontend always shows the route that actually ran. No
        // log entry is pushed here on purpose (BUG-06): a steer that
        // never runs to a result must leave no log entry, and a
        // fallback swap is not a completed steer — the successful
        // turn logs its own steer entry in complete_steer.
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.route_id = next.id.clone();
        }
        Some((bridge, next))
    }

    /// sen_session_steer / sen_session_follow_up, step 1 of 3: validate
    /// the contract, the text, and the steer gate (Running, or Error:
    /// a session whose last turn errored may retry — the Error ->
    /// Running recovery path, P2-9), then either hand
    /// out the session's bridge (Run) or, when this exact steer already
    /// completed on the session, return the recorded turn result
    /// (Duplicate) so the caller returns it without running the model
    /// again (BUG-10, #25). When the same steer is still running on the
    /// session, return InFlight instead (BUG-10 extra, tin 9645/9650):
    /// the caller refuses it, it never starts a second model call. The
    /// caller runs the turn on the bridge WITHOUT holding the registry
    /// lock, then calls complete_steer (or release_steer when the turn
    /// fails, so the key never leaks).
    /// `kind` is "steer" or "follow_up".
    pub fn begin_steer(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
        kind: &str,
        text: &str,
    ) -> Result<SteerStart, CommandError> {
        validate_contract(contract)?;
        if kind != "steer" && kind != "follow_up" {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                "steer kind must be steer or follow_up, FAIL",
            ));
        }
        if text.trim().is_empty() {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                "steer/follow-up text must be non-empty, FAIL",
            ));
        }
        let record = self.get_mut_or_unknown(session_id)?;
        if record.state != SessionState::Running && record.state != SessionState::Error {
            return Err(CommandError::new(
                ERR_ILLEGAL_TRANSITION,
                &format!(
                    "Cannot steer a session in state {}, only Running or Error accepts steer/follow-up",
                    record.state.as_str()
                ),
            ));
        }
        let state = record.state;
        let key = SteerKey::new(contract, kind, text);
        // Idempotency (BUG-10): same key + same contract + same text +
        // same kind already completed -> replay the recorded result,
        // never run the model again.
        if let Some(prior) = self
            .completed_steers
            .get(session_id)
            .and_then(|done| done.get(&key))
        {
            return Ok(SteerStart::Duplicate {
                state,
                turn: prior.clone(),
            });
        }
        // Concurrent duplicate (BUG-10 extra, tin 9645/9650): the same
        // steer is already running on this session — refuse it. The key
        // was recorded as in-flight when the first steer began (below),
        // so a second identical steer arriving while the first turn
        // still runs sees it here instead of starting a second model
        // call for the same request.
        if self
            .in_flight_steers
            .get(session_id)
            .map(|running| running.contains(&key))
            .unwrap_or(false)
        {
            return Ok(SteerStart::InFlight { state });
        }
        let bridge = self.bridges.get(session_id).cloned().ok_or_else(|| {
            CommandError::new(
                ERR_UNKNOWN_SESSION,
                &format!("Session has no CLI bridge: {}", session_id),
            )
        })?;
        // Record the steer as in-flight NOW, while the registry lock is
        // held and before the bridge is handed out: the caller runs the
        // turn with NO lock held, so any identical steer that begins
        // meanwhile is refused above. The bridge lookup ran first on
        // purpose, so a missing bridge cannot leave a key behind.
        self.in_flight_steers
            .entry(session_id.to_string())
            .or_default()
            .insert(key);
        Ok(SteerStart::Run(bridge))
    }

    /// sen_session_steer / sen_session_follow_up, step 3 of 3: record the
    /// turn AFTER the model ran. The log entry is written only here, so
    /// the backend never records a steer that did not run a model — the
    /// BUG-06 lesson. A turn that ran to a SUCCESS result is also stored
    /// under its steer key, so a later retry of the same key + contract
    /// + text replays this result instead of running again (BUG-10,
    /// #25). A turn whose result record carries is_error=true (R2B-02)
    /// is NOT stored: the model call failed (e.g. HTTP 401), so there is
    /// no completed result to replay, and a retry with the same key must
    /// run the model again. The error turn is still logged (its subtype
    /// marks it, e.g. steer:steer:error_during_execution) and this still
    /// returns Ok(state): the backend ack means the result record was
    /// received, and the error itself is visible in the event stream.
    /// The in-flight key is dropped FIRST, before anything else: on the
    /// success path the key moves from in-flight to completed below; on
    /// the refusal path (the session left Running, abort won the race)
    /// the key must still not leak, so a later steer is never refused
    /// for a turn that already ended (BUG-10 extra, tin 9645/9650). If
    /// the session left Running or Error meanwhile (abort won the
    /// race), the turn is NOT recorded and this refuses.
    // P2-9: the turn's outcome moves the state. A successful turn
    // leaves the session Running: one live session serves every
    // chat turn, so a turn that succeeds does not end the session
    // (there is no finish/close command that would produce
    // Completed — a session ends via abort, to Cancelled). An error
    // turn (is_error=true) moves Running -> Error; a second
    // consecutive error turn moves Error -> Failed (terminal), so
    // a session that keeps failing is failed, not left Running
    // forever. A successful turn from Error recovers to Running.
    pub fn complete_steer(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
        kind: &str,
        text: &str,
        turn: &TurnResult,
    ) -> Result<SessionState, CommandError> {
        if let Some(running) = self.in_flight_steers.get_mut(session_id) {
            running.remove(&SteerKey::new(contract, kind, text));
        }
        let state = {
            let record = self.get_mut_or_unknown(session_id)?;
            if record.state != SessionState::Running && record.state != SessionState::Error {
                return Err(CommandError::new(
                    ERR_ILLEGAL_TRANSITION,
                    &format!(
                        "Session is {} after the turn, steer not recorded",
                        record.state.as_str()
                    ),
                ));
            }
            record.log.push(format!("steer:{}:{}", kind, turn.subtype));
            // P2-9: move the state by the turn's outcome (see the doc
            // above). A successful turn always lands on Running, from
            // either entry state; an error turn steps one failure
            // further (Running -> Error, Error -> Failed).
            let next = if turn.is_error {
                match record.state {
                    SessionState::Error => SessionState::Failed,
                    _ => SessionState::Error,
                }
            } else {
                SessionState::Running
            };
            record.state = next;
            record.state
        };
        // R2B-02: an error turn (is_error=true) is not a completed
        // steer — do not cache it, so a retry with the same key runs
        // the model again instead of replaying the error as a
        // Duplicate. The log entry above already records that the
        // turn ran and what its subtype was.
        if !turn.is_error {
            // P2-5: remember insertion order (a key is pushed only
            // the first time it is recorded), then evict the oldest
            // recorded steers while the session holds more than
            // MAX_COMPLETED_STEERS_PER_SESSION.
            let key = SteerKey::new(contract, kind, text);
            let is_new = !self
                .completed_steers
                .get(session_id)
                .map(|done| done.contains_key(&key))
                .unwrap_or(false);
            self.completed_steers
                .entry(session_id.to_string())
                .or_default()
                .insert(key.clone(), turn.clone());
            if is_new {
                self.completed_steer_order
                    .entry(session_id.to_string())
                    .or_default()
                    .push_back(key);
            }
            self.evict_oldest_completed_steers(session_id);
        }
        Ok(state)
    }

    /// Release an in-flight steer key WITHOUT recording a result: the
    /// turn ended in error, so there is nothing to replay, and a retry
    /// with the same key must still execute (the BUG-10 rule that only
    /// a turn that ran to a result is recorded). Called by the
    /// production wrapper (session.rs run_steer_turn) when run_turn
    /// fails, and by the test-only steer() helper on the same path.
    /// Idempotent on purpose: releasing a key that is not in-flight
    /// (never began, already completed) is a no-op, never an error —
    /// the turn already failed, so this must not mask it with a second
    /// failure. (BUG-10 extra, tin 9645/9650.)
    pub fn release_steer(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
        kind: &str,
        text: &str,
    ) {
        if let Some(running) = self.in_flight_steers.get_mut(session_id) {
            running.remove(&SteerKey::new(contract, kind, text));
        }
    }

    /// Run a whole turn in one call (tests only): begin, turn, complete.
    /// Production wrappers use the three steps separately so the turn
    /// runs without the registry lock held. Events go to `emit`. A
    /// duplicate steer returns the recorded result with no turn run,
    /// replaying exactly the cached Result event and nothing else
    /// (CI P2-2; BUG-10, #25); a steer already running elsewhere is
    /// refused (BUG-10 extra, tin 9645/9650); a failed turn releases
    /// its in-flight key, so the retry still executes.
    #[cfg(test)]
    pub fn steer(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
        kind: &str,
        text: &str,
        emit: &mut dyn FnMut(SenEvent),
    ) -> Result<SessionState, CommandError> {
        match self.begin_steer(session_id, contract, kind, text)? {
            SteerStart::Duplicate { state, turn } => {
                // CI P2-2: mirror of run_steer_turn — replay the
                // cached Result event so a caller waiting for it
                // resolves. No model run, no log entry, no other event.
                emit(turn.result_event());
                Ok(state)
            }
            SteerStart::InFlight { .. } => Err(CommandError::new(
                ERR_DUPLICATE_STEER,
                "Steer already running on this session, refused",
            )),
            SteerStart::Run(mut bridge) => {
                // BUG-11: the contract timeout_ms is the turn deadline, capped
                // at the 120s turn timeout inside the bridge (min(timeout_ms,
                // 120s) per tin 9566). A hanging turn is killed at that
                // deadline instead of running to the cap or hanging forever.
                // P0-3 fallback: a turn that fails with a retryable code
                // (auth/rate-limit/no-result/timeout/...) runs again on
                // the next route in the chain. P2-2: the attempt order
                // is snapshotted once here, at turn start, and the turn
                // counts attempts — at most MAX_ATTEMPTS in total. The
                // order used to be re-read from disk on every retry,
                // so a config change mid-turn could drop the failed
                // route out of the chain and restart the loop at the
                // first route forever; there was no counter to stop it.
                // A non-retryable failure returns at once, never retried.
                let mut failed_route = self
                    .route_id(session_id)
                    .unwrap_or_else(|| "spark".to_string());
                let order = self.attempt_order();
                let mut attempts = 1;
                let turn = loop {
                    match bridge.run_turn_with_timeout(
                        kind,
                        text,
                        Duration::from_millis(contract.timeout_ms),
                        emit,
                    ) {
                        Ok(turn) => break turn,
                        Err(e) => {
                            if !super::model_router::is_retryable_error_code(&e.code) {
                                self.release_steer(session_id, contract, kind, text);
                                return Err(bridge_error_to_command(e));
                            }
                            if attempts >= super::model_router::MAX_ATTEMPTS {
                                self.release_steer(session_id, contract, kind, text);
                                return Err(bridge_error_to_command(e));
                            }
                            attempts += 1;
                            match self.fallback_bridge(session_id, &failed_route, &order) {
                                Some((next_bridge, next_route)) => {
                                    failed_route = next_route.id.clone();
                                    bridge = next_bridge;
                                }
                                None => {
                                    self.release_steer(session_id, contract, kind, text);
                                    return Err(bridge_error_to_command(e));
                                }
                            }
                        }
                    }
                };
                self.complete_steer(session_id, contract, kind, text, &turn)
            }
        }
    }

    /// sen_session_abort, step 1 of 3 (R2B-07): validate the contract
    /// and the transition, then hand out the session's bridge. The
    /// state is NOT changed here: the caller runs bridge.abort() with
    /// NO registry lock held (it polls up to the 5s grace when the
    /// child ignores SIGINT), then calls complete_abort to record the
    /// outcome. Returns None when the session has no bridge (the
    /// complete step then records a plain NoChild abort).
    pub fn begin_abort(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<Option<Arc<CliBridge>>, CommandError> {
        validate_contract(contract)?;
        let record = self.get_mut_or_unknown(session_id)?;
        if !can_transition(record.state, SessionState::Cancelled) {
            return Err(CommandError::new(
                ERR_ILLEGAL_TRANSITION,
                &format!(
                    "Illegal state transition: {} to Cancelled",
                    record.state.as_str()
                ),
            ));
        }
        Ok(self.bridges.get(session_id).cloned())
    }

    /// sen_session_abort, step 3 of 3 (R2B-07): re-check the transition
    /// — the state may have changed while bridge.abort() ran lock-free
    /// (a concurrent abort may already have cancelled the session) —
    /// then Running, AwaitingApproval, or Error -> Cancelled, per the
    /// point-854 table as amended by P2-9 (Error gains the Cancelled
    /// path so an errored session can still be closed), with the
    /// subscriber count reset to 0 and the log entry matching the
    /// AbortOutcome, exactly as the old one-call abort did. Terminal
    /// states have no outgoing path at all.
    pub fn complete_abort(
        &mut self,
        session_id: &str,
        outcome: AbortOutcome,
    ) -> Result<SessionState, CommandError> {
        let record = self.get_mut_or_unknown(session_id)?;
        if !can_transition(record.state, SessionState::Cancelled) {
            return Err(CommandError::new(
                ERR_ILLEGAL_TRANSITION,
                &format!(
                    "Illegal state transition: {} to Cancelled",
                    record.state.as_str()
                ),
            ));
        }
        record.state = SessionState::Cancelled;
        // BUG-09 (#24): close drops the subscriber count. A cancelled
        // session has no listeners left: whatever subscribers were
        // attached are gone with the session, so the count resets to 0
        // instead of leaking. This holds even for a direct abort that
        // bypasses the frontend close path (which unsubscribes first).
        record.subscribers = 0;
        match outcome {
            AbortOutcome::NoChild => record.log.push("session_abort".to_string()),
            AbortOutcome::ExitedAfterSigint => record.log.push("session_abort:sigint".to_string()),
            AbortOutcome::KilledAfterGrace => record
                .log
                .push("session_abort:kill-after-grace".to_string()),
        }
        Ok(record.state)
    }

    /// Run a whole abort in one call (tests only): begin, escalate,
    /// complete. Production wrappers use the three steps separately so
    /// the escalation runs without the registry lock held (R2B-07).
    /// With no turn running the escalation is a no-op and this is a
    /// pure state transition.
    #[cfg(test)]
    pub fn abort(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<SessionState, CommandError> {
        let bridge = self.begin_abort(session_id, contract)?;
        let outcome = match bridge {
            Some(bridge) => bridge.abort(),
            None => AbortOutcome::NoChild,
        };
        self.complete_abort(session_id, outcome)
    }

    /// sen_events_subscribe: records one subscriber on the session. The
    /// event stream itself will flow from the backend (Tauri events) once
    /// the CLI bridge lands; this record is the backend bookkeeping for
    /// it, not the stream.
    ///
    /// BUG-09 (#24): a terminal session accepted subscribers, so the
    /// count only ever grew. Like steer, subscribe now refuses a
    /// terminal session with ILLEGAL_TRANSITION; only a live session
    /// accepts a new subscriber.
    pub fn subscribe(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<u32, CommandError> {
        validate_contract(contract)?;
        let record = self.get_mut_or_unknown(session_id)?;
        if record.state.is_terminal() {
            return Err(CommandError::new(
                ERR_ILLEGAL_TRANSITION,
                &format!(
                    "Cannot subscribe to a session in state {}, only a live session accepts subscribers",
                    record.state.as_str()
                ),
            ));
        }
        record.subscribers += 1;
        Ok(record.subscribers)
    }

    /// sen_events_unsubscribe: drops one subscriber from the session and
    /// returns the remaining count. BUG-09 (#24): no unsubscribe existed,
    /// so the subscriber count leaked upward forever. The count
    /// saturates at 0: an extra unsubscribe (for example a close racing
    /// a second close) is a no-op returning 0, never an underflow and
    /// never a failure that could block cleanup. Unsubscribe is allowed
    /// on a terminal session for the same reason: abort already drops
    /// the count to 0, so unsubscribing after close is a harmless no-op
    /// instead of a second error on the close path.
    pub fn unsubscribe(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<u32, CommandError> {
        validate_contract(contract)?;
        let record = self.get_mut_or_unknown(session_id)?;
        record.subscribers = record.subscribers.saturating_sub(1);
        Ok(record.subscribers)
    }

    /// P0-2 approval gate, operator side:
    /// - `allow_tool` adds one explicit allow rule to the policy.
    ///   The policy starts empty (default-deny), so this is the only
    ///   way a tool becomes runnable at all.
    /// - `approve_tool` grants one approval for one tool + one
    ///   idempotency key, by a named granter. A grant for a
    ///   never-tier tool is recorded but never honored. A grant
    ///   lives APPROVAL_TTL_SECS (15 minutes) and is spent the
    ///   first time the gate allows a call with it (P1-4).
    /// - `revoke_tool` removes one approval: the next call with
    ///   that tool + key waits for a fresh approval. It never
    ///   touches the activity log (the log has no delete path).
    /// - `activity_log` / `list_approvals` read back the gate state
    ///   (clones; the log has no delete or update path).
    pub fn allow_tool(&mut self, tool_name: &str) {
        self.tool_policy.allow_tool(tool_name);
    }

    pub fn set_tool_policy(&mut self, policy: ToolPolicy) {
        self.tool_policy = policy;
    }

    pub fn approve_tool(
        &mut self,
        tool_name: &str,
        idempotency_key: &str,
        granted_by: &str,
    ) -> Approval {
        self.approvals.grant(tool_name, idempotency_key, granted_by)
    }

    pub fn revoke_tool(&mut self, tool_name: &str, idempotency_key: &str) -> bool {
        self.approvals.revoke(tool_name, idempotency_key)
    }

    pub fn activity_log(&self) -> Vec<ActivityEntry> {
        self.activity_log.entries()
    }

    pub fn list_approvals(&mut self) -> Vec<Approval> {
        self.approvals.list()
    }

    /// sen_tool_call: validates the contract and the gate order, then
    /// runs the P0-2 approval gate (auto / ask-first / never +
    /// default-deny policy) and records the evaluation in the
    /// append-only activity log — allowed, refused, and
    /// awaiting-approval alike. A gate refusal fails here, before any
    /// execution is attempted. When the gate passes, the call still
    /// fails loudly with TOOL_EXECUTION_NOT_WIRED: the gate execute
    /// step is a later phase, and no tool runs for real yet.
    pub fn tool_call(
        &mut self,
        contract: &ActionContract,
        tool_name: &str,
    ) -> Result<(), CommandError> {
        validate_contract(contract)?;
        if tool_name.is_empty() {
            return Err(CommandError::new(
                ERR_INVALID_CONTRACT,
                "tool_name must be non-empty, FAIL",
            ));
        }
        assert_gate_order(&GATE_ORDER)?;
        let evaluation = gate_tool_call(
            &self.tool_policy,
            &mut self.approvals,
            &mut self.activity_log,
            &contract.actor,
            tool_name,
            &contract.idempotency_key,
        );
        match evaluation.decision {
            GateDecision::Allow => Err(CommandError::new(
                ERR_TOOL_EXECUTION_NOT_WIRED,
                "Approval gate passed, but tool execution is NOT FILLED IN YET: gate execute step is a later phase, no tool ran.",
            )),
            GateDecision::AskFirst => Err(CommandError::new(
                ERR_APPROVAL_REQUIRED,
                &evaluation.reason,
            )),
            GateDecision::Deny => {
                if evaluation.tier == super::approval_gate::ApprovalTier::Never {
                    Err(CommandError::new(
                        ERR_APPROVAL_NEVER_DENIED,
                        &evaluation.reason,
                    ))
                } else {
                    Err(CommandError::new(
                        ERR_TOOL_POLICY_DENIED,
                        &evaluation.reason,
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> ActionContract {
        ActionContract {
            id: "c1".to_string(),
            capability: "sen.session".to_string(),
            capability_version: "1".to_string(),
            actor: "test".to_string(),
            purpose: "test".to_string(),
            phase: "test".to_string(),
            tool_kind: "registered-tool".to_string(),
            side_effect: "no-side-effect".to_string(),
            risk: "low".to_string(),
            idempotency_key: "k1".to_string(),
            timeout_ms: 1000,
            approval_required: false,
            audit_required: false,
        }
    }

    // P2-9: the Error table edge is live now (Error -> Cancelled,
    // so an errored session can still be closed). The point-854
    // table said "Error has exactly 2 paths"; the third path is
    // the P2-9 amendment: without it an errored session is a trap
    // (abort refuses, and retry is the only exit).
    #[test]
    fn statechart_point_854() {
        use SessionState::*;
        assert!(can_transition(Running, AwaitingApproval));
        assert!(can_transition(AwaitingApproval, Running));
        assert!(can_transition(AwaitingApproval, Cancelled));
        assert!(!can_transition(AwaitingApproval, Error));
        assert!(can_transition(Error, Running));
        assert!(can_transition(Error, Failed));
        assert!(can_transition(Error, Cancelled));
        for terminal in [Completed, Cancelled, Failed] {
            for to in [
                Running,
                AwaitingApproval,
                Error,
                Completed,
                Cancelled,
                Failed,
            ] {
                assert!(!can_transition(terminal, to));
            }
        }
    }

    /// A fake `zen` binary for tests: prints "zen 0.1.0" for --version,
    /// else runs the given shell body (which prints canned stream-json).
    fn fake_zen(tag: &str, body: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sen_reg_fake_zen_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zen");
        let mut script = String::from("#!/bin/sh\n");
        script.push_str("if [ \"$1\" = \"--version\" ]; then echo \"zen 0.1.0\"; exit 0; fi\n");
        script.push_str(body);
        script.push('\n');
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    const HAPPY_BODY: &str = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","tools":[],"mcp_servers":[],"permissionMode":"default","slash_commands":[],"v":1}
{"type":"assistant","uuid":"u1","session_id":"s1","message":{"role":"assistant","content":"hello"},"parent_tool_use_id":null,"v":1}
{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0},"model":"m:free","session_id":"s1","v":1}
EOF
exit 0"#;

    // P2-9: a turn that ran to a result record carrying is_error=true
    // (the real CLI's behaviour on a bad key: HTTP 401). The model
    // ran, so complete_steer records it; the session must move to
    // Error, not stay Running as if the turn had succeeded.
    const ERROR_BODY: &str = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
{"type":"result","subtype":"error_during_execution","is_error":true,"result":"HTTP 401 Unauthorized","duration_ms":1,"num_turns":0,"usage":{},"model":"m:free","session_id":"s1","v":1}
EOF
exit 1"#;

    #[test]
    fn create_then_duplicate_then_unknown() {
        let mut reg = Registry::new();
        assert_eq!(reg.create("s1", &contract()), Ok(SessionState::Running));
        let dup = reg.create("s1", &contract()).unwrap_err();
        assert_eq!(dup.code, ERR_DUPLICATE_SESSION);
        let mut emit = |_ev: SenEvent| {};
        let missing = reg
            .steer("nope", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
        let missing = reg.abort("nope", &contract()).unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
        let missing = reg.subscribe("nope", &contract()).unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
    }

    #[test]
    fn create_wires_a_bridge_with_the_default_free_model() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        let bridge = reg.bridge("s1").unwrap();
        assert_eq!(bridge.session_id(), "s1");
        assert_eq!(bridge.model(), DEFAULT_MODEL);
    }

    #[test]
    fn create_uses_the_stored_settings_model() {
        let dir = std::env::temp_dir().join(format!("sen_reg_settings_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        CliSettings {
            model: "some/model:free".to_string(),
            base_url: None,
            router: None,
        }
        .save(&dir)
        .unwrap();
        let mut reg = Registry::with_settings_dir(dir.clone());
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.bridge("s1").unwrap().model(), "some/model:free");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // R2B-05: a stored base URL reaches the new session's bridge, and
    // a settings file without one leaves the bridge with no base URL.
    #[test]
    fn create_uses_the_stored_settings_base_url() {
        let dir = std::env::temp_dir().join(format!("sen_reg_baseurl_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        CliSettings {
            model: "some/model:free".to_string(),
            base_url: Some("https://example.invalid/v1".to_string()),
            router: None,
        }
        .save(&dir)
        .unwrap();
        let mut reg = Registry::with_settings_dir(dir.clone());
        reg.create("s1", &contract()).unwrap();
        assert_eq!(
            reg.bridge("s1").unwrap().base_url(),
            Some("https://example.invalid/v1")
        );
        // Old file, no base_url key: the bridge gets None.
        std::fs::write(
            CliSettings::file_path(&dir),
            "{\"model\":\"some/model:free\"}",
        )
        .unwrap();
        let mut reg2 = Registry::with_settings_dir(dir.clone());
        reg2.create("s2", &contract()).unwrap();
        assert_eq!(reg2.bridge("s2").unwrap().base_url(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn steer_runs_the_model_and_logs_only_after_the_result() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("happy", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], SenEvent::SessionStart { .. }));
        assert!(matches!(events[2], SenEvent::Result { .. }));
        // The log grew only because the model ran (BUG-06 lesson).
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        assert!(reg.get("s1").unwrap().log[1].starts_with("steer:steer:success"));
        // A second turn on the same session works (resume path).
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "follow_up", "again", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
    }

    #[test]
    fn steer_failure_leaves_no_log_entry() {
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
EOF
exit 1"#;
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("noresult", body));
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, "CLI_NO_RESULT");
        // A steer that never ran a model to a result is never recorded
        // as if it had (BUG-06 lesson).
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
    }

    // BUG-10 (#25): a second steer with the same key + contract + text
    // replays the recorded result: no model run, no extra log entry,
    // and begin_steer reports the recorded turn itself.
    // CI P2-2: the replay re-emits exactly the cached Result event
    // (identical to the live turn's), so a caller waiting for it
    // (chat.ts armResultWaiter) resolves instead of timing out after
    // 130s. Nothing else is re-emitted: no SessionStart, no Message.
    #[test]
    fn steer_duplicate_key_replays_without_rerunning() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("dup", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut first_events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| first_events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(first_events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 1, "duplicate steer must replay one event");
        assert_eq!(
            events[0], first_events[2],
            "replayed event must be identical to the live Result event"
        );
        assert_eq!(
            reg.get("s1").unwrap().log.len(),
            2,
            "duplicate steer must push no log entry"
        );
        match reg.begin_steer("s1", &contract(), "steer", "hi").unwrap() {
            SteerStart::Duplicate { state, turn } => {
                assert_eq!(state, SessionState::Running);
                assert_eq!(turn.subtype, "success");
                assert_eq!(turn.text, "hello");
            }
            SteerStart::Run(_) | SteerStart::InFlight { .. } => {
                panic!("third identical steer must be Duplicate")
            }
        }
    }

    // R2B-02 (P1): a turn that returns a result record with
    // is_error=true (e.g. HTTP 401, subtype error_during_execution)
    // is NOT a completed steer. It must not be cached, so a retry
    // with the same key + contract + text runs the model again
    // instead of replaying the error as a Duplicate.
    #[test]
    fn steer_error_turn_is_not_cached_retry_reruns_model() {
        let marker = std::env::temp_dir().join(format!("sen_reg_r2b02_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // First run: no marker yet -> the error result, exit 1 (the
        // real CLI's behaviour on a bad key). Later runs: marker
        // exists -> the happy output (key fixed, model runs).
        let body = format!(
            r#"if [ -f '{marker}' ]; then cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"assistant","uuid":"u1","session_id":"s1","message":{{"role":"assistant","content":"hello"}},"parent_tool_use_id":null,"v":1}}
{{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 0; else touch '{marker}'; cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"HTTP 401 Unauthorized","duration_ms":1,"num_turns":0,"usage":{{}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 1; fi"#,
            marker = marker.display()
        );
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("r2b02", &body));
        reg.create("s1", &contract()).unwrap();
        // Steer 1: the error turn. The backend ack is Ok(Error) -
        // the error is visible in the event stream, not as a failure,
        // and the session records that its last turn errored (P2-9:
        // Running -> Error) instead of staying Running as if
        // nothing had happened.
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Error)
        );
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Error);
        assert_eq!(events.len(), 2, "error turn must still emit its events");
        assert!(matches!(events[1], SenEvent::Result { is_error: true, .. }));
        if let SenEvent::Result { subtype, text, .. } = &events[1] {
            assert_eq!(subtype, "error_during_execution");
            assert!(text.contains("401"), "error text must carry the 401");
        }
        // The error turn ran, so it is logged - but it must NOT be
        // cached as a completed steer.
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        assert_eq!(
            reg.get("s1").unwrap().log[1],
            "steer:steer:error_during_execution"
        );
        assert_eq!(
            reg.completed_steers
                .get("s1")
                .map(|done| done.len())
                .unwrap_or(0),
            0,
            "error turn must not be recorded in completed_steers"
        );
        // Steer 2, same key + contract + text: must run the model
        // again (events emitted, fake binary ran a second time and
        // returned the happy result), not replay the error. The
        // session recovers: a successful turn from Error goes back
        // to Running (P2-9: the Error -> Running path).
        let mut retry_events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| retry_events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
        assert_eq!(
            retry_events.len(),
            3,
            "retry after an error turn must run the model"
        );
        assert!(matches!(
            retry_events[2],
            SenEvent::Result {
                is_error: false,
                ..
            }
        ));
        assert!(marker.exists(), "fake binary must have run a second time");
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
        // The successful retry IS cached: a third identical steer
        // replays it (BUG-10 behaviour, unchanged) — CI P2-2: as one
        // replayed Result event identical to the retry's, not silence.
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 1, "success turn must still be cached");
        assert_eq!(
            events[0], retry_events[2],
            "replayed event must be identical to the live Result event"
        );
        let _ = std::fs::remove_file(&marker);
    }

    // P2-9: an error turn moves the session Running -> Error, the
    // turn is logged but not cached (R2B-02, unchanged), and a retry
    // from Error runs the model again and recovers to Running on
    // success. Before the fix the error turn left the session
    // Running, so no caller could tell the last turn had errored.
    #[test]
    fn p2_9_error_turn_moves_session_to_error_and_retry_recovers() {
        let marker = std::env::temp_dir().join(format!("sen_reg_p29_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // First run: no marker yet -> the error result, exit 1 (the
        // real CLI's behaviour on a bad key). Later runs: marker
        // exists -> the happy output (key fixed, model runs).
        let body = format!(
            r#"if [ -f '{marker}' ]; then cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"assistant","uuid":"u1","session_id":"s1","message":{{"role":"assistant","content":"hello"}},"parent_tool_use_id":null,"v":1}}
{{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 0; else touch '{marker}'; cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"HTTP 401 Unauthorized","duration_ms":1,"num_turns":0,"usage":{{}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 1; fi"#,
            marker = marker.display()
        );
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("p29err", &body));
        reg.create("s1", &contract()).unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Error)
        );
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Error);
        assert_eq!(events.len(), 2, "error turn must still emit its events");
        assert!(matches!(events[1], SenEvent::Result { is_error: true, .. }));
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        assert_eq!(
            reg.get("s1").unwrap().log[1],
            "steer:steer:error_during_execution"
        );
        assert_eq!(
            reg.completed_steers
                .get("s1")
                .map(|done| done.len())
                .unwrap_or(0),
            0,
            "error turn must not be recorded in completed_steers"
        );
        // Retry from Error, same key: begin_steer accepts the Error
        // state, the turn re-runs (not a replay: nothing was cached),
        // and the session recovers to Running on success.
        let mut retry_events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| retry_events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(retry_events.len(), 3, "retry must run the model");
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
        let _ = std::fs::remove_file(&marker);
    }

    // P2-9: a second consecutive error turn fails the session
    // terminally (Error -> Failed, the table's second Error path).
    // A Failed session refuses steer and subscribe, like any
    // terminal state; abort refuses too (terminal has no outgoing
    // path). Before the fix both error turns left the session
    // Running, so Failed was unreachable.
    #[test]
    fn p2_9_second_consecutive_error_turn_fails_session() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("p29fail", ERROR_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Error)
        );
        // Same key again: error turns are never cached, so the retry
        // runs the model a second time — and errors a second time.
        let mut other = contract();
        other.idempotency_key = "k2".to_string();
        let mut emit = |_ev: SenEvent| {};
        assert_eq!(
            reg.steer("s1", &other, "steer", "hi", &mut emit),
            Ok(SessionState::Failed)
        );
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Failed);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "again", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
        let err = reg.subscribe("s1", &contract()).unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
        let err = reg.abort("s1", &contract()).unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
    }

    // P2-9: abort from Error reaches Cancelled (the table's third
    // Error path, added by this fix). Before the fix the session
    // never entered Error, so this edge was dead; without the edge
    // an errored session is a trap — abort refuses and retry is
    // the only exit, so a user who wants to close the session
    // cannot.
    #[test]
    fn p2_9_abort_from_error_cancels() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("p29abort", ERROR_BODY));
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.subscribe("s1", &contract()), Ok(1));
        let mut emit = |_ev: SenEvent| {};
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Error)
        );
        assert_eq!(reg.abort("s1", &contract()), Ok(SessionState::Cancelled));
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Cancelled);
        assert_eq!(reg.get("s1").unwrap().subscribers, 0);
    }

    // P2-9, deliberate deviation from the brief's literal "Completed
    // when a turn succeeds": a successful turn keeps the session
    // Running. One live session serves every chat turn (chat.ts:
    // steer once, then follow_up for each later message; main.ts
    // attaches one session per app lifetime), so Completed after a
    // turn would terminal the session after the first message and
    // refuse every later one with ILLEGAL_TRANSITION — and break
    // Duplicate replay (BUG-10: begin_steer refuses terminal
    // sessions). Completed stays defined but unreachable: no
    // finish/close command exists, a session ends via abort
    // (Cancelled). This test pins the live behaviour.
    #[test]
    fn p2_9_success_turn_keeps_session_running_for_next_turn() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("p29live", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "follow_up", "again", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
    }

    // BUG-10 extra (tin 9645/9650): a second steer identical to one
    // still running is refused (InFlight) — no second model call.
    // After the first turn completes, the same steer replays the
    // recorded result (Duplicate): the old behaviour, unchanged.
    #[test]
    fn steer_second_identical_while_first_in_flight_is_refused() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("inflight", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        reg.create("s2", &contract()).unwrap();
        // First steer begins: Run, and the key is in-flight at once.
        let bridge = match reg.begin_steer("s1", &contract(), "steer", "hi").unwrap() {
            SteerStart::Run(bridge) => bridge,
            SteerStart::Duplicate { .. } | SteerStart::InFlight { .. } => {
                panic!("first steer must Run")
            }
        };
        assert_eq!(
            reg.in_flight_steers.get("s1").map(|running| running.len()),
            Some(1),
            "begin_steer must record the key as in-flight at once"
        );
        // Identical second steer while the first still runs: refused.
        match reg.begin_steer("s1", &contract(), "steer", "hi").unwrap() {
            SteerStart::InFlight { state } => {
                assert_eq!(state, SessionState::Running);
            }
            SteerStart::Run(_) | SteerStart::Duplicate { .. } => {
                panic!("second identical steer while the first runs must be InFlight")
            }
        }
        // Same key on another session is independent: Run.
        assert!(matches!(
            reg.begin_steer("s2", &contract(), "steer", "hi").unwrap(),
            SteerStart::Run(_)
        ));
        // Different kind / different text on s1 are different steers:
        // they still begin (Run), not refused.
        assert!(matches!(
            reg.begin_steer("s1", &contract(), "follow_up", "hi")
                .unwrap(),
            SteerStart::Run(_)
        ));
        assert!(matches!(
            reg.begin_steer("s1", &contract(), "steer", "other")
                .unwrap(),
            SteerStart::Run(_)
        ));
        // Tidy up the extra in-flight keys (their turns never ran).
        reg.release_steer("s1", &contract(), "follow_up", "hi");
        reg.release_steer("s1", &contract(), "steer", "other");
        reg.release_steer("s2", &contract(), "steer", "hi");
        // Finish the first turn through the bridge, then complete: the
        // key moves from in-flight to completed.
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let turn = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        assert_eq!(events.len(), 3);
        reg.complete_steer("s1", &contract(), "steer", "hi", &turn)
            .unwrap();
        assert!(reg
            .in_flight_steers
            .get("s1")
            .map(|running| running.is_empty())
            .unwrap_or(true));
        // Third identical steer now replays the recorded result.
        match reg.begin_steer("s1", &contract(), "steer", "hi").unwrap() {
            SteerStart::Duplicate { state, turn } => {
                assert_eq!(state, SessionState::Running);
                assert_eq!(turn.subtype, "success");
                assert_eq!(turn.text, "hello");
            }
            SteerStart::Run(_) | SteerStart::InFlight { .. } => {
                panic!("third identical steer must be Duplicate")
            }
        }
    }

    // BUG-10 extra (tin 9645/9650): a turn that fails releases its
    // in-flight key, so the retry with the same key still executes
    // instead of being refused for a turn that already ended.
    #[test]
    fn steer_failed_turn_releases_in_flight_key() {
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
EOF
exit 1"#;
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("inflightfail", body));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, "CLI_NO_RESULT");
        // The failed turn left no in-flight key behind ...
        assert!(reg
            .in_flight_steers
            .get("s1")
            .map(|running| running.is_empty())
            .unwrap_or(true));
        // ... so the retry with the same key begins cleanly (Run, not
        // InFlight).
        assert!(matches!(
            reg.begin_steer("s1", &contract(), "steer", "hi").unwrap(),
            SteerStart::Run(_)
        ));
    }

    // BUG-10 extra (tin 9645/9650): complete_steer drops the
    // in-flight key even when it refuses to record (abort won the
    // race), so the key never leaks for a turn that already ended.
    #[test]
    fn complete_steer_refusal_still_releases_in_flight_key() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("inflightabort", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let bridge = match reg.begin_steer("s1", &contract(), "steer", "hi").unwrap() {
            SteerStart::Run(bridge) => bridge,
            SteerStart::Duplicate { .. } | SteerStart::InFlight { .. } => {
                panic!("first steer must Run")
            }
        };
        assert_eq!(
            reg.in_flight_steers.get("s1").map(|running| running.len()),
            Some(1)
        );
        // The turn runs to a result first; abort wins the race before
        // complete_steer is called (the real race: abort lands while
        // the turn is resolving).
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let turn = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        reg.abort("s1", &contract()).unwrap();
        // complete refuses (the session left Running), but the
        // in-flight key must be gone all the same.
        let err = reg
            .complete_steer("s1", &contract(), "steer", "hi", &turn)
            .unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
        assert!(
            reg.in_flight_steers
                .get("s1")
                .map(|running| running.is_empty())
                .unwrap_or(true),
            "refused complete_steer must still release the in-flight key"
        );
    }

    // Same text, different idempotency key: a new request, runs again.
    #[test]
    fn steer_different_key_runs_again() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("diffkey", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        reg.steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap();
        let mut other = contract();
        other.idempotency_key = "k2".to_string();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &other, "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
    }

    // Same key, different text: a different request body, runs again.
    #[test]
    fn steer_same_key_different_text_runs_again() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("difftext", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        reg.steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "other", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
    }

    // Same key + text but a different kind (steer vs follow_up): a
    // different operation, runs again.
    #[test]
    fn steer_same_key_same_text_different_kind_runs_again() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("diffkind", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        reg.steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "follow_up", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
    }

    // Same key + text but a different contract id: a different action
    // that happens to reuse a key, runs again.
    #[test]
    fn steer_same_key_same_text_different_contract_runs_again() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("diffcontract", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        reg.steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap();
        let mut other = contract();
        other.id = "c2".to_string();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &other, "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
    }

    // P0-3 SUPERSEDES the pre-router reading of this test: a
    // retryable failure (exit 1, no result = CLI_NO_RESULT) no longer
    // ends the steer — the same steer falls back to the next route
    // and succeeds there. The BUG-10 rule itself ("only a turn that
    // ran to a result is recorded") still holds and is pinned by the
    // R2B-02 error-turn test plus the non-retryable test below: a
    // failure that no route can fix still records nothing, so a
    // retry with the same key still runs the model.
    #[test]
    fn steer_retryable_failure_falls_back_within_the_same_steer() {
        let marker =
            std::env::temp_dir().join(format!("sen_reg_bug10_retry_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // First run: no marker yet -> exit 1 with no result record.
        // Later runs: marker exists -> the happy output.
        let body = format!(
            r#"if [ -f '{marker}' ]; then cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"assistant","uuid":"u1","session_id":"s1","message":{{"role":"assistant","content":"hello"}},"parent_tool_use_id":null,"v":1}}
{{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 0; else touch '{marker}'; exit 1; fi"#,
            marker = marker.display()
        );
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("retry", &body));
        reg.create("s1", &contract()).unwrap();
        // The first steer succeeds through the fallback (the failed
        // first attempt emitted no events; the retry ran the happy
        // turn), and the successful turn IS recorded: a second
        // identical steer replays it as one Result event (CI P2-2).
        let mut first_events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| first_events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(first_events.len(), 3, "fallback retry must run the model");
        assert_eq!(reg.route_id("s1").as_deref(), Some("openrouter-free"));
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 1, "success turn must still be cached");
        assert_eq!(
            events[0], first_events[2],
            "replayed event must be identical to the live Result event"
        );
        let _ = std::fs::remove_file(&marker);
    }

    // P0-3: the BUG-10 "failed turn records nothing" rule, kept for a
    // failure no route can fix (exit 4 = validation, not retryable):
    // the first steer errors, records nothing, and the retry with the
    // same key still runs the model instead of replaying.
    #[test]
    fn steer_non_retryable_failure_does_not_record_key() {
        let marker =
            std::env::temp_dir().join(format!("sen_reg_bug10_noretry_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // First run: no marker yet -> exit 4 with no result record.
        // Later runs: marker exists -> the happy output.
        let body = format!(
            r#"if [ -f '{marker}' ]; then cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"assistant","uuid":"u1","session_id":"s1","message":{{"role":"assistant","content":"hello"}},"parent_tool_use_id":null,"v":1}}
{{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 0; else touch '{marker}'; cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
EOF
exit 4; fi"#,
            marker = marker.display()
        );
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("noretry", &body));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, "CLI_VALIDATION");
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3, "retry after failure must run");
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn steer_enforces_contract_timeout_ms() {
        // BUG-11: a hanging CLI with contract timeout_ms=300 must fail
        // as CLI_TURN_TIMEOUT in well under 10s, not run on to the
        // 120s bridge cap. The effective deadline is
        // min(timeout_ms, TURN_TIMEOUT 120s) per the tin 9566 decision.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("bug11hang", body));
        reg.create("s1", &contract()).unwrap();
        let mut c = contract();
        c.timeout_ms = 300;
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = reg.steer("s1", &c, "steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, "CLI_TURN_TIMEOUT");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        // A failed steer leaves no log entry and the session stays Running.
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
    }

    #[test]
    fn begin_steer_rejects_bad_kind_empty_text_and_terminal() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("happy2", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let err = reg
            .begin_steer("s1", &contract(), "nudge", "hi")
            .unwrap_err();
        assert_eq!(err.code, ERR_INVALID_CONTRACT);
        let err = reg
            .begin_steer("s1", &contract(), "steer", "   ")
            .unwrap_err();
        assert_eq!(err.code, ERR_INVALID_CONTRACT);
        reg.abort("s1", &contract()).unwrap();
        let err = reg
            .begin_steer("s1", &contract(), "steer", "hi")
            .unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
    }

    #[test]
    fn abort_escalates_the_running_turn_then_cancels() {
        // Fake CLI: traps SIGINT, prints the interrupted result, exits
        // 130 — the real CLI's SIGINT behaviour.
        let body = r#"trap 'printf "%s\n" "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"result\":\"Interrupted by user\",\"duration_ms\":1,\"num_turns\":0,\"usage\":{},\"model\":\"m:free\",\"session_id\":\"s1\",\"v\":1}"; exit 130' INT
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("abort", body));
        reg.create("s1", &contract()).unwrap();
        let bridge = reg.bridge("s1").unwrap();
        let bridge_run = std::sync::Arc::clone(&bridge);
        let handle = std::thread::spawn(move || {
            let mut emit = |_ev: SenEvent| {};
            bridge_run.run_turn("steer", "hi", &mut emit)
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(reg.abort("s1", &contract()), Ok(SessionState::Cancelled));
        assert!(reg
            .get("s1")
            .unwrap()
            .log
            .contains(&"session_abort:sigint".to_string()));
        // The interrupted turn resolves with its interrupted result.
        let turn = handle.join().unwrap().unwrap();
        assert!(turn.is_error);
        assert_eq!(turn.subtype, "error_during_execution");
        // complete_steer refuses: the session left Running meanwhile,
        // so the interrupted turn is not recorded as a steer.
        let err = reg
            .complete_steer("s1", &contract(), "steer", "hi", &turn)
            .unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
    }

    #[test]
    fn abort_paths_follow_the_table() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.abort("s1", &contract()), Ok(SessionState::Cancelled));
        let err = reg.abort("s1", &contract()).unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
    }

    #[test]
    fn contract_validation_rejects_each_bad_field() {
        let mut bad = contract();
        bad.id.clear();
        assert_eq!(
            Registry::new().create("s1", &bad).unwrap_err().code,
            ERR_INVALID_CONTRACT
        );
        let mut bad = contract();
        bad.idempotency_key.clear();
        assert_eq!(
            Registry::new().create("s1", &bad).unwrap_err().code,
            ERR_INVALID_CONTRACT
        );
        let mut bad = contract();
        bad.timeout_ms = 0;
        assert_eq!(
            Registry::new().create("s1", &bad).unwrap_err().code,
            ERR_INVALID_CONTRACT
        );
        let mut bad = contract();
        bad.tool_kind = "shell".to_string();
        assert_eq!(
            Registry::new().create("s1", &bad).unwrap_err().code,
            ERR_INVALID_CONTRACT
        );
        let mut bad = contract();
        bad.side_effect = "explode".to_string();
        assert_eq!(
            Registry::new().create("s1", &bad).unwrap_err().code,
            ERR_INVALID_CONTRACT
        );
        let err = Registry::new().create("", &contract()).unwrap_err();
        assert_eq!(err.code, ERR_INVALID_CONTRACT);
    }

    #[test]
    fn gate_order_exact() {
        assert!(assert_gate_order(&GATE_ORDER).is_ok());
        assert!(assert_gate_order(&["protect"]).is_err());
        let mut reversed = GATE_ORDER;
        reversed.reverse();
        assert!(assert_gate_order(&reversed).is_err());
    }

    #[test]
    fn subscribe_counts_and_tool_call_refuses() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.subscribe("s1", &contract()), Ok(1));
        assert_eq!(reg.subscribe("s1", &contract()), Ok(2));
        // P0-2: "my-tool" is unclassified (ask-first) and the default
        // policy is empty, so default-deny refuses it before any
        // execution is attempted.
        let err = reg.tool_call(&contract(), "my-tool").unwrap_err();
        assert_eq!(err.code, ERR_TOOL_POLICY_DENIED);
        let err = reg.tool_call(&contract(), "").unwrap_err();
        assert_eq!(err.code, ERR_INVALID_CONTRACT);
    }

    // P0-2 approval gate, through Registry::tool_call:
    // - an auto tool with an explicit allow rule passes the gate and
    //   then fails only because execution is not wired yet;
    // - the same tool without a rule is refused by default-deny;
    // - an ask-first tool waits for approval, then passes the gate;
    // - a never-tier tool is refused even with a rule and an approval;
    // - every evaluation lands in the append-only activity log.
    #[test]
    fn tool_call_auto_allowed_passes_gate_but_execution_not_wired() {
        let mut reg = Registry::new();
        reg.allow_tool("read_file");
        let err = reg.tool_call(&contract(), "read_file").unwrap_err();
        assert_eq!(err.code, ERR_TOOL_EXECUTION_NOT_WIRED);
        let entries = reg.activity_log();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[0].tool, "read_file");
        assert_eq!(entries[0].decision, GateDecision::Allow);
        assert_eq!(entries[0].result, "allowed");
    }

    #[test]
    fn tool_call_auto_denied_by_default_policy() {
        let mut reg = Registry::new();
        let err = reg.tool_call(&contract(), "read_file").unwrap_err();
        assert_eq!(err.code, ERR_TOOL_POLICY_DENIED);
        assert!(err.message.contains("default-deny"));
        assert_eq!(reg.activity_log().len(), 1);
        assert_eq!(reg.activity_log()[0].result, "denied");
    }

    #[test]
    fn tool_call_ask_first_needs_approval_then_passes_gate() {
        let mut reg = Registry::new();
        reg.allow_tool("send_message");
        let err = reg.tool_call(&contract(), "send_message").unwrap_err();
        assert_eq!(err.code, ERR_APPROVAL_REQUIRED);
        assert_eq!(reg.activity_log()[0].result, "awaiting-approval");
        reg.approve_tool("send_message", "k1", "zen");
        let err = reg.tool_call(&contract(), "send_message").unwrap_err();
        assert_eq!(err.code, ERR_TOOL_EXECUTION_NOT_WIRED);
        let entries = reg.activity_log();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
        assert_eq!(entries[1].result, "allowed");
        // A different idempotency key is a different call: still waiting.
        let mut other = contract();
        other.idempotency_key = "k2".to_string();
        let err = reg.tool_call(&other, "send_message").unwrap_err();
        assert_eq!(err.code, ERR_APPROVAL_REQUIRED);
    }

    #[test]
    fn tool_call_approval_is_single_use() {
        // P1-4: one grant authorizes exactly one allowed call. The
        // first tool_call passes the gate (and ends in
        // TOOL_EXECUTION_NOT_WIRED, because execution is a later
        // phase); the same tool + key again waits for a fresh
        // approval instead of passing on the spent grant.
        let mut reg = Registry::new();
        reg.allow_tool("send_message");
        reg.approve_tool("send_message", "k1", "zen");
        let err = reg.tool_call(&contract(), "send_message").unwrap_err();
        assert_eq!(err.code, ERR_TOOL_EXECUTION_NOT_WIRED);
        let err = reg.tool_call(&contract(), "send_message").unwrap_err();
        assert_eq!(err.code, ERR_APPROVAL_REQUIRED);
        assert_eq!(reg.list_approvals().len(), 0);
    }

    #[test]
    fn revoke_tool_removes_the_grant_and_the_next_call_waits() {
        let mut reg = Registry::new();
        reg.allow_tool("send_message");
        assert!(!reg.revoke_tool("send_message", "k1"));
        reg.approve_tool("send_message", "k1", "zen");
        assert_eq!(reg.list_approvals().len(), 1);
        assert!(reg.revoke_tool("send_message", "k1"));
        assert!(!reg.revoke_tool("send_message", "k1"));
        assert!(reg.list_approvals().is_empty());
        // The revoked grant authorizes nothing: the call waits.
        let err = reg.tool_call(&contract(), "send_message").unwrap_err();
        assert_eq!(err.code, ERR_APPROVAL_REQUIRED);
        // Revoking never touches the log: only the one
        // awaiting-approval entry from this call is there.
        assert_eq!(reg.activity_log().len(), 1);
        assert_eq!(reg.activity_log()[0].result, "awaiting-approval");
    }

    #[test]
    fn tool_call_never_tier_denied_even_with_rule_and_approval() {
        let mut reg = Registry::new();
        for tool in [
            "trade_securities",
            "transfer_between_accounts",
            "access_other_account",
        ] {
            reg.allow_tool(tool);
            reg.approve_tool(tool, "k1", "zen");
            let err = reg.tool_call(&contract(), tool).unwrap_err();
            assert_eq!(err.code, ERR_APPROVAL_NEVER_DENIED, "{tool}");
        }
        let entries = reg.activity_log();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.result == "denied"));
        assert!(entries.iter().all(|e| e.decision == GateDecision::Deny));
    }

    #[test]
    fn tool_call_unknown_tool_is_never_auto() {
        let mut reg = Registry::new();
        reg.allow_tool("edit_file");
        // Unclassified + allowed by rule + no approval: ask-first,
        // never auto.
        let err = reg.tool_call(&contract(), "edit_file").unwrap_err();
        assert_eq!(err.code, ERR_APPROVAL_REQUIRED);
    }

    /// BUG-09 (#24): subscribing to a terminal session returned Ok and
    /// the count only ever grew. Subscribe must refuse exactly like
    /// steer, with ILLEGAL_TRANSITION, and leave the count untouched.
    #[test]
    fn subscribe_rejects_terminal_session() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.subscribe("s1", &contract()), Ok(1));
        reg.abort("s1", &contract()).unwrap();
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Cancelled);
        let err = reg.subscribe("s1", &contract()).unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
        assert_eq!(reg.get("s1").unwrap().subscribers, 0);
        // A live session still accepts subscribers after the refusal.
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.subscribe("s1", &contract()), Ok(1));
    }

    /// BUG-09 (#24): there was no unsubscribe at all. Unsubscribe drops
    /// the count one at a time, saturates at 0 instead of underflowing,
    /// and refuses an unknown session.
    #[test]
    fn unsubscribe_decrements_and_saturates_at_zero() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.subscribe("s1", &contract()), Ok(1));
        assert_eq!(reg.subscribe("s1", &contract()), Ok(2));
        assert_eq!(reg.unsubscribe("s1", &contract()), Ok(1));
        assert_eq!(reg.unsubscribe("s1", &contract()), Ok(0));
        // An extra unsubscribe (close racing a second close) is a
        // no-op returning 0, never an underflow.
        assert_eq!(reg.unsubscribe("s1", &contract()), Ok(0));
        let missing = reg.unsubscribe("nope", &contract()).unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
    }

    /// BUG-09 (#24): close must drop the subscriber count. Abort (the
    /// backend half of close) resets the count to 0, so a direct abort
    /// that bypasses the frontend close path leaks nothing either; and
    /// unsubscribing after close is a harmless no-op, not a second
    /// error on the close path.
    #[test]
    fn abort_drops_subscriber_count() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.subscribe("s1", &contract()), Ok(1));
        assert_eq!(reg.subscribe("s1", &contract()), Ok(2));
        assert_eq!(reg.abort("s1", &contract()), Ok(SessionState::Cancelled));
        assert_eq!(reg.get("s1").unwrap().subscribers, 0);
        assert_eq!(reg.unsubscribe("s1", &contract()), Ok(0));
    }

    // R2B-07: abort must NOT hold the registry lock while the bridge
    // polls the grace period. Thread A aborts a session whose child
    // ignores SIGINT (grace 1000ms, test-only) through the production
    // 3-step pattern (begin_abort / bridge.abort() lock-free /
    // complete_abort); thread B creates another session meanwhile and
    // must finish well under the grace. On the old code the wrapper
    // held the lock across the whole abort and thread B blocked for
    // the full grace (5s with the production grace, proven by the
    // scratch probe: 5.05s).
    #[test]
    fn abort_does_not_hold_registry_lock_during_grace() {
        let body = r#"trap '' INT
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let reg = std::sync::Arc::new(std::sync::Mutex::new(Registry::new()));
        {
            let mut r = reg.lock().unwrap();
            r.set_zen_bin(fake_zen("r2b07", body));
            r.set_abort_grace(Duration::from_millis(1000));
            r.create("s1", &contract()).unwrap();
        }
        // Start a turn on s1 in the background (child ignores SIGINT).
        let bridge = reg.lock().unwrap().bridge("s1").unwrap();
        let handle = std::thread::spawn(move || {
            let mut emit = |_ev: SenEvent| {};
            bridge.run_turn("steer", "hi", &mut emit)
        });
        std::thread::sleep(Duration::from_millis(500));
        // Thread A: the production 3-step abort, lock released while
        // bridge.abort() waits out the grace.
        let reg_a = std::sync::Arc::clone(&reg);
        let abort_handle = std::thread::spawn(move || {
            let bridge = {
                let mut registry = reg_a.lock().unwrap();
                registry.begin_abort("s1", &contract()).unwrap()
            };
            let outcome = match bridge {
                Some(bridge) => bridge.abort(),
                None => AbortOutcome::NoChild,
            };
            let mut registry = reg_a.lock().unwrap();
            registry.complete_abort("s1", outcome)
        });
        std::thread::sleep(Duration::from_millis(100));
        // Thread B: create another session while the abort waits.
        let reg_b = std::sync::Arc::clone(&reg);
        let started = std::time::Instant::now();
        let created = std::thread::spawn(move || {
            let mut registry = reg_b.lock().unwrap();
            registry.create("s2", &contract())
        })
        .join();
        let elapsed_b = started.elapsed();
        assert!(created.is_ok(), "thread B panicked");
        assert_eq!(created.unwrap(), Ok(SessionState::Running));
        assert!(
            elapsed_b < Duration::from_millis(500),
            "R2B-07: create blocked {:?} while abort waited out the grace",
            elapsed_b
        );
        // The abort itself still completes: KilledAfterGrace, Cancelled.
        assert_eq!(abort_handle.join().unwrap(), Ok(SessionState::Cancelled));
        assert!(reg
            .lock()
            .unwrap()
            .get("s1")
            .unwrap()
            .log
            .contains(&"session_abort:kill-after-grace".to_string()));
        // The interrupted turn ends aborted.
        let err = handle.join().unwrap().unwrap_err();
        assert_eq!(err.code, "CLI_TURN_ABORTED");
    }

    // R2B-07: complete_abort re-checks the transition, because the
    // state may have changed while bridge.abort() ran lock-free. A
    // concurrent abort that cancels first wins; this abort's complete
    // step must refuse instead of writing a second log entry.
    #[test]
    fn complete_abort_refuses_when_session_already_cancelled() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        let bridge = reg.begin_abort("s1", &contract()).unwrap();
        assert!(bridge.is_some(), "begin_abort must hand out the bridge");
        // A concurrent abort wins the race and cancels first.
        assert_eq!(reg.abort("s1", &contract()), Ok(SessionState::Cancelled));
        let err = reg.complete_abort("s1", AbortOutcome::NoChild).unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
        assert_eq!(
            reg.get("s1").unwrap().log.len(),
            2,
            "refused complete_abort must write no log entry"
        );
    }

    #[test]
    fn error_wire_form_keeps_code() {
        let err = CommandError::new("SOME_CODE", "detail");
        assert_eq!(err.to_wire(), "SOME_CODE: detail");
    }

    // P0-3: a turn that fails with a retryable code (exit 2 = auth,
    // no result record) runs again on the next route in the chain and
    // succeeds there. The session record moves to the new route; the
    // fallback itself leaves no log entry (BUG-06: only a completed
    // steer is recorded), and the retryable failure itself is not
    // recorded as a completed steer.
    #[test]
    fn steer_retries_on_next_route_after_retryable_failure() {
        let marker = std::env::temp_dir().join(format!("sen_reg_p03_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // First invocation: auth failure, no result (exit 2).
        // Later invocations: the happy output.
        let body = format!(
            r#"if [ -f '{marker}' ]; then cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"assistant","uuid":"u1","session_id":"s1","message":{{"role":"assistant","content":"hello"}},"parent_tool_use_id":null,"v":1}}
{{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 0; else touch '{marker}'; cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
EOF
exit 2; fi"#,
            marker = marker.display()
        );
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("p03retry", &body));
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.route_id("s1").as_deref(), Some("spark"));
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(reg.route_id("s1").as_deref(), Some("openrouter-free"));
        assert!(marker.exists(), "fake binary must have run twice");
        // The failed attempt emitted its init event; the retry ran
        // the full happy turn (3 events). The turn recorded is the
        // successful one, and the log holds only session_create +
        // that turn (BUG-06: the failed attempt left no entry).
        assert_eq!(events.len(), 4);
        assert!(matches!(events[3], SenEvent::Result { .. }));
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        let _ = std::fs::remove_file(&marker);
    }

    // P2-2: the fallback loop stops after MAX_ATTEMPTS attempts even
    // when the router config changes mid-turn. The fake binary fails
    // every run with a retryable code (exit 2 = auth) and rewrites
    // the settings file on each run to drop the route that just ran
    // out of the chain — on the old code that made next_route restart
    // at the first route and the loop ping-ponged forever. The turn
    // must return the auth error after exactly 3 runs, one per route,
    // with no route repeated.
    #[test]
    fn steer_stops_after_max_attempts_when_config_changes_mid_turn() {
        let dir = std::env::temp_dir().join(format!("sen_reg_p22count_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let counter =
            std::env::temp_dir().join(format!("sen_reg_p22counter_{}", std::process::id()));
        let _ = std::fs::remove_file(&counter);
        // The live settings start as the default 3-route chain. Each
        // variant drops one route; the fake binary copies the variant
        // that drops the route it just ran.
        CliSettings {
            model: DEFAULT_MODEL.to_string(),
            base_url: None,
            router: Some(RouterConfig::default()),
        }
        .save(&dir)
        .unwrap();
        let variant = |tag: &str, drop_id: &str| -> std::path::PathBuf {
            let vdir =
                std::env::temp_dir().join(format!("sen_reg_p22var_{}_{}", tag, std::process::id()));
            let _ = std::fs::remove_dir_all(&vdir);
            std::fs::create_dir_all(&vdir).unwrap();
            let mut config = RouterConfig::default();
            config.routes.retain(|r| r.id != drop_id);
            CliSettings {
                model: DEFAULT_MODEL.to_string(),
                base_url: None,
                router: Some(config),
            }
            .save(&vdir)
            .unwrap();
            vdir
        };
        let no_spark = variant("nospark", "spark");
        let no_openrouter = variant("noopenrouter", "openrouter-free");
        let no_ollama = variant("noollama", "ollama");
        // The bridge passes --model <model> [--base-url <url>], so the
        // route is told apart by its base URL: ollama on 11434,
        // openrouter-free on openrouter.ai, spark with no base URL.
        // The echo lines go to the counter file, never to stdout, so
        // the bridge only ever sees the init record below.
        let body = format!(
            r#"case "$*" in *11434*) echo ollama >> '{counter}'; cp '{no_ollama}/cli-settings.json' '{dir}/cli-settings.json';; *openrouter.ai*) echo openrouter-free >> '{counter}'; cp '{no_openrouter}/cli-settings.json' '{dir}/cli-settings.json';; *) echo spark >> '{counter}'; cp '{no_spark}/cli-settings.json' '{dir}/cli-settings.json';; esac
cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
EOF
exit 2"#,
            counter = counter.display(),
            no_ollama = no_ollama.display(),
            no_openrouter = no_openrouter.display(),
            no_spark = no_spark.display(),
            dir = dir.display(),
        );
        let mut reg = Registry::with_settings_dir(dir.clone());
        reg.set_zen_bin(fake_zen("p22count", &body));
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.route_id("s1").as_deref(), Some("spark"));
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, "CLI_AUTH");
        let runs = std::fs::read_to_string(&counter).unwrap();
        let tried: Vec<&str> = runs.lines().collect();
        assert_eq!(
            tried,
            vec!["spark", "openrouter-free", "ollama"],
            "at most MAX_ATTEMPTS=3 runs, one per route, no repeat; got: {tried:?}"
        );
        assert_eq!(reg.route_id("s1").as_deref(), Some("ollama"));
        let _ = std::fs::remove_file(&counter);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&no_spark);
        let _ = std::fs::remove_dir_all(&no_openrouter);
        let _ = std::fs::remove_dir_all(&no_ollama);
    }

    // P2-2: the turn walks the attempt order snapshotted at its start,
    // not the config re-read mid-turn. The fake binary fails once on
    // spark (exit 2) and rewrites the settings mid-turn to a chain
    // that drops openrouter-free and prefers ollama; the retry must
    // still run on openrouter-free — the second route of the snapshot
    // — and succeed there. On the old code the re-read chain had no
    // route after spark, so the turn failed outright.
    #[test]
    fn steer_uses_the_order_snapshot_from_turn_start() {
        let dir = std::env::temp_dir().join(format!("sen_reg_p22snap_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        CliSettings {
            model: DEFAULT_MODEL.to_string(),
            base_url: None,
            router: Some(RouterConfig::default()),
        }
        .save(&dir)
        .unwrap();
        let vdir = std::env::temp_dir().join(format!("sen_reg_p22snapvar_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&vdir);
        std::fs::create_dir_all(&vdir).unwrap();
        let mut changed = RouterConfig::default();
        changed.routes.retain(|r| r.id != "openrouter-free");
        changed.preferred_route = Some("ollama".to_string());
        CliSettings {
            model: DEFAULT_MODEL.to_string(),
            base_url: None,
            router: Some(changed),
        }
        .save(&vdir)
        .unwrap();
        let marker =
            std::env::temp_dir().join(format!("sen_reg_p22snap_marker_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let body = format!(
            r#"if [ -f '{marker}' ]; then cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
{{"type":"assistant","uuid":"u1","session_id":"s1","message":{{"role":"assistant","content":"hello"}},"parent_tool_use_id":null,"v":1}}
{{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0}},"model":"m:free","session_id":"s1","v":1}}
EOF
exit 0; else touch '{marker}'; cp '{vdir}/cli-settings.json' '{dir}/cli-settings.json'; cat <<'EOF'
{{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}}
EOF
exit 2; fi"#,
            marker = marker.display(),
            vdir = vdir.display(),
            dir = dir.display(),
        );
        let mut reg = Registry::with_settings_dir(dir.clone());
        reg.set_zen_bin(fake_zen("p22snap", &body));
        reg.create("s1", &contract()).unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert!(marker.exists(), "fake binary must have run twice");
        assert_eq!(
            reg.route_id("s1").as_deref(),
            Some("openrouter-free"),
            "retry must use the snapshot order, not the rewritten config"
        );
        assert_eq!(events.len(), 4);
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&vdir);
    }

    // P0-3: a non-retryable failure (exit 4 = validation) is never
    // retried: the error returns at once and the route does not move.
    #[test]
    fn steer_does_not_retry_after_non_retryable_failure() {
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
EOF
exit 4"#;
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("p03noretry", body));
        reg.create("s1", &contract()).unwrap();
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, "CLI_VALIDATION");
        assert_eq!(reg.route_id("s1").as_deref(), Some("spark"));
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
    }

    // P0-3: create() resolves the preferred route from the stored
    // router, and a session with no stored router keeps the legacy
    // model/base_url as its implicit spark route.
    #[test]
    fn create_uses_the_preferred_route_from_the_router() {
        let dir = std::env::temp_dir().join(format!("sen_reg_p03pref_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = RouterConfig::default();
        config.preferred_route = Some("ollama".to_string());
        CliSettings {
            model: "some/model:free".to_string(),
            base_url: None,
            router: Some(config),
        }
        .save(&dir)
        .unwrap();
        let mut reg = Registry::with_settings_dir(dir.clone());
        reg.create("s1", &contract()).unwrap();
        let bridge = reg.bridge("s1").unwrap();
        assert_eq!(bridge.model(), "llama3.1");
        assert_eq!(bridge.base_url(), Some("http://127.0.0.1:11434/v1"));
        assert_eq!(reg.route_id("s1").as_deref(), Some("ollama"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- P2-5: retention caps, FIFO eviction ---

    /// A fabricated successful turn result: no model run, so the cap
    /// tests can complete 100+ steers without spawning 100+
    /// processes. Only complete_steer is exercised, never the bridge.
    fn fake_turn(session_id: &str, text: &str) -> TurnResult {
        TurnResult {
            kind: "result".to_string(),
            session_id: session_id.to_string(),
            model: "m:free".to_string(),
            subtype: "success".to_string(),
            is_error: false,
            text: text.to_string(),
            num_turns: 1,
            usage: crate::commands::stream_json::TurnUsage {
                input_tokens: 3,
                output_tokens: 2,
                cache_read_input_tokens: 0,
            },
            duration_ms: 7,
            exit_code: Some(0),
            exit_code_label: "ok".to_string(),
            records_seen: 3,
            unknown_records: 0,
            version_mismatches: 0,
        }
    }

    /// Drive one steer to a recorded result without running the
    /// model: begin (Run), then complete with the fabricated turn.
    /// A duplicate key would return Duplicate from begin_steer
    /// instead, so callers use a fresh key per steer.
    fn record_steer(reg: &mut Registry, session_id: &str, key_suffix: &str) {
        let mut c = contract();
        c.idempotency_key = format!("k-{key_suffix}");
        let text = format!("hello {key_suffix}");
        match reg.begin_steer(session_id, &c, "steer", &text).unwrap() {
            SteerStart::Run(_) => {}
            other => panic!("expected Run, got {other:?}"),
        }
        let turn = fake_turn(session_id, &text);
        reg.complete_steer(session_id, &c, "steer", &text, &turn)
            .unwrap();
    }

    #[test]
    fn create_evicts_oldest_session_over_the_cap() {
        assert_eq!(MAX_SESSIONS, 100);
        let mut reg = Registry::new();
        for i in 0..=MAX_SESSIONS {
            reg.create(&format!("s{i:03}"), &contract()).unwrap();
        }
        assert_eq!(reg.session_count(), MAX_SESSIONS);
        // Oldest (s000) evicted: no record, no bridge.
        assert!(reg.get("s000").is_none());
        assert!(reg.bridge("s000").is_none());
        // Newest still live.
        assert!(reg.get(&format!("s{MAX_SESSIONS:03}")).is_some());
        assert!(reg.bridge(&format!("s{MAX_SESSIONS:03}")).is_some());
        // Under the cap nothing is evicted.
        let mut small = Registry::new();
        for i in 0..MAX_SESSIONS {
            small.create(&format!("t{i:03}"), &contract()).unwrap();
        }
        assert_eq!(small.session_count(), MAX_SESSIONS);
        assert!(small.get("t000").is_some());
    }

    #[test]
    fn evicted_session_fails_loudly_and_can_be_recreated() {
        let mut reg = Registry::new();
        reg.create("s0", &contract()).unwrap();
        for i in 1..=MAX_SESSIONS {
            reg.create(&format!("s{i:03}"), &contract()).unwrap();
        }
        assert!(reg.get("s0").is_none());
        // A steer on the evicted session fails loudly, never silently.
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s0", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, ERR_UNKNOWN_SESSION);
        // The id is free again: recreating it is a fresh session,
        // not a duplicate.
        assert_eq!(reg.create("s0", &contract()), Ok(SessionState::Running));
        assert!(reg.get("s0").is_some());
    }

    #[test]
    fn complete_steer_evicts_oldest_completed_steer_over_the_cap() {
        assert_eq!(MAX_COMPLETED_STEERS_PER_SESSION, 100);
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        for i in 0..=MAX_COMPLETED_STEERS_PER_SESSION {
            record_steer(&mut reg, "s1", &format!("{i:03}"));
        }
        assert_eq!(
            reg.completed_steer_count("s1"),
            MAX_COMPLETED_STEERS_PER_SESSION
        );
        // The oldest recorded steer (key k-000) is gone: begin_steer
        // runs the model again instead of replaying a Duplicate.
        let mut c = contract();
        c.idempotency_key = "k-000".to_string();
        match reg.begin_steer("s1", &c, "steer", "hello 000").unwrap() {
            SteerStart::Run(_) => {}
            other => panic!("expected Run for evicted key, got {other:?}"),
        }
        // The newest recorded steer still replays.
        let mut c2 = contract();
        c2.idempotency_key = format!("k-{:03}", MAX_COMPLETED_STEERS_PER_SESSION);
        let text2 = format!("hello {:03}", MAX_COMPLETED_STEERS_PER_SESSION);
        match reg.begin_steer("s1", &c2, "steer", &text2).unwrap() {
            SteerStart::Duplicate { .. } => {}
            other => panic!("expected Duplicate for newest key, got {other:?}"),
        }
    }
}
