// File memory Phase 1 (P0-1). PURE RUST, no Tauri dependency: compiles
// and tests standalone in the scratch harness.
//
// Layout under one root dir (the app data dir + "sen-memory/"):
// - MEMORY.md — durable facts, one fact per "- " line.
// - USER.md — facts about the user, same one-item-per-line layout.
// - SOUL.md — persona text, free-form (the whole file is the persona).
// - daily/YYYY-MM-DD.md — event log, append-only, one event per line.
//
// Item rules: a line starting with "- " is an item; every other line
// (headings, blanks, plain text) is preserved verbatim by forget/edit,
// so a hand-written note around the items is never destroyed. Item
// numbers are 1-based over the items only. Forget by text needs exactly
// one substring match, or it refuses and says how many matched: deleting
// the wrong fact silently would be worse than refusing.
//
// What NEVER lives here: no key, no token, no credential of any kind.
// The store holds user facts only; the Telegram token lives in the
// Telegram client, the model key in the process environment.
//
// The memory context block (context_block) is what the backend prepends
// to every turn prompt (see session.rs): the assistant name first, then
// SOUL.md, USER.md, MEMORY.md, then the last 10 lines of today's daily
// log. It is prepended to every turn on purpose: always correct (the
// model never misses an edit, including a `quên`), at the cost of
// repeating a small block in the CLI history. Only the daily part is
// bounded (the last 10 lines): SOUL.md, USER.md and MEMORY.md go in
// in full, so the block grows with those files — the "bounded" claim
// in the first draft was wrong.
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Max characters of one text kept in a daily log line. The log is an
/// event record, not a transcript: a 1000-char cap keeps one pasted wall
/// of text from flooding the file (and the context block, which carries
/// the last 10 lines).
pub const LOG_TEXT_MAX_CHARS: usize = 1000;

/// Daily log lines carried into the memory context block. Enough for
/// same-day continuity; the CLI session history already holds the full
/// turns, so the block does not need more.
pub const CONTEXT_DAILY_LINES: usize = 10;

/// The assistant's own name. The context block always carries it, even
/// when every memory file is empty, so the model always knows who it is.
pub const ASSISTANT_NAME: &str = "Sen";

pub const MEMORY_FILE: &str = "MEMORY.md";
pub const USER_FILE: &str = "USER.md";
pub const SOUL_FILE: &str = "SOUL.md";
pub const DAILY_DIR: &str = "daily";

/// Which item file an operation targets. An enum, never a string: no
/// user input ever reaches a path, so there is no traversal to guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Memory,
    User,
}

impl Scope {
    pub fn file_name(&self) -> &'static str {
        match self {
            Scope::Memory => MEMORY_FILE,
            Scope::User => USER_FILE,
        }
    }
}

/// How a forget target is given: a 1-based item number, or a substring
/// that must match exactly one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgetTarget {
    Number(usize),
    Text(String),
}

/// Kind of a daily log event. The kind is part of the line so the log
/// stays readable without any other record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    User,
    Assistant,
    Memory,
}

impl LogKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogKind::User => "user",
            LogKind::Assistant => "assistant",
            LogKind::Memory => "memory",
        }
    }
}

/// Which channel an event came through. Recorded in the log line so a
/// later reader can tell a desktop turn from a Telegram turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Desktop,
    Telegram,
    Local,
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Channel::Desktop => "desktop",
            Channel::Telegram => "telegram",
            Channel::Local => "local",
        }
    }
}

/// One line of a memory file: an item ("- ...") or anything else
/// (heading, blank, plain text), kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MemLine {
    Item(String),
    Other(String),
}

fn parse_lines(text: &str) -> Vec<MemLine> {
    text.lines()
        .map(|line| {
            if let Some(item) = line.strip_prefix("- ") {
                MemLine::Item(item.to_string())
            } else {
                MemLine::Other(line.to_string())
            }
        })
        .collect()
}

fn render_lines(lines: &[MemLine]) -> String {
    let mut out = String::new();
    for line in lines {
        match line {
            MemLine::Item(item) => out.push_str(&format!("- {}\n", item)),
            MemLine::Other(text) => out.push_str(&format!("{}\n", text)),
        }
    }
    out
}

fn items_of(lines: &[MemLine]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| match line {
            MemLine::Item(item) => Some(item.clone()),
            MemLine::Other(_) => None,
        })
        .collect()
}

/// Truncate to at most `max` chars (char-based, never byte-based: a cut
/// inside a multi-byte character would panic on slicing). Adds "..."
/// when truncated, so a cut is visible.
fn truncate_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("...");
    out
}

/// Validate one fact text: non-empty after trimming, single line. A
/// newline inside a fact would break the one-item-per-line layout and
/// silently create phantom items.
fn validate_fact(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Fact text must be non-empty, FAIL".to_string());
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("Fact text must be a single line, FAIL".to_string());
    }
    Ok(trimmed.to_string())
}

/// Collapse newlines to spaces, so a multi-line text stays one log
/// line. The daily log is one event per line, and the context block
/// carries the last 10 lines verbatim: a newline inside a logged text
/// would inject fake log lines into both.
fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y } as i32, m, d)
}

/// Local date + time ("YYYY-MM-DD", "HH:MM:SS") from the system clock.
/// Local, not UTC: the daily log is the user's log, and the user is not
/// on UTC. Falls back to UTC when localtime fails, never to a crash.
fn local_now() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let time_t = secs as libc::time_t;
    let mut tm = MaybeUninit::<libc::tm>::uninit();
    let ok = unsafe { libc::localtime_r(&time_t, tm.as_mut_ptr()) };
    if !ok.is_null() {
        let tm = unsafe { tm.assume_init() };
        return (
            format!(
                "{:04}-{:02}-{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday
            ),
            format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
        );
    }
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let rest = secs.rem_euclid(86400);
    (
        format!("{:04}-{:02}-{:02}", y, m, d),
        format!(
            "{:02}:{:02}:{:02}",
            rest / 3600,
            (rest / 60) % 60,
            rest % 60
        ),
    )
}

/// A read of the whole memory: item lists, persona text, today's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub memory: Vec<String>,
    pub user: Vec<String>,
    pub soul: String,
    pub today: Vec<String>,
}

/// The file memory store, rooted at one dir. Missing files read as
/// empty: memory must never fail a turn, and a fresh install has no
/// files yet.
pub struct MemoryStore {
    root: PathBuf,
}

impl MemoryStore {
    pub fn new(root: PathBuf) -> MemoryStore {
        MemoryStore { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file_path(&self, scope: Scope) -> PathBuf {
        self.root.join(scope.file_name())
    }

    fn soul_path(&self) -> PathBuf {
        self.root.join(SOUL_FILE)
    }

    fn daily_path(&self, date: &str) -> PathBuf {
        self.root.join(DAILY_DIR).join(format!("{}.md", date))
    }

    /// Today's local date ("YYYY-MM-DD"), for the daily log + context.
    pub fn today_date() -> String {
        local_now().0
    }

    fn read_file(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    fn write_file(path: &Path, text: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create memory dir {}: {}", parent.display(), e))?;
        }
        std::fs::write(path, text)
            .map_err(|e| format!("Cannot write memory file {}: {}", path.display(), e))
    }

    /// Append one line to a file, keeping the existing content and
    /// making sure the new line starts on its own line. Small files, so
    /// read-modify-write is fine and avoids a half-written append.
    fn append_line(path: &Path, line: &str) -> Result<(), String> {
        let mut text = Self::read_file(path);
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
        Self::write_file(path, &text)
    }

    /// Read everything: both item lists, the persona, today's log lines.
    pub fn read(&self) -> MemorySnapshot {
        let memory = items_of(&parse_lines(&Self::read_file(
            &self.file_path(Scope::Memory),
        )));
        let user = items_of(&parse_lines(&Self::read_file(&self.file_path(Scope::User))));
        let soul = Self::read_file(&self.soul_path()).trim().to_string();
        let today = Self::read_file(&self.daily_path(&Self::today_date()))
            .lines()
            .map(str::to_string)
            .collect();
        MemorySnapshot {
            memory,
            user,
            soul,
            today,
        }
    }

    /// Append one fact. Returns its 1-based item number.
    pub fn add(&self, scope: Scope, text: &str) -> Result<usize, String> {
        let fact = validate_fact(text)?;
        let path = self.file_path(scope);
        let before = items_of(&parse_lines(&Self::read_file(&path))).len();
        Self::append_line(&path, &format!("- {}", fact))?;
        Ok(before + 1)
    }

    /// Delete one item, by 1-based number or by text. A text target must
    /// match exactly one item (case-insensitive substring): zero matches
    /// and several matches both refuse, so a `quên` never deletes the
    /// wrong fact. Returns the removed item text.
    pub fn forget(&self, scope: Scope, target: &ForgetTarget) -> Result<String, String> {
        let path = self.file_path(scope);
        let mut lines = parse_lines(&Self::read_file(&path));
        let item_positions: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| matches!(line, MemLine::Item(_)))
            .map(|(i, _)| i)
            .collect();
        let pos = match target {
            ForgetTarget::Number(n) => {
                if *n == 0 || *n > item_positions.len() {
                    return Err(format!(
                        "No memory item {}: {} has {} item(s), FAIL",
                        n,
                        scope.file_name(),
                        item_positions.len()
                    ));
                }
                item_positions[n - 1]
            }
            ForgetTarget::Text(needle) => {
                let query = needle.trim().to_lowercase();
                if query.is_empty() {
                    return Err("Forget text must be non-empty, FAIL".to_string());
                }
                let matches: Vec<usize> = item_positions
                    .iter()
                    .copied()
                    .filter(|&i| match &lines[i] {
                        MemLine::Item(item) => item.to_lowercase().contains(&query),
                        MemLine::Other(_) => false,
                    })
                    .collect();
                match matches.len() {
                    0 => {
                        return Err(format!(
                            "No memory item matches {:?} in {}, FAIL",
                            needle,
                            scope.file_name()
                        ));
                    }
                    1 => matches[0],
                    n => {
                        let numbers: Vec<String> = matches
                            .iter()
                            .map(|&p| {
                                item_positions
                                    .iter()
                                    .position(|&x| x == p)
                                    .map(|i| (i + 1).to_string())
                                    .unwrap_or_default()
                            })
                            .collect();
                        return Err(format!(
                            "{} memory items match {:?} in {} (items {}), be specific, FAIL",
                            n,
                            needle,
                            scope.file_name(),
                            numbers.join(", "),
                        ));
                    }
                }
            }
        };
        let removed = match &lines[pos] {
            MemLine::Item(item) => item.clone(),
            MemLine::Other(_) => unreachable!(),
        };
        lines.remove(pos);
        Self::write_file(&path, &render_lines(&lines))?;
        Ok(removed)
    }

    /// Replace one item's text (1-based number). Returns the new text.
    pub fn edit(&self, scope: Scope, index: usize, text: &str) -> Result<String, String> {
        let fact = validate_fact(text)?;
        let path = self.file_path(scope);
        let mut lines = parse_lines(&Self::read_file(&path));
        let item_positions: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| matches!(line, MemLine::Item(_)))
            .map(|(i, _)| i)
            .collect();
        if index == 0 || index > item_positions.len() {
            return Err(format!(
                "No memory item {}: {} has {} item(s), FAIL",
                index,
                scope.file_name(),
                item_positions.len()
            ));
        }
        lines[item_positions[index - 1]] = MemLine::Item(fact.clone());
        Self::write_file(&path, &render_lines(&lines))?;
        Ok(fact)
    }

    /// Replace the whole persona text. An empty text clears SOUL.md; the
    /// context block still names the assistant, so the name never drops.
    pub fn set_soul(&self, text: &str) -> Result<(), String> {
        if text.contains('\r') {
            return Err("Persona text must not contain carriage returns, FAIL".to_string());
        }
        Self::write_file(&self.soul_path(), text.trim())?;
        Ok(())
    }

    /// Append one event to today's daily log. The timestamp is local
    /// time, taken here so every caller logs on the same clock. The
    /// text is flattened to one line first (see single_line): a
    /// multi-line message must not inject fake log lines.
    pub fn log(&self, kind: LogKind, channel: Channel, text: &str) -> Result<(), String> {
        let (date, time) = local_now();
        let line = format!(
            "- {} [{}] {}: {}",
            time,
            channel.as_str(),
            kind.as_str(),
            truncate_chars(&single_line(text.trim()), LOG_TEXT_MAX_CHARS)
        );
        Self::append_line(&self.daily_path(&date), &line)
    }

    /// The memory context block: assistant name, persona, user facts,
    /// durable facts, today's recent events. Sections with no content
    /// are left out, so an empty memory stays a short block.
    pub fn context_block(&self) -> String {
        let snapshot = self.read();
        let mut block = String::new();
        block.push_str(&format!("[Sen memory - {}]\n", Self::today_date()));
        block.push_str(&format!(
            "Bạn là {}, trợ lý cá nhân. Trả lời bằng tiếng Việt, trừ khi người dùng viết bằng ngôn ngữ khác.\n",
            ASSISTANT_NAME
        ));
        if !snapshot.soul.is_empty() {
            block.push_str("Tính cách:\n");
            block.push_str(&snapshot.soul);
            block.push('\n');
        }
        if !snapshot.user.is_empty() {
            block.push_str("Về người dùng:\n");
            for item in &snapshot.user {
                block.push_str(&format!("- {}\n", item));
            }
        }
        if !snapshot.memory.is_empty() {
            block.push_str("Dữ kiện bền:\n");
            for item in &snapshot.memory {
                block.push_str(&format!("- {}\n", item));
            }
        }
        let recent: Vec<&String> = snapshot
            .today
            .iter()
            .rev()
            .take(CONTEXT_DAILY_LINES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if !recent.is_empty() {
            block.push_str("Hôm nay:\n");
            for line in recent {
                block.push_str(line);
                block.push('\n');
            }
        }
        block.trim_end().to_string()
    }

    /// The full prompt for one turn: the context block, a separator,
    /// then the user's text. The separator keeps the block from ever
    /// blending into the user's words.
    pub fn turn_prompt(&self, text: &str) -> String {
        format!("{}\n---\n{}", self.context_block(), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sen_mem_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(tag: &str) -> (MemoryStore, PathBuf) {
        let dir = tmpdir(tag);
        (MemoryStore::new(dir.clone()), dir)
    }

    #[test]
    fn add_then_read_round_trips_and_numbers_are_one_based() {
        let (store, dir) = store("addread");
        assert_eq!(store.add(Scope::Memory, "likes tea").unwrap(), 1);
        assert_eq!(store.add(Scope::Memory, "works nights").unwrap(), 2);
        assert_eq!(store.add(Scope::User, "name is Zen").unwrap(), 1);
        let snap = store.read();
        assert_eq!(snap.memory, vec!["likes tea", "works nights"]);
        assert_eq!(snap.user, vec!["name is Zen"]);
        assert_eq!(snap.soul, "");
        assert_eq!(snap.today, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_empty_and_multiline_facts() {
        let (store, dir) = store("addbad");
        assert!(store.add(Scope::Memory, "   ").is_err());
        assert!(store.add(Scope::Memory, "two\nlines").is_err());
        assert_eq!(store.read().memory, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_by_number_removes_only_that_item_and_keeps_other_lines() {
        let (store, dir) = store("forgetnum");
        let path = store.file_path(Scope::Memory);
        std::fs::write(&path, "# Notes\n- one\n- two\n\nSome prose.\n- three\n").unwrap();
        assert_eq!(
            store
                .forget(Scope::Memory, &ForgetTarget::Number(2))
                .unwrap(),
            "two"
        );
        assert_eq!(store.read().memory, vec!["one", "three"]);
        // The heading, the blank line and the prose survive the delete.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# Notes"));
        assert!(text.contains("Some prose."));
        assert!(!text.contains("- two"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_by_number_refuses_out_of_range() {
        let (store, dir) = store("forgetrange");
        store.add(Scope::Memory, "only").unwrap();
        assert!(store
            .forget(Scope::Memory, &ForgetTarget::Number(0))
            .is_err());
        assert!(store
            .forget(Scope::Memory, &ForgetTarget::Number(2))
            .is_err());
        assert_eq!(store.read().memory, vec!["only"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_by_text_needs_exactly_one_match() {
        let (store, dir) = store("forgettext");
        store.add(Scope::Memory, "likes green tea").unwrap();
        store.add(Scope::Memory, "likes black tea").unwrap();
        store.add(Scope::Memory, "works nights").unwrap();
        // Two match "tea": refuse, delete nothing.
        let err = store
            .forget(Scope::Memory, &ForgetTarget::Text("tea".to_string()))
            .unwrap_err();
        assert!(
            err.contains('2'),
            "error must say how many matched: {}",
            err
        );
        assert_eq!(store.read().memory.len(), 3);
        // Zero match: refuse too.
        assert!(store
            .forget(Scope::Memory, &ForgetTarget::Text("coffee".to_string()))
            .is_err());
        // One match, case-insensitive: deletes.
        assert_eq!(
            store
                .forget(Scope::Memory, &ForgetTarget::Text("NIGHTS".to_string()))
                .unwrap(),
            "works nights"
        );
        assert_eq!(
            store.read().memory,
            vec!["likes green tea", "likes black tea"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_replaces_one_item_in_place() {
        let (store, dir) = store("edit");
        store.add(Scope::Memory, "old fact").unwrap();
        store.add(Scope::Memory, "other fact").unwrap();
        assert_eq!(
            store.edit(Scope::Memory, 1, "new fact").unwrap(),
            "new fact"
        );
        assert_eq!(store.read().memory, vec!["new fact", "other fact"]);
        assert!(store.edit(Scope::Memory, 3, "x").is_err());
        assert!(store.edit(Scope::Memory, 1, "two\nlines").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_soul_round_trips_and_empty_clears() {
        let (store, dir) = store("soul");
        store.set_soul("Gần gũi, thẳng thắn, ít chữ.").unwrap();
        assert_eq!(store.read().soul, "Gần gũi, thẳng thắn, ít chữ.");
        store.set_soul("   ").unwrap();
        assert_eq!(store.read().soul, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_appends_timestamped_lines_and_truncates_long_text() {
        let (store, dir) = store("log");
        store
            .log(LogKind::User, Channel::Telegram, "hello")
            .unwrap();
        let long = "x".repeat(1500);
        store
            .log(LogKind::Assistant, Channel::Telegram, &long)
            .unwrap();
        let snap = store.read();
        assert_eq!(snap.today.len(), 2);
        assert!(snap.today[0].contains("[telegram] user: hello"));
        assert!(snap.today[0].starts_with("- "), "line: {}", snap.today[0]);
        // The long line is cut to 1000 chars + "..." and stays one line.
        assert!(snap.today[1].ends_with("..."));
        assert!(!snap.today[1].contains('\n'));
        let tail = snap.today[1].split("assistant: ").nth(1).unwrap();
        assert_eq!(tail.chars().count(), LOG_TEXT_MAX_CHARS + 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_sanitizes_newlines_so_one_event_stays_one_line() {
        // A multi-line Telegram message must not inject fake lines
        // into the daily log (and from there into the context block).
        let (store, dir) = store("lognl");
        store
            .log(
                LogKind::User,
                Channel::Telegram,
                "line one\nline two\rline three",
            )
            .unwrap();
        let snap = store.read();
        assert_eq!(snap.today.len(), 1, "one event, one line: {:?}", snap.today);
        assert!(snap.today[0].contains("line one line two line three"));
        assert!(!snap.today[0].contains('\n'));
        assert!(!snap.today[0].contains('\r'));
        // The raw file has exactly one line for the event too.
        let raw = std::fs::read_to_string(
            dir.join("daily")
                .join(format!("{}.md", MemoryStore::today_date())),
        )
        .unwrap();
        assert_eq!(raw.lines().count(), 1, "raw file: {:?}", raw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_block_names_sen_and_carries_facts_but_skips_empty_sections() {
        let (store, dir) = store("context");
        let block = store.context_block();
        assert!(block.contains("Bạn là Sen"), "block: {}", block);
        assert!(!block.contains("Về người dùng:"));
        assert!(!block.contains("Dữ kiện bền:"));
        store.add(Scope::Memory, "likes tea").unwrap();
        store.add(Scope::User, "name is Zen").unwrap();
        store.set_soul("Thẳng thắn.").unwrap();
        let block = store.context_block();
        assert!(block.contains("Bạn là Sen"));
        assert!(block.contains("Tính cách:\nThẳng thắn."));
        assert!(block.contains("Về người dùng:\n- name is Zen"));
        assert!(block.contains("Dữ kiện bền:\n- likes tea"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turn_prompt_puts_the_user_text_after_the_separator() {
        let (store, dir) = store("prompt");
        store.add(Scope::Memory, "likes tea").unwrap();
        let prompt = store.turn_prompt("what do I like?");
        assert!(prompt.contains("- likes tea"));
        assert!(prompt.ends_with("\n---\nwhat do I like?"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2026-11-16 00:00 UTC (checked with `date -u -d @1794787200`).
        assert_eq!(civil_from_days(1794787200 / 86400), (2026, 11, 16));
        assert_eq!(civil_from_days(20000), (2024, 10, 4));
    }

    #[test]
    fn local_now_has_the_right_shape() {
        let (date, time) = local_now();
        assert_eq!(date.len(), 10);
        assert_eq!(date.chars().nth(4), Some('-'));
        assert_eq!(date.chars().nth(7), Some('-'));
        assert_eq!(time.len(), 8);
        assert_eq!(time.chars().nth(2), Some(':'));
        assert_eq!(time.chars().nth(5), Some(':'));
    }

    #[test]
    fn memory_files_never_carry_a_credential() {
        // The store has no token/key field at all; this pins that none
        // of its files ever contains one, even after every operation.
        let (store, dir) = store("nocred");
        store.add(Scope::Memory, "likes tea").unwrap();
        store.set_soul("Thẳng thắn.").unwrap();
        store.log(LogKind::User, Channel::Desktop, "hello").unwrap();
        let mut found = Vec::new();
        for entry in walkdir(&dir) {
            let text = std::fs::read_to_string(&entry)
                .unwrap_or_default()
                .to_lowercase();
            if text.contains("token") || text.contains("api_key") || text.contains("api-key") {
                found.push(entry);
            }
        }
        assert!(found.is_empty(), "credential word in: {:?}", found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn walkdir(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(walkdir(&path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    }
}
