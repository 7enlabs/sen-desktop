// CLI spawn bridge (Phase 2, tin 9530 quyết 3 + 4).
// PURE RUST, no Tauri dependency: compiles and tests standalone.
//
// One CliBridge handle per session (created when the session is created,
// kept in the Registry). A handle owns the CLI process lifecycle for its
// session: at most one turn runs at a time, steer/followUp go to the
// session's process, and abort escalates SIGINT → 5s grace → SIGKILL.
//
// HOW A TURN RUNS (verified against the real CLI, rev 8ac73fe):
//   zen -p --output-format stream-json --bare --setting-sources ""
//       --model <model> [--base-url <url>] [--session-id <id> | -r <id>] -- <prompt>
// - `--setting-sources ""` + `--bare` keep project/user settings out of
//   the run, so a turn is reproducible from its explicit inputs.
// - The first turn of a handle starts a fresh CLI session named with
//   --session-id; every later turn resumes it with -r. The B2 bridge
//   verified that --session-id alone does NOT load history while -r <id>
//   sends the full history (system, prior prompts, prior replies).
// - The prompt is the last argv element, preceded by `--` so a prompt
//   starting with `-` (e.g. "- fix bug", "--help") is passed to the CLI
//   as the prompt, never parsed as a flag (Rust P1-1). Flags never
//   follow it. The key is NEVER in argv: the CLI reads ZEN_API_KEY from
//   and never logs the environment.
// - stream-json records on stdout are parsed + translated by
//   stream_json.rs and delivered to the caller through the `emit`
//   callback as they arrive. A `result` record closes the turn. Control
//   flow comes only from the result record (subtype / is_error); the
//   process exit code is recorded on the turn result and mapped to a B2
//   label for reporting only.
// - Turn timeout (Leader tin 9566 decision 1): a turn that produces no
//   result record within 120s is killed and reported as CLI_TURN_TIMEOUT,
//   so a turn that truly hangs (no result record at all) does not hang
//   silently until the user aborts. A turn with no API key is NOT such a
//   case: with the bridge's stdin null, the real CLI returns a 401 result
//   (is_error=true, exit 1) in ~1s (R2B-08). The earlier "no key hangs"
//   reading came from a probe with inherited stdin: CLI -p reads stdin
//   to EOF, so it waited on stdin, not on auth.
//   After the read loop ends (EOF) the turn waits for the child only up
//   to the 180s process-kill backstop (PROCESS_KILL_BACKSTOP, R2B-06),
//   then kills it, so a CLI that closed stdout but stays alive cannot
//   hang the turn forever — and the wait is additionally bounded by
//   the time left on the turn deadline (P2-B), so the whole turn
//   never runs past it. The IPC contract
//   timeout_ms is the per-turn deadline, capped at the 120s turn
//   timeout: the effective deadline is min(timeout_ms, 120s) (BUG-11
//   fix). A turn never runs longer than 120s, and a shorter contract
//   deadline ends a hanging turn earlier, as the contract promises.
//
// PERSISTENCE, STATED HONESTLY (the decision-3 limit, see phase2-PLAN.md):
// CLI 0.1.0 `-p` mode is one-shot — it reads stdin to EOF, runs one
// prompt, then exits (zen-cli lib.rs run_headless/read_stdin); the REPL
// emits text, not stream-json; --acp speaks JSON-RPC, not stream-json.
// So NO process stays alive between turns: each turn spawns a fresh
// process for the session, and the SESSION persists through the CLI
// session store + `-r`, not through a live process. The handle is
// persistent; the process is per-turn. If the Leader wants a process
// that literally stays alive between turns, the CLI needs a new
// persistent stream-json mode — outside this phase.
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::cli_bin::{exit_code_label, probe_cli_bin, resolve_zen_bin, CliBinError};
use super::stream_json::{
    parse_stream_json_line, snippet, translate_cli_record, CliRecord, SenEvent, TurnUsage,
    STREAM_JSON_VERSION,
};

/// Abort escalation grace: SIGINT, wait this long, then SIGKILL
/// (decision 3: 5s).
pub const ABORT_GRACE: Duration = Duration::from_secs(5);

/// Bound on `zen sessions delete` after an end (Q5): the delete is
/// a local file removal, so it finishes in milliseconds; a binary
/// that stays alive past this long is killed and the delete
/// reports failure (best effort: the end already succeeded).
pub const DELETE_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on stderr kept for error detail (stderr holds warnings and hook
/// diagnostics, never the stream; never log it whole).
pub const STDERR_TAIL_LEN: usize = 8192;

/// Keep only the last `max` bytes of a stderr tail, cutting on a char
/// boundary (P2-1): `String::drain(..cut)` panics when `cut` lands
/// inside a multi-byte character, and stderr often holds non-ASCII
/// text. The cut is advanced to the next char boundary, so the kept
/// tail is at most `max` bytes and never splits a character.
fn trim_tail_to_len(tail: &mut String, max: usize) {
    if tail.len() > max {
        let mut cut = tail.len() - max;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

/// Turn timeout (Leader tin 9566 decision 1): a turn that produces no
/// result record within this long is killed and reported as
/// CLI_TURN_TIMEOUT, so a genuinely hung turn (no result record at all)
/// does not hang silently until the user aborts. A turn with no API key
/// is not the hanging case: with stdin null the real CLI returns a 401
/// result (is_error=true, exit 1) in ~1s (R2B-08).
/// Under the 180s process-kill backstop (PROCESS_KILL_BACKSTOP, R2B-06):
/// after the read loop ends (EOF) the wait for the child is bounded,
/// then the child is SIGKILLed, so a CLI that closed stdout but stays
/// alive cannot hang the turn forever. This is
/// also the CAP for a per-turn contract timeout_ms (BUG-11): the
/// effective turn deadline is min(timeout_ms, TURN_TIMEOUT), so a turn
/// never runs longer than 120s.
pub const TURN_TIMEOUT: Duration = Duration::from_secs(120);

/// Process-kill backstop (R2B-06): after the read loop ends (EOF), the
/// turn waits at most this long for the child to exit before SIGKILLing
/// it and reaping it. Before R2B-06 this 180s value only existed in a
/// comment while the code called an unbounded `child.wait()` after EOF,
/// so a CLI that closed stdout but stayed alive hung the turn forever
/// (and left `turn_in_flight` stuck, refusing later turns with
/// CLI_TURN_IN_PROGRESS).
pub const PROCESS_KILL_BACKSTOP: Duration = Duration::from_secs(180);

/// Grace after a `result` record (R2B-09): once the result is in hand
/// the turn waits at most this long for the child to exit on its own
/// (the real CLI exits at once, so its exit code is still recorded),
/// then SIGKILLs it. Output after the result record is meaningless:
/// the result is the last record per the stream-json v1 contract.
pub const RESULT_EXIT_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}

impl BridgeError {
    pub fn new(code: &str, message: &str) -> BridgeError {
        BridgeError {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

impl From<CliBinError> for BridgeError {
    fn from(e: CliBinError) -> BridgeError {
        BridgeError::new(&e.code, &e.message)
    }
}

pub const ERR_CLI_SPAWN_FAILED: &str = "CLI_SPAWN_FAILED";
pub const ERR_CLI_TURN_IN_PROGRESS: &str = "CLI_TURN_IN_PROGRESS";
pub const ERR_CLI_INVALID_RECORD: &str = "CLI_INVALID_RECORD";
pub const ERR_CLI_NO_RESULT: &str = "CLI_NO_RESULT";
/// Typed no-result codes (R2B-03): a turn that ends with no result
/// record maps the CLI exit code to one of these instead of the shared
/// CLI_NO_RESULT, so the frontend can tell auth apart from other
/// failures by code. Exit 1 stays CLI_NO_RESULT; 130/143 and unknown
/// codes also stay CLI_NO_RESULT. Note: CLI_NOT_FOUND here is the
/// exit-3 (resource not found) code, distinct from cli_bin's
/// ERR_CLI_NOT_FOUND which reports a missing binary at resolve time.
pub const ERR_CLI_AUTH: &str = "CLI_AUTH";
pub const ERR_CLI_NOT_FOUND: &str = "CLI_NOT_FOUND";
pub const ERR_CLI_VALIDATION: &str = "CLI_VALIDATION";
pub const ERR_CLI_FORBIDDEN: &str = "CLI_FORBIDDEN";
pub const ERR_CLI_RATE_LIMIT: &str = "CLI_RATE_LIMIT";
pub const ERR_CLI_CONFLICT: &str = "CLI_CONFLICT";
pub const ERR_CLI_TRUNCATED: &str = "CLI_TRUNCATED";
pub const ERR_CLI_TURN_ABORTED: &str = "CLI_TURN_ABORTED";
pub const ERR_CLI_TURN_TIMEOUT: &str = "CLI_TURN_TIMEOUT";
pub const ERR_CLI_EMPTY_TEXT: &str = "CLI_EMPTY_TEXT";
/// `zen sessions delete` failed or never finished (Q5): the CLI
/// session on disk may still exist — or never existed, since a
/// session that never ran a turn has no file and the CLI exits 3.
/// The end already succeeded, so the caller treats this as best
/// effort and never fails the end because of it.
pub const ERR_CLI_SESSION_DELETE_FAILED: &str = "CLI_SESSION_DELETE_FAILED";

/// Map a no-result turn's CLI exit code to a typed error code
/// (R2B-03). The exit-code table in cli_bin.rs is reporting-only, so
/// this is the control-flow mapping: exit 2 auth, 3 not found,
/// 4 validation, 5 forbidden, 6 rate limit, 7 conflict. Exit 1
/// stays CLI_NO_RESULT (the existing no_result_record_fails_loudly
/// test pins it), and 130/143, unknown codes, and a missing code
/// (killed by signal) also stay CLI_NO_RESULT.
pub fn no_result_error_code(exit_code: Option<i32>) -> &'static str {
    match exit_code {
        Some(2) => ERR_CLI_AUTH,
        Some(3) => ERR_CLI_NOT_FOUND,
        Some(4) => ERR_CLI_VALIDATION,
        Some(5) => ERR_CLI_FORBIDDEN,
        Some(6) => ERR_CLI_RATE_LIMIT,
        Some(7) => ERR_CLI_CONFLICT,
        _ => ERR_CLI_NO_RESULT,
    }
}

/// Build the exact CLI argv for one turn. The prompt is the last argv
/// element, preceded by `--` so a prompt starting with `-` is passed as
/// the prompt value, never parsed as a CLI flag (Rust P1-1: without the
/// separator, "- fix bug" fails clap parsing with exit 4 and "--help"
/// prints CLI help, both with no result record). Flags never follow it.
/// `resume` = false starts a fresh CLI session named --session-id <id>;
/// true resumes it with -r <id>.
/// `base_url` (R2B-05) appends `--base-url <url>` after --model when
/// Some, and passes no flag when None (the CLI uses its own default
/// endpoint). No key appears anywhere in argv: the CLI reads
/// ZEN_API_KEY from the inherited environment.
pub fn build_cli_argv(
    model: &str,
    base_url: Option<&str>,
    session_id: &str,
    resume: bool,
    text: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--bare".to_string(),
        "--setting-sources".to_string(),
        String::new(),
        "--model".to_string(),
        model.to_string(),
    ];
    if let Some(url) = base_url {
        argv.push("--base-url".to_string());
        argv.push(url.to_string());
    }
    if resume {
        argv.push("-r".to_string());
        argv.push(session_id.to_string());
    } else {
        argv.push("--session-id".to_string());
        argv.push(session_id.to_string());
    }
    // Rust P1-1: `--` ends flag parsing, so the prompt that follows is
    // always the positional prompt, even when it starts with `-`.
    argv.push("--".to_string());
    argv.push(text.to_string());
    argv
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnResult {
    pub kind: String,
    pub session_id: String,
    pub model: String,
    pub subtype: String,
    pub is_error: bool,
    pub text: String,
    pub num_turns: u64,
    pub usage: TurnUsage,
    pub duration_ms: u64,
    /// Process exit code, reporting only. None when the child was taken
    /// by abort (its exit is owned by the abort path).
    pub exit_code: Option<i32>,
    pub exit_code_label: String,
    pub records_seen: u64,
    pub unknown_records: u64,
    pub version_mismatches: u64,
}

impl TurnResult {
    /// Rebuild the Result event this turn emitted live (CI P2-2). A
    /// cached TurnResult carries every field of the Result event
    /// (session, subtype, error flag, text, turns, usage, duration,
    /// model), so a Duplicate replay can re-deliver an event identical
    /// to the first delivery — no field is invented, and no
    /// intermediate event (SessionStart, Message) is replayed.
    pub fn result_event(&self) -> SenEvent {
        SenEvent::Result {
            session_id: self.session_id.clone(),
            subtype: self.subtype.clone(),
            is_error: self.is_error,
            text: self.text.clone(),
            num_turns: self.num_turns,
            usage: self.usage.clone(),
            duration_ms: self.duration_ms,
            model: self.model.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortOutcome {
    /// No child was running; nothing to kill.
    NoChild,
    /// The child exited after SIGINT, within the grace period.
    ExitedAfterSigint,
    /// The child ignored SIGINT and was killed after the grace period.
    KilledAfterGrace,
}

struct BridgeState {
    /// True once this handle has seen a CLI record: the CLI session
    /// exists on disk from that point, so later turns resume with -r.
    started: bool,
    /// True once the binary probe passed; the probe runs once.
    probed: bool,
    turn_in_flight: bool,
    /// Set by abort(); a turn that sees it ends as CLI_TURN_ABORTED.
    aborted: bool,
}

/// One CLI bridge handle per session. Interior mutability: run_turn must
/// not hold any lock across the whole turn, or abort (a different Tauri
/// command) could never reach the child. The child itself lives in
/// `child_slot`, shared between run_turn and abort.
pub struct CliBridge {
    /// Explicit binary path when set (tests, operator override). None
    /// means resolve per decision 1 at first turn: ZEN_BIN, then PATH.
    bin_override: Mutex<Option<PathBuf>>,
    session_id: String,
    model: String,
    /// Optional provider base URL (R2B-05): passed as --base-url on
    /// every turn when Some, no flag when None.
    base_url: Option<String>,
    state: Mutex<BridgeState>,
    child_slot: Arc<Mutex<Option<Child>>>,
    abort_grace: Duration,
    /// Turn timeout (Leader tin 9566 decision 1, default 120s): a turn
    /// that produces no result record within this long is killed and
    /// reported as CLI_TURN_TIMEOUT.
    turn_timeout: Duration,
    /// Bound on `zen sessions delete` after an end (Q5, default
    /// DELETE_TIMEOUT): a delete that never finishes is killed and
    /// reported as a failure instead of hanging the end call.
    delete_timeout: Duration,
    /// Process-kill backstop (R2B-06, default 180s): bound on waiting
    /// for the child after the read loop ends (EOF) before SIGKILLing it.
    process_kill_backstop: Duration,
}

/// Clears turn_in_flight when the turn ends on any path.
struct TurnGuard<'a> {
    bridge: &'a CliBridge,
}
impl<'a> Drop for TurnGuard<'a> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.bridge.state.lock() {
            state.turn_in_flight = false;
        }
    }
}

impl std::fmt::Debug for CliBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let started = self.state.lock().map(|s| s.started).unwrap_or(false);
        f.debug_struct("CliBridge")
            .field("session_id", &self.session_id)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("started", &started)
            .finish()
    }
}

impl CliBridge {
    /// Create a handle. Spawns no process and makes no model call.
    /// `bin_override` is Some only for tests / an explicit operator
    /// path; production passes None and resolves per decision 1.
    /// `base_url` is the stored provider base URL (R2B-05): Some
    /// passes --base-url on every turn, None passes no flag.
    pub fn new(
        bin_override: Option<PathBuf>,
        session_id: &str,
        model: &str,
        base_url: Option<&str>,
    ) -> CliBridge {
        CliBridge {
            bin_override: Mutex::new(bin_override),
            session_id: session_id.to_string(),
            model: model.to_string(),
            base_url: base_url.map(str::to_string),
            state: Mutex::new(BridgeState {
                started: false,
                probed: false,
                turn_in_flight: false,
                aborted: false,
            }),
            child_slot: Arc::new(Mutex::new(None)),
            abort_grace: ABORT_GRACE,
            turn_timeout: TURN_TIMEOUT,
            delete_timeout: DELETE_TIMEOUT,
            process_kill_backstop: PROCESS_KILL_BACKSTOP,
        }
    }

    /// Test-only: shorten the abort grace so the SIGKILL escalation can
    /// be proven without waiting the full 5s.
    #[cfg(test)]
    pub fn with_abort_grace(mut self, grace: Duration) -> CliBridge {
        self.abort_grace = grace;
        self
    }

    /// Test-only: shorten the process-kill backstop so the bounded
    /// wait after EOF can be proven without waiting the full 180s.
    #[cfg(test)]
    pub fn with_process_kill_backstop(mut self, backstop: Duration) -> CliBridge {
        self.process_kill_backstop = backstop;
        self
    }

    /// Test-only: shorten the turn timeout so the timeout path can be
    /// proven without waiting the full 120s.
    #[cfg(test)]
    pub fn with_turn_timeout(mut self, timeout: Duration) -> CliBridge {
        self.turn_timeout = timeout;
        self
    }

    /// Test-only: shorten the session-delete timeout so the kill path
    /// can be proven without waiting the full DELETE_TIMEOUT.
    #[cfg(test)]
    pub fn with_delete_timeout(mut self, timeout: Duration) -> CliBridge {
        self.delete_timeout = timeout;
        self
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// True once a turn on this handle ran to a result record: the CLI
    /// session exists on disk from that point, so a later turn (or a
    /// fallback bridge for the same session id) may resume it with -r.
    pub fn has_started(&self) -> bool {
        self.state.lock().map(|s| s.started).unwrap_or(false)
    }

    /// Test-only: whether abort() was ever called on this handle.
    /// sen_session_end must never call abort (it refuses while a
    /// turn runs instead of killing it), so the end tests assert
    /// this stays false after an end.
    #[cfg(test)]
    pub fn was_aborted(&self) -> bool {
        self.state.lock().map(|s| s.aborted).unwrap_or(false)
    }

    /// Mark this handle as resuming an existing CLI session (P0-3
    /// fallback): the next turn uses -r instead of --session-id, so
    /// the turn keeps the session history the failed bridge already
    /// built. Only call when the previous bridge has_started().
    pub fn mark_session_started(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.started = true;
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// The stored provider base URL, if any (R2B-05).
    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// Resolve the binary for this handle: the override when set, else
    /// decision 1 (ZEN_BIN, then PATH).
    pub fn resolve_bin(&self) -> Result<PathBuf, BridgeError> {
        if let Ok(guard) = self.bin_override.lock() {
            if let Some(path) = guard.as_ref() {
                return Ok(path.clone());
            }
        }
        resolve_zen_bin().map_err(BridgeError::from)
    }

    /// Run one turn ("steer" or "followUp") on this session's process.
    /// One turn at a time per handle: a second steer while one is in
    /// flight refuses with CLI_TURN_IN_PROGRESS. Events are delivered
    /// through `emit` as they arrive; the turn result comes from the
    /// CLI's `result` record, never from the exit code. The deadline is
    /// the bridge turn timeout (120s by Leader decision).
    pub fn run_turn(
        &self,
        kind: &str,
        text: &str,
        emit: &mut dyn FnMut(SenEvent),
    ) -> Result<TurnResult, BridgeError> {
        self.run_turn_with_timeout(kind, text, self.turn_timeout, emit)
    }

    /// Run one turn with a per-turn deadline from the IPC contract
    /// (BUG-11): the effective deadline is min(timeout, turn_timeout),
    /// so the contract timeout_ms bounds the turn but never extends it
    /// past the 120s turn timeout (Leader tin 9566 decision 1). A turn
    /// that produces no result record within the effective deadline is
    /// killed and reported as CLI_TURN_TIMEOUT.
    pub fn run_turn_with_timeout(
        &self,
        kind: &str,
        text: &str,
        timeout: Duration,
        emit: &mut dyn FnMut(SenEvent),
    ) -> Result<TurnResult, BridgeError> {
        self.run_turn_with_argv(
            kind,
            text,
            timeout,
            |bridge, resume| {
                build_cli_argv(
                    &bridge.model,
                    bridge.base_url.as_deref(),
                    &bridge.session_id,
                    resume,
                    text,
                )
            },
            emit,
        )
    }

    /// Run one turn with a caller-built argv (MCP delegate, Tauri
    /// follow-up 1): the same spawn/read/wait loop as a model turn,
    /// only the argv differs (--mcp-config + the delegate prompt, see
    /// mcp_delegate.rs). `text` is the prompt (the last argv element)
    /// and is only used for the empty check; the argv itself is used
    /// verbatim, so a delegate turn never re-derives flags from the
    /// bridge's session.
    pub fn run_turn_with_custom_argv(
        &self,
        kind: &str,
        text: &str,
        timeout: Duration,
        argv: Vec<String>,
        emit: &mut dyn FnMut(SenEvent),
    ) -> Result<TurnResult, BridgeError> {
        self.run_turn_with_argv(kind, text, timeout, |_, _| argv, emit)
    }

    /// Shared turn loop for run_turn_with_timeout and
    /// run_turn_with_custom_argv: the only difference is how the argv
    /// is built, so the spawn, stream-json read, timeout, abort, and
    /// exit-code handling stay in exactly one place. `build_argv`
    /// receives the bridge and the resume flag (true once a result
    /// record proved the CLI session exists on disk) and returns the
    /// full argv, prompt last.
    fn run_turn_with_argv(
        &self,
        kind: &str,
        text: &str,
        timeout: Duration,
        build_argv: impl FnOnce(&CliBridge, bool) -> Vec<String>,
        emit: &mut dyn FnMut(SenEvent),
    ) -> Result<TurnResult, BridgeError> {
        if text.trim().is_empty() {
            return Err(BridgeError::new(
                ERR_CLI_EMPTY_TEXT,
                "No prompt text provided: refusing to run a CLI turn with an empty prompt.",
            ));
        }
        let (resume, probed) = {
            let mut state = self.state.lock().map_err(|_| {
                BridgeError::new(ERR_CLI_SPAWN_FAILED, "Bridge state lock poisoned, FAIL")
            })?;
            if state.turn_in_flight {
                return Err(BridgeError::new(
                    ERR_CLI_TURN_IN_PROGRESS,
                    "Turn already in progress: one CLI run at a time per bridge session.",
                ));
            }
            if state.aborted {
                return Err(BridgeError::new(
                    ERR_CLI_TURN_ABORTED,
                    "Turn aborted before it started.",
                ));
            }
            state.turn_in_flight = true;
            state.aborted = false;
            (state.started, state.probed)
        };
        let _guard = TurnGuard { bridge: self };

        // Turn deadline: the effective deadline is min(timeout,
        // turn_timeout) (BUG-11), counting from here so a slow
        // probe/spawn counts too. The 120s cap never moves.
        let effective_timeout = timeout.min(self.turn_timeout);
        let deadline = std::time::Instant::now() + effective_timeout;

        let bin = self.resolve_bin()?;
        if !probed {
            probe_cli_bin(&bin)?;
            if let Ok(mut state) = self.state.lock() {
                state.probed = true;
            }
        }
        if self.is_aborted() {
            return Err(BridgeError::new(
                ERR_CLI_TURN_ABORTED,
                "Turn aborted before spawn.",
            ));
        }

        let argv = build_argv(self, resume);
        let mut child = match Command::new(&bin)
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                return Err(BridgeError::new(
                    ERR_CLI_SPAWN_FAILED,
                    &format!("Failed to spawn the CLI binary: {} ({})", bin.display(), e),
                ));
            }
        };

        // stderr tail on a helper thread (warnings/diagnostics only).
        let stderr_tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        {
            let tail = Arc::clone(&stderr_tail);
            let mut stderr = child.stderr.take();
            std::thread::spawn(move || {
                use std::io::Read;
                if let Some(err) = stderr.as_mut() {
                    let mut buf = [0u8; 4096];
                    loop {
                        match err.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Ok(mut guard) = tail.lock() {
                                    guard.push_str(&String::from_utf8_lossy(&buf[..n]));
                                    trim_tail_to_len(&mut guard, STDERR_TAIL_LEN);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            });
        }

        let stdout = match child.stdout.take() {
            Some(out) => out,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BridgeError::new(
                    ERR_CLI_SPAWN_FAILED,
                    "Failed to capture the CLI stdout pipe.",
                ));
            }
        };
        {
            let mut slot = self.child_slot.lock().map_err(|_| {
                BridgeError::new(
                    ERR_CLI_SPAWN_FAILED,
                    "Bridge child slot lock poisoned, FAIL",
                )
            })?;
            *slot = Some(child);
        }
        // Abort may have landed between spawn and storing the child.
        if self.is_aborted() {
            self.kill_stored_child();
            return Err(BridgeError::new(
                ERR_CLI_TURN_ABORTED,
                "Turn aborted right after spawn.",
            ));
        }

        // Stdout is read on a helper thread and sent over a channel, so
        // the turn loop can wait on it with the remaining time instead
        // of blocking forever on a hung CLI (one that produces no result
        // record at all; a no-key turn is not such a case — with stdin
        // null it returns a 401 result in ~1s, R2B-08). EOF is the
        // sender going away.
        let (line_tx, line_rx) = std::sync::mpsc::channel::<Result<String, String>>();
        {
            let mut reader = BufReader::new(stdout);
            std::thread::spawn(move || {
                let mut line = String::new();
                loop {
                    match reader.read_line(&mut line) {
                        Ok(0) => break, // EOF: the child closed stdout.
                        Ok(_) => {
                            if line_tx.send(Ok(std::mem::take(&mut line))).is_err() {
                                break; // Turn ended; nobody reads anymore.
                            }
                        }
                        Err(e) => {
                            let _ = line_tx.send(Err(e.to_string()));
                            break;
                        }
                    }
                }
            });
        }

        let mut result_record: Option<CliRecord> = None;
        let mut broke_on_result = false;
        let mut records_seen: u64 = 0;
        let mut unknown_records: u64 = 0;
        let mut version_mismatches: u64 = 0;
        let mut turn_error: Option<BridgeError> = None;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match line_rx.recv_timeout(remaining) {
                Ok(Ok(line)) => {
                    let has_newline = line.ends_with('\n');
                    let record = match parse_stream_json_line(&line) {
                        Ok(record) => record,
                        Err(message) => {
                            turn_error = Some(BridgeError::new(ERR_CLI_INVALID_RECORD, &message));
                            break;
                        }
                    };
                    let Some(record) = record else {
                        continue;
                    };
                    records_seen += 1;
                    // R2B-04: the contract says `v` = 1 on every
                    // record, so a record with no `v` field at all
                    // (None) is a version mismatch too, not just a
                    // record with `v` != 1.
                    if record.v != Some(STREAM_JSON_VERSION) {
                        version_mismatches += 1;
                    }
                    if record.record_type == "result" {
                        result_record = Some(record.clone());
                    }
                    let event = translate_cli_record(&record);
                    if matches!(event, SenEvent::Unknown { .. }) {
                        unknown_records += 1;
                    }
                    emit(event);
                    if record.record_type == "result" {
                        // R2B-09: the result record is the last record
                        // per the stream-json v1 contract, so the turn
                        // is done — return at once instead of waiting
                        // for EOF or the remaining deadline while the
                        // CLI lingers. The bounded wait below reaps
                        // the child (SIGKILL after RESULT_EXIT_GRACE).
                        broke_on_result = true;
                        break;
                    }
                    if !has_newline && result_record.is_none() {
                        // A trailing partial line without a newline means
                        // the output was cut off (the CLI always ends
                        // records with a newline). It fails the turn only
                        // when no result record arrived.
                        turn_error = Some(BridgeError::new(
                            ERR_CLI_TRUNCATED,
                            &format!(
                                "Truncated stream-json output: last line has no newline: {}",
                                snippet(&line, 200)
                            ),
                        ));
                        break;
                    }
                }
                Ok(Err(message)) => {
                    turn_error = Some(BridgeError::new(
                        ERR_CLI_INVALID_RECORD,
                        &format!("Failed to read the CLI stdout: {}", message),
                    ));
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // No line arrived within the remaining time. Kill the
                    // child (SIGKILL — there is no partial result worth
                    // a SIGINT grace, and a SIGINT could make the CLI
                    // write an "Interrupted by user" result that would
                    // mask the timeout).
                    self.kill_stored_child();
                    if result_record.is_some() {
                        // The result already arrived; the process just
                        // lingered. End the turn with the result
                        // instead of reporting a timeout.
                        break;
                    }
                    return Err(BridgeError::new(
                        ERR_CLI_TURN_TIMEOUT,
                        &format!(
                            "CLI turn timed out after {:?} with no result record: the CLI produced no result record at all. (A missing API key is not the cause: with stdin null a no-key turn returns a 401 result in ~1s.)",
                            effective_timeout
                        ),
                    ));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break, // EOF.
            }
        }
        if let Some(err) = turn_error {
            self.kill_stored_child();
            return Err(err);
        }

        // Take the child back to wait for its exit code. Abort may have
        // taken it already; then the exit code is unknown to this path.
        // Rust P1-2: take the child out of the slot and drop the lock
        // BEFORE waiting for it. Holding `child_slot` across the wait
        // (up to the backstop in `wait_child_after_eof`) blocked `abort()`,
        // which needs the same lock to reach the child — exactly when
        // abort exists to be used. Only the `Child` is kept, never
        // the lock, on both wait paths below.
        let child = {
            let mut slot = self.child_slot.lock().map_err(|_| {
                BridgeError::new(
                    ERR_CLI_SPAWN_FAILED,
                    "Bridge child slot lock poisoned, FAIL",
                )
            })?;
            slot.take()
        };
        let exit_code = match child {
            // R2B-09 + R2B-06 (merged): the wait after the read
            // loop is bounded on both paths. With a result record
            // in hand, poll for a quick exit up to RESULT_EXIT_GRACE
            // (500ms, keeps the real exit code), then SIGKILL and
            // reap. Without a result record, go through the
            // process-kill backstop (wait_child_after_eof), bounded
            // by the time left on the turn deadline (P2-B) — never
            // an unbounded child.wait(), or R2B-06 would lose its
            // effect exactly in the lingering-child case R2B-09
            // fixes.
            Some(mut child) => {
                if broke_on_result {
                    let deadline = std::time::Instant::now() + RESULT_EXIT_GRACE;
                    loop {
                        match child.try_wait() {
                            Ok(Some(status)) => break status.code(),
                            Ok(None) => {
                                // P1-2: abort() cannot take this child
                                // any more (the turn owns it now), but it
                                // sets the aborted flag first — notice it
                                // here and kill the child at once instead
                                // of polling out the grace.
                                if self.is_aborted() {
                                    let _ = child.kill();
                                    break child.wait().ok().and_then(|status| status.code());
                                }
                                if std::time::Instant::now() >= deadline {
                                    let _ = child.kill();
                                    break child.wait().ok().and_then(|status| status.code());
                                }
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => break None,
                        }
                    }
                } else {
                    self.wait_child_after_eof(&mut child, deadline)
                }
            }
            None => None,
        };

        let Some(result) = result_record else {
            if self.is_aborted() {
                return Err(BridgeError::new(
                    ERR_CLI_TURN_ABORTED,
                    "CLI turn ended by abort with no result record.",
                ));
            }
            let stderr = stderr_tail.lock().map(|g| g.clone()).unwrap_or_default();
            let detail = stderr
                .trim()
                .chars()
                .rev()
                .take(500)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(" stderr: {}", detail)
            };
            return Err(BridgeError::new(
                no_result_error_code(exit_code),
                &format!(
                    "CLI turn ended with no result record (exited {}).{}",
                    exit_code_label(exit_code),
                    detail
                ),
            ));
        };

        // The first parsed result proves the CLI session exists on disk,
        // so later turns of this handle resume with -r.
        if let Ok(mut state) = self.state.lock() {
            state.started = true;
        }
        let event = translate_cli_record(&result);
        let SenEvent::Result {
            subtype,
            is_error,
            text,
            num_turns,
            usage,
            duration_ms,
            model,
            ..
        } = event
        else {
            return Err(BridgeError::new(
                ERR_CLI_INVALID_RECORD,
                "CLI turn ended with a malformed result record.",
            ));
        };
        Ok(TurnResult {
            kind: kind.to_string(),
            session_id: self.session_id.clone(),
            model: if model.is_empty() {
                self.model.clone()
            } else {
                model
            },
            subtype,
            is_error,
            text,
            num_turns,
            usage,
            duration_ms,
            exit_code,
            exit_code_label: exit_code_label(exit_code).to_string(),
            records_seen,
            unknown_records,
            version_mismatches,
        })
    }

    /// Wait for the child after the read loop ended (EOF), bounded by
    /// the process-kill backstop (R2B-06) AND by the time left on the
    /// turn deadline (P2-B), whichever is sooner: a CLI that closed
    /// stdout but stays alive must not hang the turn forever on
    /// `child.wait()`, and must not stretch the turn past the
    /// deadline the turn promised (min(contract timeout, 120s)).
    /// Before P2-B the wait used the backstop alone, so a turn could
    /// run to ~300s (120s of output + 180s of waiting) — past the
    /// frontend's 120s invoke timeout and 130s result waiter, with
    /// the next turn failing as CLI_TURN_IN_PROGRESS until the
    /// lingering turn finally ended. Polls `try_wait()` every 50ms
    /// (same cadence as the abort poll); a child that already exited
    /// returns at once with no waiting. Past the deadline the child
    /// is SIGKILLed and reaped, and the turn fails as CLI_NO_RESULT
    /// with the exit code (or lack of one) recorded. An abort
    /// requested while waiting is noticed within one poll (P1-2):
    /// the turn owns the child now, so abort() signals through the
    /// aborted flag and the wait kills the child at once. Returns
    /// the exit code, or None when the child was killed (signal death
    /// has no code) or the code is unknown.
    fn wait_child_after_eof(
        &self,
        child: &mut Child,
        turn_deadline: std::time::Instant,
    ) -> Option<i32> {
        let deadline = std::cmp::min(
            turn_deadline,
            std::time::Instant::now() + self.process_kill_backstop,
        );
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) => {
                    // P1-2: same as the result-grace poll — the turn
                    // owns the child now, so abort() reaches it only
                    // through the aborted flag. Kill at once when the
                    // flag is set instead of waiting out the backstop.
                    if self.is_aborted() {
                        let _ = child.kill();
                        return match child.wait() {
                            Ok(status) => status.code(),
                            Err(_) => None,
                        };
                    }
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        return match child.wait() {
                            Ok(status) => status.code(),
                            Err(_) => None,
                        };
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
    }

    fn is_aborted(&self) -> bool {
        self.state.lock().map(|s| s.aborted).unwrap_or(false)
    }

    /// SIGKILL the stored child, if any (used when a turn fails mid-read
    /// so no child is left running).
    fn kill_stored_child(&self) {
        // Rust P1-2: take the child out of the slot and drop the lock
        // before kill+wait, so abort() is never blocked behind this
        // helper either (same lock, same reason as the exit-code wait).
        let child = {
            let mut slot = match self.child_slot.lock() {
                Ok(slot) => slot,
                Err(_) => return,
            };
            slot.take()
        };
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Abort the running turn, if any: SIGINT first, so the CLI can still
    /// write its interrupted result record ("Interrupted by user", exit
    /// 130); SIGKILL after the grace period if the process does not exit.
    /// With no turn running, returns NoChild at once. Also marks the
    /// turn aborted, so a turn that is between spawn and store still ends
    /// as CLI_TURN_ABORTED instead of running on. When the turn already
    /// took the child out of the slot for its exit wait, abort() returns
    /// NoChild at once — but the wait notices the aborted flag within
    /// one poll and kills the child, so the turn still ends promptly
    /// (P1-2, instead of waiting out the backstop).
    pub fn abort(&self) -> AbortOutcome {
        {
            if let Ok(mut state) = self.state.lock() {
                state.aborted = true;
            }
        }
        let mut child = {
            let mut slot = match self.child_slot.lock() {
                Ok(slot) => slot,
                Err(_) => return AbortOutcome::NoChild,
            };
            match slot.take() {
                Some(child) => child,
                None => return AbortOutcome::NoChild,
            }
        };
        let pid = child.id();
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(pid as i32, libc::SIGINT);
            }
        }
        #[cfg(not(unix))]
        {
            // Windows has no SIGINT for a child process through std: the
            // only signal available is kill (TerminateProcess). The
            // escalation is Unix-only; this path is NOT RUN on Windows.
            let _ = pid;
            let _ = child.kill();
            let _ = child.wait();
            return AbortOutcome::KilledAfterGrace;
        }
        #[cfg(unix)]
        {
            let deadline = std::time::Instant::now() + self.abort_grace;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return AbortOutcome::ExitedAfterSigint,
                    Ok(None) => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            return AbortOutcome::KilledAfterGrace;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => return AbortOutcome::ExitedAfterSigint,
                }
            }
        }
    }

    /// Delete this session's on-disk CLI transcript (Q5): runs
    /// `zen sessions delete <id> --yes` with the same binary the
    /// turns use (resolve_bin: override, else ZEN_BIN then PATH),
    /// stdin/stdout/stderr discarded, waited on for at most
    /// delete_timeout (DELETE_TIMEOUT in production), then killed.
    /// The CLI removes `~/.zen/projects/<cwd>/<id>.jsonl` plus the
    /// `<id>` scratch dir and exits 3 when no such session exists
    /// (zencore session_mgmt.rs delete_session, commit c87769a) —
    /// so a session that never ran a turn has nothing to delete and
    /// still reports an error here, which the caller must treat as
    /// best effort: the end already succeeded and must never fail
    /// because the disk cleanup did not. Never called while a turn
    /// runs: the caller (sen_session_end) runs it after
    /// Registry::end, with no registry lock held, like
    /// bridge.abort() in sen_session_abort. The bridge handle stays
    /// in the Registry map for diagnostics; only the on-disk
    /// transcript goes. The session id is passed as one argv
    /// element, never through a shell, so no id can inject flags.
    pub fn delete_cli_session(&self) -> Result<(), BridgeError> {
        let bin = self.resolve_bin()?;
        let mut child = Command::new(&bin)
            .args(["sessions", "delete", &self.session_id, "--yes"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                BridgeError::new(
                    ERR_CLI_SESSION_DELETE_FAILED,
                    &format!(
                        "Failed to run zen sessions delete for {}: {}",
                        self.session_id, e
                    ),
                )
            })?;
        let deadline = std::time::Instant::now() + self.delete_timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(());
                    }
                    let code = status
                        .code()
                        .map_or("signal".to_string(), |c| c.to_string());
                    return Err(BridgeError::new(
                        ERR_CLI_SESSION_DELETE_FAILED,
                        &format!(
                            "zen sessions delete for {} exited with {}",
                            self.session_id, code
                        ),
                    ));
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(BridgeError::new(
                            ERR_CLI_SESSION_DELETE_FAILED,
                            &format!(
                                "zen sessions delete for {} timed out after {:?}",
                                self.session_id, self.delete_timeout
                            ),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(BridgeError::new(
                        ERR_CLI_SESSION_DELETE_FAILED,
                        &format!(
                            "Failed to wait for zen sessions delete for {}: {}",
                            self.session_id, e
                        ),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cli_bin::ERR_CLI_VERSION_MISMATCH;
    use super::*;

    /// A fake `zen` binary: prints "zen 0.1.0" for --version, else the
    /// canned stream-json turn on stdout. The script body is passed in so
    /// each test controls the CLI behaviour (happy path, unknown record,
    /// SIGINT trap, ...).
    fn fake_zen(tag: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sen_fake_zen_{}_{}", tag, std::process::id()));
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

    /// A fake `zen` binary running an arbitrary shell body (Q5): used
    /// for delete_cli_session, where the behaviour under test is the
    /// argv itself, not a turn. Returns the script path.
    fn script_zen(tag: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sen_del_zen_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zen");
        std::fs::write(&path, format!("#!/bin/sh\n{}\n", body)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// A fake `zen` binary that records its argv (one line per call)
    /// to a marker file, then exits with the given code. Returns
    /// (script, marker). Proves delete_cli_session invokes the exact
    /// CLI delete command.
    fn recording_zen(tag: &str, exit_code: i32) -> (PathBuf, PathBuf) {
        let marker =
            std::env::temp_dir().join(format!("sen_del_argv_{}_{}.txt", tag, std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let body = format!("echo \"$@\" >> '{}'\nexit {}", marker.display(), exit_code);
        (script_zen(tag, &body), marker)
    }

    /// A session id in production form: the CLI validates
    /// --session-id as a UUID, and `zen sessions delete` takes the
    /// same id to find `<id>.jsonl` on disk.
    const DELETE_SESSION_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

    #[test]
    fn delete_cli_session_runs_zen_sessions_delete_with_yes() {
        // Q5: end deletes the on-disk CLI session by running exactly
        // `zen sessions delete <id> --yes` — the subcommand the real
        // CLI exposes (zencore cli.rs SessionSubcommand::Delete,
        // c87769a). --yes skips the interactive confirm prompt, so
        // the call never blocks on stdin.
        let (bin, marker) = recording_zen("delsess", 0);
        let bridge = CliBridge::new(Some(bin), DELETE_SESSION_ID, "m:free", None);
        assert_eq!(bridge.delete_cli_session(), Ok(()));
        let recorded = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            recorded.trim(),
            format!("sessions delete {} --yes", DELETE_SESSION_ID)
        );
    }

    #[test]
    fn delete_cli_session_reports_nonzero_exit() {
        // Q5: the CLI exits 3 when no session file exists on disk
        // (a session that never ran a turn). The helper reports it
        // as an error; the sen_session_end wrapper treats it as best
        // effort and never fails the end because of it.
        let (bin, marker) = recording_zen("delsess3", 3);
        let bridge = CliBridge::new(Some(bin), DELETE_SESSION_ID, "m:free", None);
        let err = bridge.delete_cli_session().unwrap_err();
        assert_eq!(err.code, ERR_CLI_SESSION_DELETE_FAILED);
        assert!(err.message.contains("3"), "message: {}", err.message);
        assert!(std::fs::read_to_string(&marker)
            .unwrap()
            .contains("sessions delete"));
    }

    #[test]
    fn delete_cli_session_reports_missing_binary() {
        // Q5: no binary at the resolved path is a delete failure,
        // not a panic — the wrapper ignores it and the end stands.
        let bridge = CliBridge::new(
            Some(PathBuf::from("/nonexistent/zen")),
            DELETE_SESSION_ID,
            "m:free",
            None,
        );
        let err = bridge.delete_cli_session().unwrap_err();
        assert_eq!(err.code, ERR_CLI_SESSION_DELETE_FAILED);
    }

    #[test]
    fn delete_cli_session_kills_a_hung_binary() {
        // Q5: a delete that never finishes is killed after the
        // (test-shortened) timeout instead of hanging the end call.
        let bin = script_zen("delhang", "sleep 30");
        let bridge = CliBridge::new(Some(bin), DELETE_SESSION_ID, "m:free", None)
            .with_delete_timeout(Duration::from_millis(50));
        let err = bridge.delete_cli_session().unwrap_err();
        assert_eq!(err.code, ERR_CLI_SESSION_DELETE_FAILED);
        assert!(
            err.message.contains("timed out"),
            "message: {}",
            err.message
        );
    }

    /// Run a turn against a fake binary and return (result, events).
    fn run_collect(
        bin: PathBuf,
        kind: &str,
        text: &str,
    ) -> (Result<TurnResult, BridgeError>, Vec<SenEvent>) {
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None);
        let events: Arc<Mutex<Vec<SenEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let mut emit = move |ev: SenEvent| {
            sink.lock().unwrap().push(ev);
        };
        let result = bridge.run_turn(kind, text, &mut emit);
        let events = events.lock().unwrap().clone();
        (result, events)
    }

    const HAPPY_BODY: &str = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","cwd":"/tmp","model":"m:free","tools":[],"mcp_servers":[],"permissionMode":"default","slash_commands":[],"v":1}
{"type":"assistant","uuid":"u1","session_id":"s1","message":{"role":"assistant","content":"hello"},"parent_tool_use_id":null,"v":1}
{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0},"model":"m:free","session_id":"s1","v":1}
EOF
exit 0"#;

    #[test]
    fn build_argv_first_turn_uses_session_id_later_resume() {
        let first = build_cli_argv("m:free", None, "s1", false, "hi");
        assert_eq!(
            first,
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--bare",
                "--setting-sources",
                "",
                "--model",
                "m:free",
                "--session-id",
                "s1",
                "--",
                "hi",
            ]
        );
        let later = build_cli_argv("m:free", None, "s1", true, "again");
        assert!(later.contains(&"-r".to_string()));
        assert!(later.contains(&"s1".to_string()));
        assert!(!later.contains(&"--session-id".to_string()));
        assert_eq!(later.last().unwrap(), "again");
        // No key anywhere in argv.
        assert!(!first.iter().any(|a| a.contains("KEY")));
    }

    // P2-1: trimming the stderr tail must never panic on non-ASCII
    // text. The bug-hunt repro ("ế" x 3000, 3 bytes/char) put the old
    // byte cut at 808, and 808 % 3 == 1, so `drain(..808)` panicked
    // with "assertion failed: self.is_char_boundary(end)".
    #[test]
    fn stderr_tail_trim_never_splits_utf8_char() {
        let mut tail = "ế".repeat(3000); // 9000 bytes, 3000 chars
        trim_tail_to_len(&mut tail, STDERR_TAIL_LEN);
        // Old code cut at byte 808 (9000 - 8192), inside a char.
        // The fix advances to the next boundary (810), keeping
        // 8190 bytes = 2730 whole chars, all "ế".
        assert_eq!(tail.len(), 8190);
        assert_eq!(tail.chars().count(), 2730);
        assert!(tail.chars().all(|c| c == 'ế'));
        assert!(tail.is_char_boundary(0));
    }

    #[test]
    fn stderr_tail_trim_ascii_and_short_inputs() {
        // ASCII cut lands on a boundary already: exact max kept.
        let mut tail = "x".repeat(9000);
        trim_tail_to_len(&mut tail, STDERR_TAIL_LEN);
        assert_eq!(tail.len(), STDERR_TAIL_LEN);
        // At or under the cap: untouched.
        let mut short = "ế".repeat(100);
        trim_tail_to_len(&mut short, STDERR_TAIL_LEN);
        assert_eq!(short.chars().count(), 100);
        let mut exact = "a".repeat(STDERR_TAIL_LEN);
        trim_tail_to_len(&mut exact, STDERR_TAIL_LEN);
        assert_eq!(exact.len(), STDERR_TAIL_LEN);
        // Mixed content: tail keeps whole chars only, never over max.
        let mut mixed = "ab".repeat(3000) + &"ế".repeat(1000);
        trim_tail_to_len(&mut mixed, STDERR_TAIL_LEN);
        assert!(mixed.len() <= STDERR_TAIL_LEN);
        assert!(mixed.ends_with('ế'));
    }

    // R2B-05: a stored base URL is passed as --base-url after --model
    // and before --session-id/-r; None passes no --base-url flag.
    #[test]
    fn build_argv_passes_base_url_after_model_before_session() {
        let url = "https://example.invalid/v1";
        let first = build_cli_argv("m:free", Some(url), "s1", false, "hi");
        assert_eq!(
            first,
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--bare",
                "--setting-sources",
                "",
                "--model",
                "m:free",
                "--base-url",
                url,
                "--session-id",
                "s1",
                "--",
                "hi",
            ]
        );
        let later = build_cli_argv("m:free", Some(url), "s1", true, "again");
        let base_pos = later.iter().position(|a| a == "--base-url").unwrap();
        let resume_pos = later.iter().position(|a| a == "-r").unwrap();
        assert!(base_pos < resume_pos, "--base-url must precede -r");
        assert_eq!(later[base_pos + 1], url);
        assert_eq!(later.last().unwrap(), "again");
        // None passes no flag at all.
        let none = build_cli_argv("m:free", None, "s1", false, "hi");
        assert!(!none.contains(&"--base-url".to_string()));
    }

    // Rust P1-1: a prompt starting with `-` must travel after `--`,
    // so the CLI parses it as the prompt, never as a flag. Without the
    // separator, "- fix bug" fails clap parsing (exit 4) and "--help"
    // prints CLI help (exit 0); both produce no result record, so the
    // bridge reports the turn failed. Verified live against the real
    // CLI binary: with `--`, both prompts reach the model path and the
    // CLI emits a `result` record.
    #[test]
    fn build_argv_separates_hyphen_prompt_with_double_dash() {
        for prompt in ["- fix bug", "--help", "-h", "--version", "hi"] {
            let argv = build_cli_argv("m:free", None, "s1", false, prompt);
            // `--` sits immediately before the prompt, which stays last.
            let sep = argv.iter().position(|a| a == "--").unwrap();
            assert_eq!(argv[sep + 1], prompt);
            assert_eq!(argv.last().unwrap(), prompt);
            // Nothing after `--` can be mistaken for a flag: the only
            // element past the separator is the prompt itself.
            assert_eq!(argv.len(), sep + 2);
        }
        // The separator is present on resume turns too.
        let later = build_cli_argv("m:free", None, "s1", true, "--help");
        let sep = later.iter().position(|a| a == "--").unwrap();
        assert_eq!(later[sep + 1], "--help");
        assert_eq!(later.last().unwrap(), "--help");
    }

    // R2B-05: a turn on a bridge created with a base URL really spawns
    // the CLI with --base-url <url> in argv (proven against a fake
    // binary that records its argv).
    #[test]
    fn turn_passes_base_url_to_the_cli_binary() {
        let marker = std::env::temp_dir().join(format!("sen_base_url_argv_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let body = format!(
            "printf '%s\\n' \"$@\" > '{marker}'\n{HAPPY_BODY}",
            marker = marker.display()
        );
        let bin = fake_zen("baseurl", &body);
        let bridge = CliBridge::new(
            Some(bin),
            "s1",
            "m:free",
            Some("https://example.invalid/v1"),
        );
        assert_eq!(bridge.base_url(), Some("https://example.invalid/v1"));
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let result = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        assert_eq!(result.subtype, "success");
        let argv = std::fs::read_to_string(&marker).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        let base_pos = argv.iter().position(|a| *a == "--base-url").unwrap();
        assert_eq!(argv[base_pos + 1], "https://example.invalid/v1");
        let model_pos = argv.iter().position(|a| *a == "--model").unwrap();
        let session_pos = argv.iter().position(|a| *a == "--session-id").unwrap();
        assert!(model_pos < base_pos && base_pos < session_pos);
        assert_eq!(argv.last().unwrap(), &"hi");
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn happy_turn_translates_events_and_result() {
        let bin = fake_zen("happy", HAPPY_BODY);
        let (result, events) = run_collect(bin, "steer", "hi");
        let result = result.unwrap();
        assert_eq!(result.kind, "steer");
        assert_eq!(result.subtype, "success");
        assert!(!result.is_error);
        assert_eq!(result.text, "hello");
        assert_eq!(result.num_turns, 1);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.records_seen, 3);
        assert_eq!(result.unknown_records, 0);
        assert_eq!(result.version_mismatches, 0);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], SenEvent::SessionStart { .. }));
        assert!(matches!(events[1], SenEvent::Message { .. }));
        assert!(matches!(events[2], SenEvent::Result { .. }));
    }

    // CI P2-2: a cached TurnResult must rebuild the exact Result event
    // the live turn emitted, so a Duplicate replay can re-deliver it
    // to a caller waiting on the event (chat.ts armResultWaiter).
    #[test]
    fn turn_result_replays_as_identical_result_event() {
        let (result, events) = run_collect(fake_zen("replay", HAPPY_BODY), "steer", "hi");
        let result = result.unwrap();
        let replayed = result.result_event();
        // Byte-identical to the Result event the live turn emitted.
        assert_eq!(replayed, events[2]);
        if let SenEvent::Result {
            session_id,
            subtype,
            is_error,
            text,
            num_turns,
            usage,
            duration_ms,
            model,
        } = &replayed
        {
            assert_eq!(session_id, "s1");
            assert_eq!(subtype, "success");
            assert!(!is_error);
            assert_eq!(text, "hello");
            assert_eq!(*num_turns, 1);
            assert_eq!(usage.input_tokens, 3);
            assert_eq!(usage.output_tokens, 2);
            assert_eq!(usage.cache_read_input_tokens, 0);
            assert_eq!(*duration_ms, 7);
            assert_eq!(model, "m:free");
        } else {
            panic!("replayed event must be a Result event");
        }
    }

    #[test]
    fn unknown_record_is_counted_not_fatal() {
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
{"type":"future_thing","subtype":"x","session_id":"s1","v":1}
{"type":"result","subtype":"success","is_error":false,"result":"ok","duration_ms":1,"num_turns":1,"usage":{},"model":"m:free","session_id":"s1","v":1}
EOF
exit 0"#;
        let bin = fake_zen("unknown", body);
        let (result, events) = run_collect(bin, "steer", "hi");
        let result = result.unwrap();
        assert_eq!(result.unknown_records, 1);
        assert_eq!(result.records_seen, 3);
        assert!(matches!(events[1], SenEvent::Unknown { .. }));
    }

    #[test]
    fn version_mismatch_is_counted_not_fatal() {
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":99}
{"type":"result","subtype":"success","is_error":false,"result":"ok","duration_ms":1,"num_turns":1,"usage":{},"model":"m:free","session_id":"s1","v":99}
EOF
exit 0"#;
        let bin = fake_zen("vmismatch", body);
        let (result, _events) = run_collect(bin, "steer", "hi");
        let result = result.unwrap();
        assert_eq!(result.version_mismatches, 2);
    }

    #[test]
    fn missing_version_field_is_counted_as_mismatch() {
        // R2B-04: the contract says `v` = 1 on every record, so a
        // record with no `v` field at all is a version mismatch too,
        // not just a record with `v` != 1.
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free"}
{"type":"result","subtype":"success","is_error":false,"result":"ok","duration_ms":1,"num_turns":1,"usage":{},"model":"m:free","session_id":"s1"}
EOF
exit 0"#;
        let bin = fake_zen("noversion", body);
        let (result, _events) = run_collect(bin, "steer", "hi");
        let result = result.unwrap();
        assert_eq!(result.version_mismatches, 2);
        // A record WITH v = 1 next to one without still counts only
        // the missing one.
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
{"type":"result","subtype":"success","is_error":false,"result":"ok","duration_ms":1,"num_turns":1,"usage":{},"model":"m:free","session_id":"s1"}
EOF
exit 0"#;
        let bin = fake_zen("oneversion", body);
        let (result, _events) = run_collect(bin, "steer", "hi");
        let result = result.unwrap();
        assert_eq!(result.version_mismatches, 1);
    }

    #[test]
    fn no_result_record_fails_loudly() {
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
EOF
exit 1"#;
        let bin = fake_zen("noresult", body);
        let (result, _events) = run_collect(bin, "steer", "hi");
        let err = result.unwrap_err();
        assert_eq!(err.code, ERR_CLI_NO_RESULT);
        assert!(err.message.contains("general error (1)"));
    }

    /// R2B-03: a no-result turn maps the CLI exit code to a typed error
    /// code instead of one shared CLI_NO_RESULT, so the frontend can
    /// tell auth apart from other failures by code. Each of these
    /// FAILS on head (every exit maps to CLI_NO_RESULT) and PASSES
    /// after the fix. Exit 1 stays CLI_NO_RESULT (covered by
    /// no_result_record_fails_loudly above); 130/143 and unknown codes
    /// also keep CLI_NO_RESULT.
    fn no_result_with_exit(tag: &str, exit_code: i32) -> BridgeError {
        let body = format!(
            "cat <<'EOF'\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\",\"model\":\"m:free\",\"v\":1}}\nEOF\nexit {}",
            exit_code
        );
        let bin = fake_zen(tag, &body);
        let (result, _events) = run_collect(bin, "steer", "hi");
        result.unwrap_err()
    }

    #[test]
    fn no_result_exit_2_is_typed_auth() {
        let err = no_result_with_exit("noresult2", 2);
        assert_eq!(err.code, ERR_CLI_AUTH);
        assert!(err.message.contains("auth (2)"));
    }

    #[test]
    fn no_result_exit_3_is_typed_not_found() {
        let err = no_result_with_exit("noresult3", 3);
        assert_eq!(err.code, ERR_CLI_NOT_FOUND);
        assert!(err.message.contains("not found (3)"));
    }

    #[test]
    fn no_result_exit_4_is_typed_validation() {
        let err = no_result_with_exit("noresult4", 4);
        assert_eq!(err.code, ERR_CLI_VALIDATION);
        assert!(err.message.contains("validation (4)"));
    }

    #[test]
    fn no_result_exit_5_is_typed_forbidden() {
        let err = no_result_with_exit("noresult5", 5);
        assert_eq!(err.code, ERR_CLI_FORBIDDEN);
        assert!(err.message.contains("forbidden (5)"));
    }

    #[test]
    fn no_result_exit_6_is_typed_rate_limit() {
        let err = no_result_with_exit("noresult6", 6);
        assert_eq!(err.code, ERR_CLI_RATE_LIMIT);
        assert!(err.message.contains("rate limit (6)"));
    }

    #[test]
    fn no_result_exit_7_is_typed_conflict() {
        let err = no_result_with_exit("noresult7", 7);
        assert_eq!(err.code, ERR_CLI_CONFLICT);
        assert!(err.message.contains("conflict (7)"));
    }

    #[test]
    fn no_result_exit_130_keeps_no_result_code() {
        let err = no_result_with_exit("noresult130", 130);
        assert_eq!(err.code, ERR_CLI_NO_RESULT);
        assert!(err.message.contains("SIGINT (130)"));
    }

    #[test]
    fn invalid_record_fails_loudly() {
        let body = "echo 'not json'; exit 0";
        let bin = fake_zen("invalid", body);
        let (result, _events) = run_collect(bin, "steer", "hi");
        let err = result.unwrap_err();
        assert_eq!(err.code, ERR_CLI_INVALID_RECORD);
    }

    #[test]
    fn empty_text_refuses_before_spawn() {
        let bin = fake_zen("empty", HAPPY_BODY);
        let (result, _events) = run_collect(bin, "steer", "   ");
        let err = result.unwrap_err();
        assert_eq!(err.code, ERR_CLI_EMPTY_TEXT);
    }

    #[test]
    fn abort_with_no_child_is_a_noop() {
        let bin = fake_zen("noop", HAPPY_BODY);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None);
        assert_eq!(bridge.abort(), AbortOutcome::NoChild);
    }

    #[test]
    fn abort_sigint_exits_a_trapped_child() {
        // Fake CLI: traps SIGINT, prints the interrupted result record,
        // exits 130 — the real CLI's SIGINT behaviour.
        let body = r#"trap 'printf "%s\n" "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"result\":\"Interrupted by user\",\"duration_ms\":1,\"num_turns\":0,\"usage\":{},\"model\":\"m:free\",\"session_id\":\"s1\",\"v\":1}"; exit 130' INT
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("sigint", body);
        let bridge = Arc::new(CliBridge::new(Some(bin), "s1", "m:free", None));
        let events: Arc<Mutex<Vec<SenEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let bridge_run = Arc::clone(&bridge);
        let handle = std::thread::spawn(move || {
            let mut emit = move |ev: SenEvent| {
                sink.lock().unwrap().push(ev);
            };
            bridge_run.run_turn("steer", "hi", &mut emit)
        });
        std::thread::sleep(Duration::from_millis(500));
        let outcome = bridge.abort();
        assert_eq!(outcome, AbortOutcome::ExitedAfterSigint);
        let result = handle.join().unwrap();
        // The trap printed the interrupted result record, so the turn
        // resolves with that result (is_error), not with a crash.
        let result = result.unwrap();
        assert!(result.is_error);
        assert_eq!(result.subtype, "error_during_execution");
    }

    #[test]
    fn abort_kills_a_child_that_ignores_sigint() {
        let body = r#"trap '' INT
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("ignoresigint", body);
        let bridge = Arc::new(
            CliBridge::new(Some(bin), "s1", "m:free", None)
                .with_abort_grace(Duration::from_millis(300)),
        );
        let bridge_run = Arc::clone(&bridge);
        let handle = std::thread::spawn(move || {
            let mut emit = move |_ev: SenEvent| {};
            bridge_run.run_turn("steer", "hi", &mut emit)
        });
        std::thread::sleep(Duration::from_millis(500));
        let outcome = bridge.abort();
        assert_eq!(outcome, AbortOutcome::KilledAfterGrace);
        let result = handle.join().unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.code, ERR_CLI_TURN_ABORTED);
    }

    #[test]
    fn abort_is_not_blocked_while_turn_waits_for_lingering_child() {
        // Rust P1-2: a CLI that closes stdout but stays alive puts
        // the turn into the backstop wait (wait_child_after_eof).
        // Before the fix the turn held the child_slot lock across
        // that wait, so abort() — which needs the same lock to reach
        // the child — hung until the backstop expired (up to 180s).
        // After the fix the lock is dropped before the wait, so abort
        // returns at once; and because the turn owns the child now,
        // abort signals through the aborted flag, which the wait
        // notices within one poll and kills the child — so the turn
        // itself must also end promptly, far inside the 5s test
        // backstop, instead of waiting it out. The abort outcome is
        // deliberately not asserted: it is NoChild while the turn
        // owns the child (the session still cancels, and the turn
        // still ends as CLI_TURN_ABORTED).
        let body = r#"trap '' INT
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
exec 1>&-
i=0; while [ "$i" -lt 100 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("lingeringeof", body);
        let bridge = Arc::new(
            CliBridge::new(Some(bin), "s1", "m:free", None)
                .with_abort_grace(Duration::from_millis(300))
                .with_process_kill_backstop(Duration::from_secs(5)),
        );
        let bridge_run = Arc::clone(&bridge);
        let turn_started = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            let mut emit = move |_ev: SenEvent| {};
            bridge_run.run_turn("steer", "hi", &mut emit)
        });
        // Let the turn read the init record, hit EOF, and enter the
        // backstop wait before aborting.
        std::thread::sleep(Duration::from_millis(500));
        let started = std::time::Instant::now();
        let _outcome = bridge.abort();
        let abort_elapsed = started.elapsed();
        // 5s is the whole backstop: abort must return far sooner.
        assert!(
            abort_elapsed < Duration::from_secs(2),
            "abort took {:?} while the turn waited on a lingering child",
            abort_elapsed
        );
        let result = handle.join().unwrap();
        // The turn must end promptly too, not wait out the backstop.
        assert!(
            turn_started.elapsed() < Duration::from_secs(3),
            "turn took {:?} after abort, backstop is 5s",
            turn_started.elapsed()
        );
        let err = result.unwrap_err();
        assert_eq!(err.code, ERR_CLI_TURN_ABORTED);
    }

    #[test]
    fn turn_times_out_when_cli_hangs() {
        // Fake CLI: prints init, then silence — a genuinely hung CLI
        // (no result record at all). This is NOT the real CLI's no-key
        // behaviour: with stdin null a no-key turn returns a 401 result
        // in ~1s (R2B-08). With a 300ms turn
        // timeout the turn must fail as CLI_TURN_TIMEOUT in well under
        // the 30s the fake would run.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("hangs", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_turn_timeout(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_CLI_TURN_TIMEOUT);
        assert!(err.message.contains("300ms"));
        // R2B-08: the timeout message must no longer blame a missing
        // API key for the hang.
        assert!(!err.message.contains("hang without reporting an error"));
        assert!(!err.message.contains("no API key can hang"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn run_turn_with_timeout_shorter_than_cap_cancels_turn() {
        // BUG-11: a per-turn timeout shorter than the 120s bridge cap
        // bounds the turn. The hanging fake must fail as
        // CLI_TURN_TIMEOUT in well under 10s, with the 300ms deadline
        // in the message.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("hangs3", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None);
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = bridge
            .run_turn_with_timeout("steer", "hi", Duration::from_millis(300), &mut emit)
            .unwrap_err();
        assert_eq!(err.code, ERR_CLI_TURN_TIMEOUT);
        assert!(err.message.contains("300ms"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn run_turn_with_timeout_longer_than_cap_is_capped() {
        // BUG-11: a per-turn timeout longer than the bridge cap never
        // extends the turn past the cap (Leader tin 9566: 120s). With
        // a 300ms test cap, a 3600s request still times out at 300ms.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("hangs4", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_turn_timeout(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = bridge
            .run_turn_with_timeout("steer", "hi", Duration::from_secs(3600), &mut emit)
            .unwrap_err();
        assert_eq!(err.code, ERR_CLI_TURN_TIMEOUT);
        assert!(err.message.contains("300ms"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn run_turn_with_timeout_fast_turn_still_completes() {
        // A turn that finishes quickly is not cut off by a short
        // per-turn timeout.
        let bin = fake_zen("fast2", HAPPY_BODY);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None);
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let result = bridge
            .run_turn_with_timeout("steer", "hi", Duration::from_millis(300), &mut emit)
            .unwrap();
        assert_eq!(result.subtype, "success");
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn fast_turn_completes_before_short_timeout() {
        // A turn that finishes quickly is not cut off by a short
        // timeout: the timeout only fires when no result arrives.
        let bin = fake_zen("fast", HAPPY_BODY);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_turn_timeout(Duration::from_secs(5));
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let result = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        assert_eq!(result.subtype, "success");
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn result_record_returns_without_waiting_for_child_exit() {
        // R2B-09: fake CLI prints a result record, then lingers 5s
        // before exiting. The turn must return with the result at
        // once (well under 2s), not wait for EOF / the child exit.
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
{"type":"result","subtype":"success","is_error":false,"result":"hello","duration_ms":7,"num_turns":1,"usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":0},"model":"m:free","session_id":"s1","v":1}
EOF
sleep 5
exit 0"#;
        let bin = fake_zen("lingers_after_result", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None);
        let started = std::time::Instant::now();
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let result = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        assert_eq!(result.subtype, "success");
        assert_eq!(result.text, "hello");
        assert_eq!(events.len(), 2);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn turn_can_run_again_after_timeout() {
        // After a timed-out turn the handle is not stuck: a second
        // run_turn proceeds (and times out again on this hanging fake)
        // instead of refusing with CLI_TURN_IN_PROGRESS.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("hangs2", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_turn_timeout(Duration::from_millis(300));
        let mut emit = |_ev: SenEvent| {};
        let first = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(first.code, ERR_CLI_TURN_TIMEOUT);
        let second = bridge
            .run_turn("follow_up", "again", &mut emit)
            .unwrap_err();
        assert_eq!(second.code, ERR_CLI_TURN_TIMEOUT);
    }

    #[test]
    fn turn_returns_when_cli_closes_stdout_but_stays_alive() {
        // R2B-06: fake CLI prints the result record, then closes
        // stdout (exec 1>&-) but stays alive 30s. The read loop ends
        // on EOF; the turn must return the result promptly once the
        // process-kill backstop fires, not hang on child.wait() until
        // the fake exits. With a 300ms test backstop the turn must
        // finish in well under 5s, with the result intact and the
        // child killed (no exit code: killed by signal).
        let body = r#"cat <<'EOF'
{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}
{"type":"result","subtype":"success","is_error":false,"result":"done","duration_ms":1,"num_turns":1,"usage":{},"model":"m:free","session_id":"s1","v":1}
EOF
exec 1>&-
sleep 30"#;
        let bin = fake_zen("eof_alive", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_process_kill_backstop(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let events: Arc<Mutex<Vec<SenEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let mut emit = move |ev: SenEvent| {
            sink.lock().unwrap().push(ev);
        };
        let result = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(result.subtype, "success");
        assert_eq!(result.text, "done");
        assert_eq!(result.exit_code, None);
        assert_eq!(events.lock().unwrap().len(), 2);
        // The handle is not stuck: a second turn proceeds instead of
        // refusing with CLI_TURN_IN_PROGRESS.
        let second = bridge.run_turn("follow_up", "again", &mut emit);
        assert!(second.is_ok() || second.unwrap_err().code != ERR_CLI_TURN_IN_PROGRESS);
    }

    #[test]
    fn eof_lingering_child_is_bounded_by_turn_deadline() {
        // P2-B: a CLI that closes stdout but stays alive, with no
        // result record, must end the turn at the turn deadline — not
        // after the 180s process-kill backstop. The fake prints init,
        // closes stdout at once, then lingers 30s. With a 300ms turn
        // timeout and a 5s test backstop the turn must fail as
        // CLI_NO_RESULT (the real error: output ended with no result)
        // in well under 2s. Before the fix the turn waited out the
        // whole 5s backstop, so the elapsed assertion fails.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
exec 1>&-
sleep 30"#;
        let bin = fake_zen("eof_linger_nodeadline", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_turn_timeout(Duration::from_millis(300))
            .with_process_kill_backstop(Duration::from_secs(5));
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_CLI_NO_RESULT);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "turn took {:?} with a 300ms deadline and a 5s backstop: the EOF wait ignored the turn deadline",
            started.elapsed()
        );
        // The handle is not stuck: a second turn proceeds instead of
        // refusing with CLI_TURN_IN_PROGRESS.
        let second = bridge.run_turn("follow_up", "again", &mut emit);
        assert!(second.is_ok() || second.unwrap_err().code != ERR_CLI_TURN_IN_PROGRESS);
    }

    #[test]
    fn eof_near_deadline_does_not_extend_turn_past_deadline() {
        // P2-B worst case: the CLI dribbles output until just before
        // the turn deadline, then closes stdout and lingers. The
        // backstop wait after EOF must be bounded by the time left on
        // the turn deadline, so the turn still ends at ~1s, not at
        // 200ms of output + the whole 5s backstop. Before the fix the
        // turn took ~5.2s, so the elapsed assertion fails. The 1s
        // deadline (vs 200ms of output) leaves a wide margin for a
        // slow spawn, so the test does not depend on scheduling luck.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
sleep 0.2
exec 1>&-
sleep 30"#;
        let bin = fake_zen("eof_linger_late", body);
        let bridge = CliBridge::new(Some(bin), "s1", "m:free", None)
            .with_turn_timeout(Duration::from_secs(1))
            .with_process_kill_backstop(Duration::from_secs(5));
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_CLI_NO_RESULT);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "turn took {:?} with a 1s deadline and a 5s backstop: the EOF wait extended the turn past its deadline",
            started.elapsed()
        );
    }

    #[test]
    fn probe_rejects_wrong_version_binary() {
        let dir =
            std::env::temp_dir().join(format!("sen_fake_zen_wrongver_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zen");
        std::fs::write(&path, "#!/bin/sh\necho \"zen 9.9.9\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bridge = CliBridge::new(Some(path), "s1", "m:free", None);
        let mut emit = |_ev: SenEvent| {};
        let err = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_CLI_VERSION_MISMATCH);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
