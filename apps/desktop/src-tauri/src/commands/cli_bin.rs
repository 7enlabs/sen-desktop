// CLI binary resolution + probe (Phase 2, tin 9530 quyết 1).
// PURE RUST, no Tauri dependency: compiles and tests standalone, so the
// logic has real test evidence even on a box without the Tauri system
// libraries (same pattern as commands/registry.rs).
//
// Decision 1: the Desktop finds the `zen` binary from the ZEN_BIN env var
// first, then from PATH. No binary is bundled with the app.
// The probe (`<bin> --version`) is a release-line gate, not a rev proof:
// the CLI exposes no git rev at runtime (`zen --version` prints
// "zen 0.1.0", `zen doctor --json` carries "version":"0.1.0", stream-json
// records carry only the schema version v=1). The pin below enforces the
// pinned release line 0.1.0 (source rev 8ac73fe, decision tin 6046, same
// gate as the B2 TS bridge); a different 0.1.0 build still reports 0.1.0,
// so this gate proves the release line, not the exact rev — that limit is
// stated here, not hidden.
//
// No key is involved anywhere in this module: resolution and probe run no
// model call and never touch an API key.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned CLI release line (decision tin 6046). See the module comment:
// the gate proves the line, not the exact source rev.
pub const PINNED_CLI_VERSION: &str = "0.1.0";
/// Pinned CLI source revision the 0.1.0 line comes from (for reporting
/// only; it cannot be checked at runtime).
pub const PINNED_CLI_REV: &str = "8ac73fe";

/// How long the `--version` probe may take before the binary is treated
/// as hung and killed.
pub const PROBE_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliBinError {
    pub code: String,
    pub message: String,
}

impl CliBinError {
    pub fn new(code: &str, message: &str) -> CliBinError {
        CliBinError {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

pub const ERR_CLI_NOT_FOUND: &str = "CLI_NOT_FOUND";
pub const ERR_CLI_VERSION_MISMATCH: &str = "CLI_VERSION_MISMATCH";
pub const ERR_CLI_PROBE_FAILED: &str = "CLI_PROBE_FAILED";

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Resolve the `zen` binary. `env_bin` is the value of ZEN_BIN when set
/// (non-empty); `path_var` is the PATH value to search. Pure so tests can
/// pass explicit values instead of mutating process env.
pub fn resolve_zen_bin_with(env_bin: Option<&str>, path_var: &str) -> Result<PathBuf, CliBinError> {
    if let Some(bin) = env_bin {
        let trimmed = bin.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if is_executable_file(&path) {
                return Ok(path);
            }
            return Err(CliBinError::new(
                ERR_CLI_NOT_FOUND,
                &format!(
                    "ZEN_BIN points at a missing or non-executable file: {}",
                    trimmed
                ),
            ));
        }
    }
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join("zen");
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err(CliBinError::new(
        ERR_CLI_NOT_FOUND,
        "No zen binary found: set ZEN_BIN to its path or put zen on PATH (no binary is bundled with the app).",
    ))
}

/// Resolve the `zen` binary from the real process environment: ZEN_BIN
/// first, then PATH. This is the only production entry point.
pub fn resolve_zen_bin() -> Result<PathBuf, CliBinError> {
    let env_bin = std::env::var("ZEN_BIN").ok();
    let path_var = std::env::var("PATH").unwrap_or_default();
    resolve_zen_bin_with(env_bin.as_deref(), &path_var)
}

/// Parse `zen --version` output ("zen 0.1.0") into the bare version
/// string, or None when it is not a recognisable version line.
pub fn parse_cli_version_output(output: &str) -> Option<String> {
    let trimmed = output.trim();
    let bare = trimmed.strip_prefix("zen ").unwrap_or(trimmed);
    let bare = bare.trim();
    let mut parts = bare.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !major.chars().all(|c| c.is_ascii_digit())
        || !minor.chars().all(|c| c.is_ascii_digit())
        || !patch.chars().all(|c| c.is_ascii_digit())
        || major.is_empty()
        || minor.is_empty()
        || patch.is_empty()
    {
        return None;
    }
    Some(bare.to_string())
}

/// Fail unless the binary reports the pinned CLI version. Release-line
/// gate only (see the module comment).
pub fn assert_pinned_cli_version(version: &str) -> Result<(), CliBinError> {
    if version != PINNED_CLI_VERSION {
        return Err(CliBinError::new(
            ERR_CLI_VERSION_MISMATCH,
            &format!(
                "CLI version mismatch: binary reports {}, pinned CLI is {} (source rev {}). Refusing to run with an unpinned CLI binary.",
                version, PINNED_CLI_VERSION, PINNED_CLI_REV
            ),
        ));
    }
    Ok(())
}

/// Exit-code table (B2, PR #40). REPORTING ONLY, never control flow.
pub fn exit_code_label(code: Option<i32>) -> &'static str {
    match code {
        Some(0) => "ok (0)",
        Some(1) => "general error (1)",
        Some(2) => "auth (2)",
        Some(3) => "not found (3)",
        Some(4) => "validation (4)",
        Some(5) => "forbidden (5)",
        Some(6) => "rate limit (6)",
        Some(7) => "conflict (7)",
        Some(130) => "SIGINT (130)",
        Some(143) => "SIGTERM (143)",
        Some(_) => "unknown exit code",
        None => "no exit code (killed by signal)",
    }
}

/// Prove the binary exists and runs, without a key, a model, or a network
/// call: `<bin> --version` only. Returns the parsed version string.
/// A probe that does not finish within PROBE_TIMEOUT_MS is killed and
/// reported as a failure.
pub fn probe_cli_bin(bin: &Path) -> Result<String, CliBinError> {
    use std::io::Read;
    let bin_str = bin.to_string_lossy().to_string();
    let mut child = match Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return Err(CliBinError::new(
                ERR_CLI_PROBE_FAILED,
                &format!(
                    "CLI binary not found or not executable: {} ({})",
                    bin_str, e
                ),
            ));
        }
    };
    // Read stdout on a helper thread so the wait below can time out;
    // wait() itself has no timeout in std.
    let mut stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_string(&mut text);
        }
        let _ = tx.send(text);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(PROBE_TIMEOUT_MS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => {
                return Err(CliBinError::new(
                    ERR_CLI_PROBE_FAILED,
                    &format!("CLI probe failed: {} ({})", bin_str, e),
                ));
            }
        }
    };
    let text = rx
        .recv_timeout(std::time::Duration::from_millis(1000))
        .unwrap_or_default();
    let status = match status {
        Some(status) => status,
        None => {
            return Err(CliBinError::new(
                ERR_CLI_PROBE_FAILED,
                &format!(
                    "CLI probe timed out after {}ms: {}",
                    PROBE_TIMEOUT_MS, bin_str
                ),
            ));
        }
    };
    if !status.success() {
        return Err(CliBinError::new(
            ERR_CLI_PROBE_FAILED,
            &format!(
                "CLI probe failed: {} --version exited {} with no version output.",
                bin_str,
                exit_code_label(status.code())
            ),
        ));
    }
    let version = parse_cli_version_output(&text).ok_or_else(|| {
        CliBinError::new(
            ERR_CLI_PROBE_FAILED,
            &format!(
                "CLI probe failed: {} --version output not recognised: {:?} (pinned CLI {}, source rev {}).",
                bin_str,
                text.trim(),
                PINNED_CLI_VERSION,
                PINNED_CLI_REV
            ),
        )
    })?;
    assert_pinned_cli_version(&version)?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_zen_bin_over_path() {
        let dir = std::env::temp_dir().join(format!("zen_bin_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join("my-zen");
        let path_path = dir.join("zen");
        std::fs::write(&env_path, "#!/bin/sh\necho zen 0.1.0\n").unwrap();
        std::fs::write(&path_path, "#!/bin/sh\necho zen 0.1.0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&env_path, &path_path] {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let found =
            resolve_zen_bin_with(Some(env_path.to_str().unwrap()), dir.to_str().unwrap()).unwrap();
        assert_eq!(found, env_path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_falls_back_to_path_and_refuses_missing() {
        let dir = std::env::temp_dir().join(format!("zen_bin_test2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path_path = dir.join("zen");
        std::fs::write(&path_path, "#!/bin/sh\necho zen 0.1.0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = resolve_zen_bin_with(None, dir.to_str().unwrap()).unwrap();
        assert_eq!(found, path_path);
        let err = resolve_zen_bin_with(None, "/nonexistent-dir-xyz").unwrap_err();
        assert_eq!(err.code, ERR_CLI_NOT_FOUND);
        let err = resolve_zen_bin_with(Some("/nonexistent-dir-xyz/zen"), dir.to_str().unwrap())
            .unwrap_err();
        assert_eq!(err.code, ERR_CLI_NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_version_output() {
        assert_eq!(
            parse_cli_version_output("zen 0.1.0\n"),
            Some("0.1.0".to_string())
        );
        assert_eq!(parse_cli_version_output("0.1.0"), Some("0.1.0".to_string()));
        assert_eq!(
            parse_cli_version_output("zen 0.2.0"),
            Some("0.2.0".to_string())
        );
        assert_eq!(parse_cli_version_output("garbage"), None);
        assert_eq!(parse_cli_version_output("zen 0.1"), None);
        assert_eq!(parse_cli_version_output("zen 0.1.0.1"), None);
        assert_eq!(parse_cli_version_output(""), None);
    }

    #[test]
    fn pin_gate_accepts_010_and_refuses_other() {
        assert!(assert_pinned_cli_version("0.1.0").is_ok());
        let err = assert_pinned_cli_version("0.2.0").unwrap_err();
        assert_eq!(err.code, ERR_CLI_VERSION_MISMATCH);
        assert!(err.message.contains(PINNED_CLI_REV));
    }

    #[test]
    fn exit_code_labels() {
        assert_eq!(exit_code_label(Some(0)), "ok (0)");
        assert_eq!(exit_code_label(Some(2)), "auth (2)");
        assert_eq!(exit_code_label(Some(130)), "SIGINT (130)");
        assert_eq!(exit_code_label(None), "no exit code (killed by signal)");
    }

    #[test]
    fn probe_real_binary_reports_010() {
        // The real CLI binary is the pinned one; probe needs no key.
        let zen = std::env::var("ZEN_TEST_BIN")
            .unwrap_or_else(|_| "/home/hatch/workspace/cli-core-main/target/debug/zen".to_string());
        let path = PathBuf::from(&zen);
        if !path.is_file() {
            return;
        }
        let version = probe_cli_bin(&path).unwrap();
        assert_eq!(version, "0.1.0");
    }
}
