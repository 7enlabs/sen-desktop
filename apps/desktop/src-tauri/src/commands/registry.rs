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
//   Error -> Running | Failed (exactly 2 paths),
//   terminal states (Completed, Cancelled, Failed) have no outgoing path.
// - Every command validates the full IPC action contract first (same
//   required fields as apps/desktop/src/ipc/types.ts). A missing field
//   means FAIL, never a silent default.
// - Tool execution is NOT filled in here: the contract is validated and
//   the 6-step gate order is asserted, then the call fails loudly with
//   TOOL_EXECUTION_NOT_WIRED instead of pretending a tool ran. The gate
//   execute step and the permission layer are later phases.
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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::cli_settings::{CliSettings, DEFAULT_MODEL};
use super::cli_spawn::{AbortOutcome, BridgeError, CliBridge, TurnResult};
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
        Error => matches!(to, Running | Failed),
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

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub state: SessionState,
    pub log: Vec<String>,
    pub subscribers: u32,
}

#[derive(Default)]
pub struct Registry {
    sessions: HashMap<String, SessionRecord>,
    /// One CLI bridge handle per session (Phase 2). The handle is
    /// created with the session and outlives single turns; the process
    /// itself is per-turn (see cli_spawn.rs for why no process stays
    /// alive between turns).
    bridges: HashMap<String, Arc<CliBridge>>,
    /// Completed steers per session, for idempotency (BUG-10, #25):
    /// session id -> (steer key -> recorded turn result). Only turns
    /// that ran to a result land here, via complete_steer.
    completed_steers: HashMap<String, HashMap<SteerKey, TurnResult>>,
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
/// result without running the model, pushing a log entry, or emitting
/// events. InFlight (BUG-10 extra, tin 9645/9650) means the same steer
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
            bridges: HashMap::new(),
            completed_steers: HashMap::new(),
            in_flight_steers: HashMap::new(),
            settings_dir: None,
            zen_bin_override: None,
            #[cfg(test)]
            abort_grace_override: None,
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
    /// CLI bridge handle for the session (model and base URL from local
    /// settings, default free model and no base URL when nothing is
    /// stored). No model call happens here: the handle spawns nothing
    /// until the first steer.
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
        self.sessions.insert(
            session_id.to_string(),
            SessionRecord {
                id: session_id.to_string(),
                state: SessionState::Running,
                log: vec!["session_create".to_string()],
                subscribers: 0,
            },
        );
        self.bridges.insert(
            session_id.to_string(),
            Arc::new(self.new_bridge(session_id, &settings.model, settings.base_url.as_deref())),
        );
        Ok(SessionState::Running)
    }

    /// sen_session_steer / sen_session_follow_up, step 1 of 3: validate
    /// the contract, the text, and the Running gate, then either hand
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
        if record.state != SessionState::Running {
            return Err(CommandError::new(
                ERR_ILLEGAL_TRANSITION,
                &format!(
                    "Cannot steer a session in state {}, only Running accepts steer/follow-up",
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
    /// the session left Running meanwhile (abort won the race), the turn
    /// is NOT recorded and this refuses.
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
            if record.state != SessionState::Running {
                return Err(CommandError::new(
                    ERR_ILLEGAL_TRANSITION,
                    &format!(
                        "Session is {} after the turn, steer not recorded",
                        record.state.as_str()
                    ),
                ));
            }
            record.log.push(format!("steer:{}:{}", kind, turn.subtype));
            record.state
        };
        // R2B-02: an error turn (is_error=true) is not a completed
        // steer — do not cache it, so a retry with the same key runs
        // the model again instead of replaying the error as a
        // Duplicate. The log entry above already records that the
        // turn ran and what its subtype was.
        if !turn.is_error {
            self.completed_steers
                .entry(session_id.to_string())
                .or_default()
                .insert(SteerKey::new(contract, kind, text), turn.clone());
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
    /// duplicate steer returns the recorded result with no turn run and
    /// no events (BUG-10, #25); a steer already running elsewhere is
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
            SteerStart::Duplicate { state, .. } => Ok(state),
            SteerStart::InFlight { .. } => Err(CommandError::new(
                ERR_DUPLICATE_STEER,
                "Steer already running on this session, refused",
            )),
            SteerStart::Run(bridge) => {
                // BUG-11: the contract timeout_ms is the turn deadline, capped
                // at the 120s turn timeout inside the bridge (min(timeout_ms,
                // 120s) per tin 9566). A hanging turn is killed at that
                // deadline instead of running to the cap or hanging forever.
                let turn = match bridge.run_turn_with_timeout(
                    kind,
                    text,
                    Duration::from_millis(contract.timeout_ms),
                    emit,
                ) {
                    Ok(turn) => turn,
                    Err(e) => {
                        self.release_steer(session_id, contract, kind, text);
                        return Err(bridge_error_to_command(e));
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
    /// then Running or AwaitingApproval -> Cancelled, per the
    /// point-854 table, with the subscriber count reset to 0 and the
    /// log entry matching the AbortOutcome, exactly as the old
    /// one-call abort did. Error has no path to Cancelled (only
    /// Running and Failed), and terminal states have no outgoing path
    /// at all.
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

    /// sen_tool_call: validates the contract and the gate order, then
    /// fails loudly. The gate execute step and the permission layer are
    /// not filled in yet, so no tool runs for real in this phase.
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
        Err(CommandError::new(
            ERR_TOOL_EXECUTION_NOT_WIRED,
            "Tool execution NOT FILLED IN YET: gate execute step and permission layer are later phases, no tool ran.",
        ))
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

    #[test]
    fn statechart_point_854() {
        use SessionState::*;
        assert!(can_transition(Running, AwaitingApproval));
        assert!(can_transition(AwaitingApproval, Running));
        assert!(can_transition(AwaitingApproval, Cancelled));
        assert!(!can_transition(AwaitingApproval, Error));
        assert!(can_transition(Error, Running));
        assert!(can_transition(Error, Failed));
        assert!(!can_transition(Error, Cancelled));
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
    // replays the recorded result: no model run (no events), no extra
    // log entry, and begin_steer reports the recorded turn itself.
    #[test]
    fn steer_duplicate_key_replays_without_rerunning() {
        let mut reg = Registry::new();
        reg.set_zen_bin(fake_zen("dup", HAPPY_BODY));
        reg.create("s1", &contract()).unwrap();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 3);
        assert_eq!(reg.get("s1").unwrap().log.len(), 2);
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 0, "duplicate steer must emit no events");
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
        // Steer 1: the error turn. The backend ack is Ok(Running) -
        // the error is visible in the event stream, not as a failure.
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
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
        // returned the happy result), not replay the error.
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(
            events.len(),
            3,
            "retry after an error turn must run the model"
        );
        assert!(matches!(
            events[2],
            SenEvent::Result {
                is_error: false,
                ..
            }
        ));
        assert!(marker.exists(), "fake binary must have run a second time");
        assert_eq!(reg.get("s1").unwrap().log.len(), 3);
        // The successful retry IS cached: a third identical steer
        // replays it (BUG-10 behaviour, unchanged).
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        assert_eq!(
            reg.steer("s1", &contract(), "steer", "hi", &mut emit),
            Ok(SessionState::Running)
        );
        assert_eq!(events.len(), 0, "success turn must still be cached");
        let _ = std::fs::remove_file(&marker);
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

    // Only a turn that ran to a result is recorded: a failed first
    // steer records nothing, so the retry with the same key still runs.
    #[test]
    fn steer_failed_turn_does_not_record_key() {
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
        let mut emit = |_ev: SenEvent| {};
        let err = reg
            .steer("s1", &contract(), "steer", "hi", &mut emit)
            .unwrap_err();
        assert_eq!(err.code, "CLI_NO_RESULT");
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
        let err = reg.tool_call(&contract(), "my-tool").unwrap_err();
        assert_eq!(err.code, ERR_TOOL_EXECUTION_NOT_WIRED);
        let err = reg.tool_call(&contract(), "").unwrap_err();
        assert_eq!(err.code, ERR_INVALID_CONTRACT);
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
}
