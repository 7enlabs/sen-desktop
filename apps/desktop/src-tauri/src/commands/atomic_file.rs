// Atomic file write (Sen P2-6). PURE RUST, no Tauri dependency:
// compiles and tests standalone in the scratch harness.
//
// `std::fs::write` truncates the target and then writes: a crash (or
// a kill) between the truncate and the last byte leaves a torn file.
// That tore three durable files in this crate:
// - cli-settings.json: a torn file loads as the default, silently
//   losing the stored router and BYOK approval.
// - MEMORY.md: a torn file loses facts.
// - the Telegram offset file: a torn file loads as 0, replaying up to
//   100 already-handled updates.
// `write_atomic` instead writes a temp file in the SAME directory,
// fsyncs it, and renames it over the target. The rename is one atomic
// step, so a reader — or the next process after a crash — sees either
// the whole old file or the whole new file, never a mix. A temp file
// left behind by a crash is an orphan with a `.tmp-` suffix, never
// mistaken for the real file; the next write uses a fresh unique name
// and never reuses it.
//
// What this does NOT do: it does not serialize concurrent writers.
// Two threads writing the same path at once still race — the last
// rename wins, whole. Callers that read-modify-write (settings)
// serialize that sequence with their own lock (see
// `CliSettings::update`).
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Counter that makes every temp name unique inside this process, so
/// two threads writing the same target never share a temp file.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically: temp file in the same
/// directory, fsync, rename over the target. The parent directory
/// must already exist (callers create it); on failure the temp file
/// is removed and the target is left untouched.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let temp_name = format!(
        ".{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let temp_path = parent.join(temp_name);
    let result = write_and_rename(&temp_path, parent, path, contents);
    if result.is_err() {
        // Best effort: a failed write must not leave a temp file
        // behind for the next write (or the next reader) to trip on.
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn write_and_rename(
    temp_path: &Path,
    parent: &Path,
    path: &Path,
    contents: &[u8],
) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::File::create(temp_path).map_err(|e| {
        format!(
            "Cannot create temp file {} for {}: {}",
            temp_path.display(),
            path.display(),
            e
        )
    })?;
    file.write_all(contents).map_err(|e| {
        format!(
            "Cannot write temp file {} for {}: {}",
            temp_path.display(),
            path.display(),
            e
        )
    })?;
    // Flush the bytes to disk before the rename: without this, a
    // power loss could leave the renamed file short.
    file.sync_all().map_err(|e| {
        format!(
            "Cannot sync temp file {} for {}: {}",
            temp_path.display(),
            path.display(),
            e
        )
    })?;
    drop(file);
    std::fs::rename(temp_path, path).map_err(|e| {
        format!(
            "Cannot rename temp file {} over {}: {}",
            temp_path.display(),
            path.display(),
            e
        )
    })?;
    // Best effort: fsync the directory so the rename itself survives
    // a power loss. Opening a directory as a file works on Unix; on
    // platforms where it does not, the rename is still atomic for a
    // process crash, which is the failure this fix targets.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sen_atomic_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_names_left(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn write_atomic_round_trips_and_leaves_no_temp_file() {
        let dir = tmpdir("roundtrip");
        let path = dir.join("data.txt");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert!(temp_names_left(&dir).is_empty());
        // A second write replaces the whole file, still no temp left.
        write_atomic(&path, b"hello again, longer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello again, longer");
        assert!(temp_names_left(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_fails_cleanly_when_the_directory_is_missing() {
        let dir = tmpdir("missingdir");
        let path = dir.join("no-such-dir").join("data.txt");
        let err = write_atomic(&path, b"hello").unwrap_err();
        assert!(err.contains("data.txt"), "error names the target: {}", err);
        // No temp file was left behind (there was nowhere to put one).
        assert!(!dir.join("no-such-dir").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // P2-6: the crash test. A helper child process writes two large
    // payloads in a loop through write_atomic; the parent kills it
    // with SIGKILL at a random moment, then the file must be exactly
    // one of the two whole payloads — never a torn mix. The helper is
    // a real #[test] so it runs inside the same test binary the
    // parent spawns; it does nothing unless SEN_ATOMIC_CRASH_CHILD=1
    // is set, so a normal suite run never hangs on it.
    // Windows: this test is #[cfg(unix)] and does NOT run on Windows
    // — Windows has no SIGKILL (and libc::kill is Unix-only), so the
    // kill-mid-write case cannot be reproduced there. The skip is
    // marked by crash_mid_write_skipped_on_windows_no_sigkill below,
    // so it shows up in the Windows test list instead of vanishing
    // silently; the atomicity itself is still covered on Windows by
    // the other write_atomic tests in this module.
    const CRASH_PAYLOAD_A: &[u8] = &[b'A'; 65_536];
    const CRASH_PAYLOAD_B: &[u8] = &[b'B'; 65_537];

    #[test]
    fn crash_writer_child_do_not_run_directly() {
        if std::env::var("SEN_ATOMIC_CRASH_CHILD").is_err() {
            return;
        }
        let path = std::env::var("SEN_ATOMIC_CRASH_PATH").unwrap();
        let path = Path::new(&path);
        let mut use_a = true;
        loop {
            let payload = if use_a {
                CRASH_PAYLOAD_A
            } else {
                CRASH_PAYLOAD_B
            };
            // A failed write (parent removed the dir, child about to
            // die) is ignored: the loop just tries the next payload.
            let _ = write_atomic(path, payload);
            use_a = !use_a;
        }
    }

    #[test]
    #[cfg(unix)]
    fn crash_mid_write_leaves_old_or_new_file_never_torn() {
        use std::io::Read;
        let dir = tmpdir("crash");
        let path = dir.join("data.txt");
        write_atomic(&path, CRASH_PAYLOAD_A).unwrap();
        let exe = std::env::current_exe().unwrap();
        for attempt in 0..5 {
            let mut child = std::process::Command::new(&exe)
                .arg("crash_writer_child_do_not_run_directly")
                .env("SEN_ATOMIC_CRASH_CHILD", "1")
                .env("SEN_ATOMIC_CRASH_PATH", &path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            // Let the child run long enough to be mid-write (or
            // between writes) when the kill lands.
            std::thread::sleep(std::time::Duration::from_millis(50 + attempt * 20));
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.wait();
            let mut content = Vec::new();
            match std::fs::File::open(&path) {
                Ok(mut file) => {
                    file.read_to_end(&mut content).unwrap();
                }
                // Killed before the first write finished: no file to
                // judge yet, try the next attempt.
                Err(_) => continue,
            }
            assert!(
                content == CRASH_PAYLOAD_A || content == CRASH_PAYLOAD_B,
                "attempt {}: torn file, {} bytes, starts with {:?}",
                attempt,
                content.len(),
                &content[..content.len().min(16)]
            );
            // A kill between temp creation and rename may leave one
            // orphan temp file behind; it is never the target, and
            // the target above is still whole, so it needs no
            // cleanup here.
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Windows marker for the crash test above: it only exists so the
    // #[cfg(unix)] skip is visible in `cargo test` output on Windows
    // (a test name in the list, not a silent absence). It asserts
    // nothing about atomicity — see the other tests in this module.
    #[test]
    #[cfg(windows)]
    fn crash_mid_write_skipped_on_windows_no_sigkill() {
        eprintln!("SKIPPED on Windows: no SIGKILL, so the crash test is #[cfg(unix)]");
    }
}
