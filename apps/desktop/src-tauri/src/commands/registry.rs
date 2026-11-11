// Backend session registry for the Tauri command handlers (bridge round).
// PURE RUST, no Tauri dependency: this module compiles and tests standalone
// with `rustc --test`, so the handler logic has real test evidence even on a
// box without the Tauri system libraries (gdk-3.0, webkit2gtk). The thin
// #[tauri::command] wrappers in session.rs, tools.rs and events.rs only
// convert wire args and delegate here; they hold no logic of their own.
//
// Design, per prep B-2 section 2 and the decided point-854 statechart:
// - The backend owns the command gate and the session record. The model
//   runs in the backend through the CLI core + stream-json bridge (a later
//   phase); until then steer/follow_up refuse with MODEL_NOT_WIRED instead
//   of acking a model call that never ran. The backend never calls the Pi
//   SDK and never holds a key: the SDK is out of the webview entirely
//   (tin 9328/9459), so no model runtime exists anywhere in this phase.
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

use std::collections::HashMap;

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
pub const ERR_MODEL_NOT_WIRED: &str = "MODEL_NOT_WIRED";

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

#[derive(Debug, Default)]
pub struct Registry {
    sessions: HashMap<String, SessionRecord>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry {
            sessions: HashMap::new(),
        }
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionRecord> {
        self.sessions.get(session_id)
    }

    fn get_mut_or_unknown(
        &mut self,
        session_id: &str,
    ) -> Result<&mut SessionRecord, CommandError> {
        self.sessions.get_mut(session_id).ok_or_else(|| {
            CommandError::new(
                ERR_UNKNOWN_SESSION,
                &format!("Session does not exist: {}", session_id),
            )
        })
    }

    /// sen_session_create: backend record only, state Running. No model
    /// call happens here or anywhere in the backend; the model runs
    /// through the CLI core + stream-json bridge, which lands in a later
    /// phase, so a created session has a record but no model runtime yet.
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
        self.sessions.insert(
            session_id.to_string(),
            SessionRecord {
                id: session_id.to_string(),
                state: SessionState::Running,
                log: vec!["session_create".to_string()],
                subscribers: 0,
            },
        );
        Ok(SessionState::Running)
    }

    /// sen_session_steer / sen_session_follow_up: the backend gates on a
    /// Running session, then refuses with MODEL_NOT_WIRED until the CLI
    /// bridge lands. The refusal happens BEFORE any log entry, so the
    /// backend never records a steer that did not run a model — the
    /// BUG-06 lesson (an ack must never imply a model call happened).
    /// Once the CLI bridge is wired, this refusal is replaced by the
    /// real model call and the log entry below records it.
    pub fn steer(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
        // `_kind` ("steer" / "follow_up") names the log entry the CLI
        // bridge will record once it runs the model; unused until then.
        _kind: &str,
        text: &str,
    ) -> Result<SessionState, CommandError> {
        validate_contract(contract)?;
        if text.is_empty() {
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
        // No model runtime exists in this phase (CLI bridge lands later):
        // refuse instead of acking. A steer that never ran a model must
        // not enter the log as if it had.
        return Err(CommandError::new(
            ERR_MODEL_NOT_WIRED,
            "Model runtime NOT WIRED YET: the CLI core + stream-json bridge lands in a later phase, no model call ran.",
        ));
    }

    /// sen_session_abort: Running or AwaitingApproval -> Cancelled, per the
    /// point-854 table. Error has no path to Cancelled (only Running and
    /// Failed), and terminal states have no outgoing path at all.
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
        record.state = SessionState::Cancelled;
        record.log.push("session_abort".to_string());
        Ok(record.state)
    }

    /// sen_events_subscribe: records one subscriber on the session. The
    /// event stream itself will flow from the backend (Tauri events) once
    /// the CLI bridge lands; this record is the backend bookkeeping for
    /// it, not the stream.
    pub fn subscribe(
        &mut self,
        session_id: &str,
        contract: &ActionContract,
    ) -> Result<u32, CommandError> {
        validate_contract(contract)?;
        let record = self.get_mut_or_unknown(session_id)?;
        record.subscribers += 1;
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
            for to in [Running, AwaitingApproval, Error, Completed, Cancelled, Failed] {
                assert!(!can_transition(terminal, to));
            }
        }
    }

    #[test]
    fn create_then_duplicate_then_unknown() {
        let mut reg = Registry::new();
        assert_eq!(reg.create("s1", &contract()), Ok(SessionState::Running));
        let dup = reg.create("s1", &contract()).unwrap_err();
        assert_eq!(dup.code, ERR_DUPLICATE_SESSION);
        let missing = reg.steer("nope", &contract(), "steer", "hi").unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
        let missing = reg.abort("nope", &contract()).unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
        let missing = reg.subscribe("nope", &contract()).unwrap_err();
        assert_eq!(missing.code, ERR_UNKNOWN_SESSION);
    }

    #[test]
    fn steer_requires_running_and_refuses_without_model() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        // No model runtime exists in this phase (CLI bridge lands later):
        // steer and follow_up refuse with MODEL_NOT_WIRED and the log
        // does not grow — a steer that never ran a model is never
        // recorded as if it had (BUG-06 lesson).
        let err = reg.steer("s1", &contract(), "steer", "hi").unwrap_err();
        assert_eq!(err.code, ERR_MODEL_NOT_WIRED);
        let err = reg
            .steer("s1", &contract(), "follow_up", "again")
            .unwrap_err();
        assert_eq!(err.code, ERR_MODEL_NOT_WIRED);
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
        assert_eq!(reg.get("s1").unwrap().state, SessionState::Running);
        // The refusals harmed nothing: the session still aborts cleanly,
        // and steering a terminal session is still ILLEGAL_TRANSITION
        // (the state check runs before the model check).
        reg.abort("s1", &contract()).unwrap();
        let err = reg.steer("s1", &contract(), "steer", "hi").unwrap_err();
        assert_eq!(err.code, ERR_ILLEGAL_TRANSITION);
    }

    #[test]
    fn steer_model_not_wired_leaves_no_log_entry() {
        let mut reg = Registry::new();
        reg.create("s1", &contract()).unwrap();
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
        let err = reg.steer("s1", &contract(), "steer", "hi").unwrap_err();
        assert_eq!(err.code, ERR_MODEL_NOT_WIRED);
        assert!(err.message.contains("CLI core"));
        assert_eq!(reg.get("s1").unwrap().log.len(), 1);
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

    #[test]
    fn error_wire_form_keeps_code() {
        let err = CommandError::new("SOME_CODE", "detail");
        assert_eq!(err.to_wire(), "SOME_CODE: detail");
    }
}
