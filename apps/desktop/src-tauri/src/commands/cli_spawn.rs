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
//       --model <model> [--session-id <id> | -r <id>] <prompt>
// - `--setting-sources ""` + `--bare` keep project/user settings out of
//   the run, so a turn is reproducible from its explicit inputs.
// - The first turn of a handle starts a fresh CLI session named with
//   --session-id; every later turn resumes it with -r. The B2 bridge
//   verified that --session-id alone does NOT load history while -r <id>
//   sends the full history (system, prior prompts, prior replies).
// - The prompt is the last argv element; flags never follow it. The key
//   is NEVER in argv: the CLI reads ZEN_API_KEY from the inherited
//   environment itself. The bridge never logs argv (it holds prompt text)
//   and never logs the environment.
// - stream-json records on stdout are parsed + translated by
//   stream_json.rs and delivered to the caller through the `emit`
//   callback as they arrive. A `result` record closes the turn. Control
//   flow comes only from the result record (subtype / is_error); the
//   process exit code is recorded on the turn result and mapped to a B2
//   label for reporting only.
// - Turn timeout (Leader tin 9566 decision 1): a turn that produces no
//   result record within 120s is killed and reported as CLI_TURN_TIMEOUT,
//   so a turn with no API key (the real CLI hangs after its init line
//   without an auth error) does not hang silently until the user aborts.
//   The 180s process-kill backstop is unchanged. The IPC contract
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

/// Cap on stderr kept for error detail (stderr holds warnings and hook
/// diagnostics, never the stream; never log it whole).
pub const STDERR_TAIL_LEN: usize = 8192;

/// Turn timeout (Leader tin 9566 decision 1): a turn that produces no
/// result record within this long is killed and reported as
/// CLI_TURN_TIMEOUT, so a turn with no API key (the real CLI hangs
/// without an auth error) does not hang silently until the user aborts.
/// Under the 180s process-kill backstop, which is unchanged. This is
/// also the CAP for a per-turn contract timeout_ms (BUG-11): the
/// effective turn deadline is min(timeout_ms, TURN_TIMEOUT), so a turn
/// never runs longer than 120s.
pub const TURN_TIMEOUT: Duration = Duration::from_secs(120);

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
pub const ERR_CLI_TRUNCATED: &str = "CLI_TRUNCATED";
pub const ERR_CLI_TURN_ABORTED: &str = "CLI_TURN_ABORTED";
pub const ERR_CLI_TURN_TIMEOUT: &str = "CLI_TURN_TIMEOUT";
pub const ERR_CLI_EMPTY_TEXT: &str = "CLI_EMPTY_TEXT";

/// Build the exact CLI argv for one turn. The prompt is the last argv
/// element; flags never follow it. `resume` = false starts a fresh CLI
/// session named --session-id <id>; true resumes it with -r <id>.
/// No key appears anywhere in argv: the CLI reads ZEN_API_KEY from the
/// inherited environment.
pub fn build_cli_argv(model: &str, session_id: &str, resume: bool, text: &str) -> Vec<String> {
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
    if resume {
        argv.push("-r".to_string());
        argv.push(session_id.to_string());
    } else {
        argv.push("--session-id".to_string());
        argv.push(session_id.to_string());
    }
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
    state: Mutex<BridgeState>,
    child_slot: Arc<Mutex<Option<Child>>>,
    abort_grace: Duration,
    /// Turn timeout (Leader tin 9566 decision 1, default 120s): a turn
    /// that produces no result record within this long is killed and
    /// reported as CLI_TURN_TIMEOUT.
    turn_timeout: Duration,
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
            .field("started", &started)
            .finish()
    }
}

impl CliBridge {
    /// Create a handle. Spawns no process and makes no model call.
    /// `bin_override` is Some only for tests / an explicit operator
    /// path; production passes None and resolves per decision 1.
    pub fn new(bin_override: Option<PathBuf>, session_id: &str, model: &str) -> CliBridge {
        CliBridge {
            bin_override: Mutex::new(bin_override),
            session_id: session_id.to_string(),
            model: model.to_string(),
            state: Mutex::new(BridgeState {
                started: false,
                probed: false,
                turn_in_flight: false,
                aborted: false,
            }),
            child_slot: Arc::new(Mutex::new(None)),
            abort_grace: ABORT_GRACE,
            turn_timeout: TURN_TIMEOUT,
        }
    }

    /// Test-only: shorten the abort grace so the SIGKILL escalation can
    /// be proven without waiting the full 5s.
    #[cfg(test)]
    pub fn with_abort_grace(mut self, grace: Duration) -> CliBridge {
        self.abort_grace = grace;
        self
    }

    /// Test-only: shorten the turn timeout so the timeout path can be
    /// proven without waiting the full 120s.
    #[cfg(test)]
    pub fn with_turn_timeout(mut self, timeout: Duration) -> CliBridge {
        self.turn_timeout = timeout;
        self
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn model(&self) -> &str {
        &self.model
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

        let argv = build_cli_argv(&self.model, &self.session_id, resume, text);
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
                                    if guard.len() > STDERR_TAIL_LEN {
                                        let cut = guard.len() - STDERR_TAIL_LEN;
                                        guard.drain(..cut);
                                    }
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
        // of blocking forever on a hung CLI (measured: no key → init
        // line only, then silence). EOF is the sender going away.
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
                    if record.v.is_some_and(|v| v != STREAM_JSON_VERSION) {
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
                            "CLI turn timed out after {:?} with no result record: the CLI produced no result (a turn with no API key can hang without reporting an error; check ZEN_API_KEY).",
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
        let exit_code = {
            let mut slot = self.child_slot.lock().map_err(|_| {
                BridgeError::new(
                    ERR_CLI_SPAWN_FAILED,
                    "Bridge child slot lock poisoned, FAIL",
                )
            })?;
            match slot.take() {
                Some(mut child) => match child.wait() {
                    Ok(status) => status.code(),
                    Err(_) => None,
                },
                None => None,
            }
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
                ERR_CLI_NO_RESULT,
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

    fn is_aborted(&self) -> bool {
        self.state.lock().map(|s| s.aborted).unwrap_or(false)
    }

    /// SIGKILL the stored child, if any (used when a turn fails mid-read
    /// so no child is left running).
    fn kill_stored_child(&self) {
        let mut slot = match self.child_slot.lock() {
            Ok(slot) => slot,
            Err(_) => return,
        };
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Abort the running turn, if any: SIGINT first, so the CLI can still
    /// write its interrupted result record ("Interrupted by user", exit
    /// 130); SIGKILL after the grace period if the process does not exit.
    /// With no turn running, returns NoChild at once. Also marks the
    /// turn aborted, so a turn that is between spawn and store still ends
    /// as CLI_TURN_ABORTED instead of running on.
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

    /// Run a turn against a fake binary and return (result, events).
    fn run_collect(
        bin: PathBuf,
        kind: &str,
        text: &str,
    ) -> (Result<TurnResult, BridgeError>, Vec<SenEvent>) {
        let bridge = CliBridge::new(Some(bin), "s1", "m:free");
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
        let first = build_cli_argv("m:free", "s1", false, "hi");
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
                "hi",
            ]
        );
        let later = build_cli_argv("m:free", "s1", true, "again");
        assert!(later.contains(&"-r".to_string()));
        assert!(later.contains(&"s1".to_string()));
        assert!(!later.contains(&"--session-id".to_string()));
        assert_eq!(later.last().unwrap(), "again");
        // No key anywhere in argv.
        assert!(!first.iter().any(|a| a.contains("KEY")));
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
        let bridge = CliBridge::new(Some(bin), "s1", "m:free");
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
        let bridge = Arc::new(CliBridge::new(Some(bin), "s1", "m:free"));
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
            CliBridge::new(Some(bin), "s1", "m:free").with_abort_grace(Duration::from_millis(300)),
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
    fn turn_times_out_when_cli_hangs() {
        // Fake CLI: prints init, then silence — the real CLI's no-key
        // behaviour (init line only, then hangs). With a 300ms turn
        // timeout the turn must fail as CLI_TURN_TIMEOUT in well under
        // the 30s the fake would run.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("hangs", body);
        let bridge =
            CliBridge::new(Some(bin), "s1", "m:free").with_turn_timeout(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let mut emit = |_ev: SenEvent| {};
        let err = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_CLI_TURN_TIMEOUT);
        assert!(err.message.contains("300ms"));
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
        let bridge = CliBridge::new(Some(bin), "s1", "m:free");
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
        let bridge =
            CliBridge::new(Some(bin), "s1", "m:free").with_turn_timeout(Duration::from_millis(300));
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
        let bridge = CliBridge::new(Some(bin), "s1", "m:free");
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
        let bridge =
            CliBridge::new(Some(bin), "s1", "m:free").with_turn_timeout(Duration::from_secs(5));
        let mut events: Vec<SenEvent> = Vec::new();
        let mut emit = |ev: SenEvent| events.push(ev);
        let result = bridge.run_turn("steer", "hi", &mut emit).unwrap();
        assert_eq!(result.subtype, "success");
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn turn_can_run_again_after_timeout() {
        // After a timed-out turn the handle is not stuck: a second
        // run_turn proceeds (and times out again on this hanging fake)
        // instead of refusing with CLI_TURN_IN_PROGRESS.
        let body = r#"echo '{"type":"system","subtype":"init","session_id":"s1","model":"m:free","v":1}'
i=0; while [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done"#;
        let bin = fake_zen("hangs2", body);
        let bridge =
            CliBridge::new(Some(bin), "s1", "m:free").with_turn_timeout(Duration::from_millis(300));
        let mut emit = |_ev: SenEvent| {};
        let first = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(first.code, ERR_CLI_TURN_TIMEOUT);
        let second = bridge
            .run_turn("follow_up", "again", &mut emit)
            .unwrap_err();
        assert_eq!(second.code, ERR_CLI_TURN_TIMEOUT);
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
        let bridge = CliBridge::new(Some(path), "s1", "m:free");
        let mut emit = |_ev: SenEvent| {};
        let err = bridge.run_turn("steer", "hi", &mut emit).unwrap_err();
        assert_eq!(err.code, ERR_CLI_VERSION_MISMATCH);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
