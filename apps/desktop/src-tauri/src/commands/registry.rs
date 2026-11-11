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
// Idempotency (BUG-10, #25): the idempotency key is enforced, not just
// checked for presence. begin_steer returns Duplicate when the same
// (idempotency key, contract id, kind, text) already completed on this
// session, and the caller returns the recorded turn result WITHOUT
// running the model again: no new turn, no new log entry, no new events.
// Only a turn that ran to a result is recorded (complete_steer); a failed
// turn records nothing, so a retry after a failure still executes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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
    /// App config dir holding cli-settings.json (None in tests and when
    /// the app config dir is unavailable: the default model applies).
    settings_dir: Option<PathBuf>,
    /// Explicit binary path (tests / operator override). None means the
    /// bridge resolves per decision 1 at first turn (ZEN_BIN, then PATH).
    zen_bin_override: Option<PathBuf>,
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
/// events.
#[derive(Debug, Clone)]
pub enum SteerStart {
    Run(Arc<CliBridge>),
    Duplicate {
        state: SessionState,
        turn: TurnResult,
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
            settings_dir: None,
            zen_bin_override: None,
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

    pub fn get(&self, session_id: &str) -> Option<&SessionRecord> {
        self.sessions.get(session_id)
    }

    /// The session's CLI bridge handle, if the session exists.
    pub fn bridge(&self, session_id: &str) -> Option<Arc<CliBridge>> {
        self.bridges.get(session_id).cloned()
    }

    /// Model for a new session: the stored settings model when a
    /// settings dir is set, else the CLI's free default. Settings never
    /// fail session creation (a bad file falls back to the default).
    fn model_for_new_session(&self) -> String {
        match &self.settings_dir {
            Some(dir) => CliSettings::load(dir).model,
            None => DEFAULT_MODEL.to_string(),
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
    /// CLI bridge handle for the session (model from local settings,
    /// default free model when nothing is stored). No model call happens
    /// here: the handle spawns nothing until the first steer.
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
        let model = self.model_for_new_session();
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
            Arc::new(CliBridge::new(
                self.zen_bin_override.clone(),
                session_id,
                &model,
            )),
        );
        Ok(SessionState::Running)
    }

    /// sen_session_steer / sen_session_follow_up, step 1 of 3: validate
    /// the contract, the text, and the Running gate, then either hand
    /// out the session's bridge (Run) or, when this exact steer already
    /// completed on the session, return the recorded turn result
    /// (Duplicate) so the caller returns it without running the model
    /// again (BUG-10, #25). The caller runs the turn on the bridge
    /// WITHOUT holding the registry lock, then calls complete_steer.
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
        // Idempotency (BUG-10): same key + same contract + same text +
        // same kind already completed -> replay the recorded result,
        // never run the model again.
        if let Some(prior) = self
            .completed_steers
            .get(session_id)
            .and_then(|done| done.get(&SteerKey::new(contract, kind, text)))
        {
            return Ok(SteerStart::Duplicate {
                state,
                turn: prior.clone(),
            });
        }
        let bridge = self.bridges.get(session_id).cloned().ok_or_else(|| {
            CommandError::new(
                ERR_UNKNOWN_SESSION,
                &format!("Session has no CLI bridge: {}", session_id),
            )
        })?;
        Ok(SteerStart::Run(bridge))
    }

    /// sen_session_steer / sen_session_follow_up, step 3 of 3: record the
    /// turn AFTER the model ran. The log entry is written only here, so
    /// the backend never records a steer that did not run a model — the
    /// BUG-06 lesson. The completed turn is also stored under its steer
    /// key, so a later retry of the same key + contract + text replays
    /// this result instead of running again (BUG-10, #25). If the
    /// session left Running meanwhile (abort won the race), the turn is
    /// NOT recorded and this refuses.
    pub fn complete_steer(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
        kind: &str,
        text: &str,
        turn: &TurnResult,
    ) -> Result<SessionState, CommandError> {
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
        self.completed_steers
            .entry(session_id.to_string())
            .or_default()
            .insert(SteerKey::new(contract, kind, text), turn.clone());
        Ok(state)
    }

    /// Run a whole turn in one call (tests only): begin, turn, complete.
    /// Production wrappers use the three steps separately so the turn
    /// runs without the registry lock held. Events go to `emit`. A
    /// duplicate steer returns the recorded result with no turn run and
    /// no events (BUG-10, #25).
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
            SteerStart::Run(bridge) => {
                let turn = bridge
                    .run_turn(kind, text, emit)
                    .map_err(bridge_error_to_command)?;
                self.complete_steer(session_id, contract, kind, text, &turn)
            }
        }
    }

    /// sen_session_abort: escalate the running CLI turn first (SIGINT,
    /// 5s grace, SIGKILL — decision 3), then Running or
    /// AwaitingApproval -> Cancelled, per the point-854 table. Error has
    /// no path to Cancelled (only Running and Failed), and terminal
    /// states have no outgoing path at all. With no turn running the
    /// escalation is a no-op and this is a pure state transition.
    pub fn abort(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<SessionState, CommandError> {
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
        let outcome = match self.bridges.get(session_id) {
            Some(bridge) => bridge.abort(),
            None => AbortOutcome::NoChild,
        };
        let record = self.get_mut_or_unknown(session_id)?;
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
        }
        .save(&dir)
        .unwrap();
        let mut reg = Registry::with_settings_dir(dir.clone());
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.bridge("s1").unwrap().model(), "some/model:free");
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
            SteerStart::Run(_) => panic!("third identical steer must be Duplicate"),
        }
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

    #[test]
    fn error_wire_form_keeps_code() {
        let err = CommandError::new("SOME_CODE", "detail");
        assert_eq!(err.to_wire(), "SOME_CODE: detail");
    }
}
