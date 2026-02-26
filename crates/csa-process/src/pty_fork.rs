//! Experimental Codex native fork via PTY emulation.
//!
//! This module is compiled only when the `codex-pty-fork` feature is enabled.
//! It provides a best-effort native fork path for Codex sessions, including:
//! - terminal handshake replies for common VT queries,
//! - trust-dialog detection with graceful degradation,
//! - SQLite polling for the newly created child session.

use anyhow::{Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

#[cfg(unix)]
use nix::pty::{Winsize, openpty};
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::{Pid, dup, setsid};
#[cfg(unix)]
use rusqlite::{Connection, OpenFlags};
#[cfg(unix)]
use std::fs::{self, File};
#[cfg(unix)]
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process::{Command as StdCommand, Stdio};

const DEFAULT_POLL_TIMEOUT_SECS: u64 = 30;
const DEFAULT_SESSION_MARKER: &str = "csa-pty-fork";

const HANDSHAKE_QUERY_CURSOR_POS: &[u8] = b"\x1b[6n";
const HANDSHAKE_QUERY_DEVICE_ATTR: &[u8] = b"\x1b[c";
const HANDSHAKE_QUERY_KITTY_KEYBOARD: &[u8] = b"\x1b[?u";
const HANDSHAKE_QUERY_OSC10: &[u8] = b"\x1b]10;?";

const HANDSHAKE_REPLY_CURSOR_POS: &[u8] = b"\x1b[1;1R";
const HANDSHAKE_REPLY_DEVICE_ATTR: &[u8] = b"\x1b[?1;2c";
const HANDSHAKE_REPLY_KITTY_KEYBOARD: &[u8] = b"\x1b[?1u";
const HANDSHAKE_REPLY_OSC10: &[u8] = b"\x1b]10;rgb:ffff/ffff/ffff\x07";

const TRUST_DIALOG_PATTERNS: &[&str] = &[
    "trust this folder",
    "trust this workspace",
    "trust this project",
    "do you trust",
    "allow this workspace",
];

/// Config for Codex PTY fork behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyForkConfig {
    /// If true, continue when trust dialog is detected.
    pub codex_auto_trust: bool,
    /// Timeout for SQLite polling window.
    pub poll_timeout_secs: u64,
    /// Unique marker appended to fork prompt for child-session discovery.
    pub session_marker: String,
}

impl Default for PtyForkConfig {
    fn default() -> Self {
        Self {
            codex_auto_trust: false,
            poll_timeout_secs: DEFAULT_POLL_TIMEOUT_SECS,
            session_marker: DEFAULT_SESSION_MARKER.to_string(),
        }
    }
}

/// Result for PTY fork attempts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyForkResult {
    /// Native fork succeeded and child session id was discovered.
    Success { child_session_id: String },
    /// Native path intentionally degraded to soft-fork path.
    Degraded { reason: String },
    /// Native path failed unexpectedly.
    Failed { error: String },
}

/// Fork a Codex session via PTY emulation.
///
/// This function validates the parent session id, checks Codex fork capability,
/// runs `codex fork` inside a PTY, answers common terminal handshake probes,
/// and polls local SQLite state for the newly created child session.
pub async fn fork_codex_session(
    parent_session_id: &str,
    codex_path: &Path,
    config: &PtyForkConfig,
) -> Result<PtyForkResult> {
    if !is_valid_ulid_session_id(parent_session_id) {
        return Ok(PtyForkResult::Failed {
            error: format!(
                "invalid parent session id (must match ULID whitelist): {parent_session_id}"
            ),
        });
    }

    let marker = config.session_marker.trim();
    if marker.is_empty() {
        return Ok(PtyForkResult::Failed {
            error: "session_marker must not be empty".to_string(),
        });
    }

    let fork_support = detect_codex_fork_support(codex_path).await?;
    if !fork_support.supported {
        return Ok(PtyForkResult::Degraded {
            reason: fork_support.reason,
        });
    }

    #[cfg(not(unix))]
    {
        let _ = (parent_session_id, codex_path, config, marker);
        return Ok(PtyForkResult::Degraded {
            reason: "codex PTY fork is only supported on unix targets".to_string(),
        });
    }

    #[cfg(unix)]
    {
        let prompt_marker = format!("{marker} [csa-parent:{parent_session_id}]");
        let poll_started_at = current_unix_timestamp_secs();

        let mut child_state = spawn_codex_fork_pty(codex_path, parent_session_id, &prompt_marker)
            .context("failed to spawn codex fork PTY")?;

        let poll_timeout = Duration::from_secs(config.poll_timeout_secs.max(1));
        let deadline = Instant::now() + poll_timeout;
        let mut final_exit: Option<i32> = None;

        loop {
            if child_state.trust_detected.load(Ordering::Relaxed) && !config.codex_auto_trust {
                terminate_pty_child(&mut child_state.child).await;
                join_pty_io_thread(child_state.io_thread.take());
                return Ok(PtyForkResult::Degraded {
                    reason: "codex trust dialog detected and codex_auto_trust=false".to_string(),
                });
            }

            if let Some(child_session_id) = query_child_session_once(
                parent_session_id,
                marker,
                poll_started_at,
                &codex_db_candidates(),
            )? {
                terminate_pty_child(&mut child_state.child).await;
                join_pty_io_thread(child_state.io_thread.take());
                return Ok(PtyForkResult::Success { child_session_id });
            }

            match child_state.child.try_wait() {
                Ok(Some(status)) => {
                    final_exit = status.code();
                    break;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(e) => {
                    terminate_pty_child(&mut child_state.child).await;
                    join_pty_io_thread(child_state.io_thread.take());
                    return Ok(PtyForkResult::Failed {
                        error: format!("failed checking codex fork process status: {e}"),
                    });
                }
            }
        }

        terminate_pty_child(&mut child_state.child).await;
        join_pty_io_thread(child_state.io_thread.take());

        let transcript = match child_state.transcript.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => String::new(),
        };

        if let Some(degraded) = trust_policy_result(&transcript, config.codex_auto_trust) {
            return Ok(degraded);
        }

        if Instant::now() >= deadline {
            return Ok(PtyForkResult::Failed {
                error: format!(
                    "timed out after {}s waiting for child session (marker='{marker}', parent='{parent_session_id}')",
                    config.poll_timeout_secs.max(1)
                ),
            });
        }

        Ok(PtyForkResult::Failed {
            error: format!(
                "codex fork exited before child session was discovered (exit_code={})",
                final_exit
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            ),
        })
    }
}

fn ulid_regex() -> &'static Regex {
    static ULID_RE: OnceLock<Regex> = OnceLock::new();
    ULID_RE.get_or_init(|| Regex::new(r"^[0-9A-HJKMNP-TV-Z]{26}$").expect("ULID regex is valid"))
}

fn is_valid_ulid_session_id(session_id: &str) -> bool {
    ulid_regex().is_match(session_id)
}

#[derive(Debug)]
struct ForkSupport {
    supported: bool,
    reason: String,
}

async fn detect_codex_fork_support(codex_path: &Path) -> Result<ForkSupport> {
    match Command::new(codex_path).arg("--version").output().await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ForkSupport {
                supported: false,
                reason: format!(
                    "codex binary not found or not executable via PATH: {}",
                    codex_path.display()
                ),
            });
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed probing '{} --version'", codex_path.display()));
        }
    }

    let output = match Command::new(codex_path).arg("--help").output().await {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ForkSupport {
                supported: false,
                reason: format!(
                    "codex binary not found or not executable via PATH: {}",
                    codex_path.display()
                ),
            });
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed running '{} --help'", codex_path.display()));
        }
    };

    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let supports_fork = help.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("fork ") || trimmed == "fork"
    });

    if supports_fork {
        Ok(ForkSupport {
            supported: true,
            reason: String::new(),
        })
    } else {
        Ok(ForkSupport {
            supported: false,
            reason: "codex binary does not expose 'fork' subcommand".to_string(),
        })
    }
}

fn trust_policy_result(output: &str, codex_auto_trust: bool) -> Option<PtyForkResult> {
    if codex_auto_trust {
        return None;
    }
    if detect_trust_dialog(output) {
        return Some(PtyForkResult::Degraded {
            reason: "codex trust dialog detected and codex_auto_trust=false".to_string(),
        });
    }
    None
}

fn detect_trust_dialog(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    TRUST_DIALOG_PATTERNS
        .iter()
        .any(|needle| lowered.contains(needle))
}

fn current_unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(unix)]
struct PtyChildState {
    child: std::process::Child,
    trust_detected: Arc<AtomicBool>,
    transcript: Arc<Mutex<String>>,
    io_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
fn spawn_codex_fork_pty(
    codex_path: &Path,
    parent_session_id: &str,
    prompt_marker: &str,
) -> Result<PtyChildState> {
    let pty = openpty(
        Some(&Winsize {
            ws_row: 48,
            ws_col: 160,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .context("failed to allocate PTY")?;

    let slave_fd_raw = pty.slave.as_raw_fd();
    let stdin_fd = dup(slave_fd_raw).context("failed to dup PTY slave for stdin")?;
    let stdout_fd = dup(slave_fd_raw).context("failed to dup PTY slave for stdout")?;
    let stderr_fd = dup(slave_fd_raw).context("failed to dup PTY slave for stderr")?;

    let mut cmd = StdCommand::new(codex_path);
    cmd.arg("fork")
        .arg(parent_session_id)
        .arg(prompt_marker)
        .arg("--dangerously-bypass-approvals-and-sandbox")
        .arg("--no-alt-screen")
        .stdin(Stdio::from(File::from(raw_fd_to_owned_fd(stdin_fd))))
        .stdout(Stdio::from(File::from(raw_fd_to_owned_fd(stdout_fd))))
        .stderr(Stdio::from(File::from(raw_fd_to_owned_fd(stderr_fd))));

    // SAFETY: `pre_exec` runs in the child process before `exec`. We only call
    // async-signal-safe operations (`setsid`, `ioctl(TIOCSCTTY)`) and return an
    // `io::Result` without touching shared Rust state.
    unsafe {
        cmd.pre_exec(move || {
            setsid().map_err(nix_errno_to_io_error)?;
            // SAFETY: ioctl with TIOCSCTTY establishes the PTY slave as the
            // controlling terminal for this freshly-created session.
            let rc = libc::ioctl(slave_fd_raw, libc::TIOCSCTTY as _, 0);
            if rc == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn '{}'", codex_path.display()))?;

    // Parent no longer needs slave side.
    drop(pty.slave);

    let mut pty_reader = File::from(pty.master);
    let mut pty_writer = pty_reader
        .try_clone()
        .context("failed to clone PTY master for writer")?;

    let trust_detected = Arc::new(AtomicBool::new(false));
    let transcript = Arc::new(Mutex::new(String::new()));

    let trust_detected_clone = Arc::clone(&trust_detected);
    let transcript_clone = Arc::clone(&transcript);

    let io_thread = std::thread::Builder::new()
        .name("codex-pty-fork-io".to_string())
        .spawn(move || {
            run_pty_io_loop(
                &mut pty_reader,
                &mut pty_writer,
                &trust_detected_clone,
                &transcript_clone,
            )
        })
        .context("failed to spawn PTY IO thread")?;

    Ok(PtyChildState {
        child,
        trust_detected,
        transcript,
        io_thread: Some(io_thread),
    })
}

#[cfg(unix)]
fn run_pty_io_loop(
    pty_reader: &mut File,
    pty_writer: &mut File,
    trust_detected: &Arc<AtomicBool>,
    transcript: &Arc<Mutex<String>>,
) {
    let mut read_buf = [0_u8; 4096];
    let mut tail = Vec::with_capacity(128);

    loop {
        match pty_reader.read(&mut read_buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &read_buf[..n];
                if let Ok(mut guard) = transcript.lock() {
                    guard.push_str(&String::from_utf8_lossy(chunk));
                }

                let mut window = Vec::with_capacity(tail.len() + chunk.len());
                window.extend_from_slice(&tail);
                window.extend_from_slice(chunk);

                for response in handshake_responses(&window) {
                    let _ = pty_writer.write_all(response);
                    let _ = pty_writer.flush();
                }

                if detect_trust_dialog(&String::from_utf8_lossy(&window)) {
                    trust_detected.store(true, Ordering::Relaxed);
                }

                // Keep short tail for detecting split control sequences.
                tail = window.iter().rev().take(96).copied().collect();
                tail.reverse();
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[cfg(unix)]
fn handshake_responses(window: &[u8]) -> Vec<&'static [u8]> {
    let mut replies = Vec::new();
    if contains_bytes(window, HANDSHAKE_QUERY_CURSOR_POS) {
        replies.push(HANDSHAKE_REPLY_CURSOR_POS);
    }
    if contains_bytes(window, HANDSHAKE_QUERY_DEVICE_ATTR) {
        replies.push(HANDSHAKE_REPLY_DEVICE_ATTR);
    }
    if contains_bytes(window, HANDSHAKE_QUERY_KITTY_KEYBOARD) {
        replies.push(HANDSHAKE_REPLY_KITTY_KEYBOARD);
    }
    if contains_bytes(window, HANDSHAKE_QUERY_OSC10) {
        replies.push(HANDSHAKE_REPLY_OSC10);
    }
    replies
}

#[cfg(unix)]
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(unix)]
fn nix_errno_to_io_error(errno: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(errno as i32)
}

#[cfg(unix)]
async fn terminate_pty_child(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    if pid > 0 {
        let pgid = Pid::from_raw(pid);
        let _ = killpg(pgid, Signal::SIGTERM);
        tokio::time::sleep(Duration::from_millis(300)).await;

        if child.try_wait().ok().flatten().is_some() {
            return;
        }

        let _ = killpg(pgid, Signal::SIGKILL);
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn raw_fd_to_owned_fd(raw_fd: i32) -> OwnedFd {
    // SAFETY: `raw_fd` comes from successful `dup(2)` calls and is uniquely
    // owned by this function, so converting to `OwnedFd` is valid.
    unsafe { OwnedFd::from_raw_fd(raw_fd) }
}

#[cfg(unix)]
fn join_pty_io_thread(handle: Option<std::thread::JoinHandle<()>>) {
    if let Some(join_handle) = handle {
        let _ = join_handle.join();
    }
}

#[cfg(unix)]
fn codex_db_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home_dir) = home {
        let codex_dir = home_dir.join(".codex");

        if let Ok(entries) = fs::read_dir(&codex_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_sqlite = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sqlite"));
                if is_sqlite {
                    paths.push(path);
                }
            }
        }

        // Backward-compatible fallback names.
        paths.push(codex_dir.join("state.sqlite"));
        paths.push(codex_dir.join("sessions.sqlite"));
    }

    paths.sort();
    paths.dedup();
    paths
}

#[cfg(unix)]
fn query_child_session_once(
    parent_session_id: &str,
    session_marker: &str,
    poll_started_at: i64,
    db_candidates: &[PathBuf],
) -> Result<Option<String>> {
    for db_path in db_candidates {
        if !db_path.exists() {
            continue;
        }

        let conn = match Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        if let Some(id) =
            query_parent_linked_tables(&conn, parent_session_id, session_marker, poll_started_at)?
        {
            return Ok(Some(id));
        }

        if let Some(id) =
            query_threads_fallback(&conn, parent_session_id, session_marker, poll_started_at)?
        {
            return Ok(Some(id));
        }
    }

    Ok(None)
}

#[cfg(unix)]
fn query_parent_linked_tables(
    conn: &Connection,
    parent_session_id: &str,
    session_marker: &str,
    poll_started_at: i64,
) -> Result<Option<String>> {
    const SESSION_COLS: &[&str] = &["session_id", "id", "child_session_id", "thread_id"];
    const PARENT_COLS: &[&str] = &["parent_session_id", "parent_id", "fork_of_session_id"];
    const MARKER_COLS: &[&str] = &[
        "prompt",
        "message",
        "content",
        "title",
        "first_user_message",
    ];
    const TIME_COLS: &[&str] = &["created_at", "updated_at", "timestamp", "ts"];

    let tables = list_tables(conn)?;
    for table in tables {
        let columns = table_columns(conn, &table)?;

        let session_col = pick_column(&columns, SESSION_COLS);
        let parent_col = pick_column(&columns, PARENT_COLS);
        let marker_col = pick_column(&columns, MARKER_COLS);
        let time_col = pick_column(&columns, TIME_COLS);

        let (Some(session_col), Some(parent_col), Some(marker_col), Some(time_col)) =
            (session_col, parent_col, marker_col, time_col)
        else {
            continue;
        };

        let Some(table_q) = quote_sql_ident(&table) else {
            continue;
        };
        let Some(session_q) = quote_sql_ident(session_col) else {
            continue;
        };
        let Some(parent_q) = quote_sql_ident(parent_col) else {
            continue;
        };
        let Some(marker_q) = quote_sql_ident(marker_col) else {
            continue;
        };
        let Some(time_q) = quote_sql_ident(time_col) else {
            continue;
        };

        let sql = format!(
            "SELECT {session_q} \
             FROM {table_q} \
             WHERE {parent_q} = ?1 \
               AND instr(lower(COALESCE({marker_q}, '')), lower(?2)) > 0 \
               AND (CAST({time_q} AS INTEGER) >= ?3 OR CAST(strftime('%s', {time_q}) AS INTEGER) >= ?3) \
             ORDER BY CAST({time_q} AS INTEGER) DESC, rowid DESC \
             LIMIT 1"
        );

        let mut stmt = match conn.prepare(&sql) {
            Ok(stmt) => stmt,
            Err(_) => continue,
        };

        let mut rows = stmt
            .query(rusqlite::params![
                parent_session_id,
                session_marker,
                poll_started_at
            ])
            .context("failed querying parent-linked session table")?;

        if let Some(row) = rows.next().context("failed reading query row")? {
            let session_id: String = row.get(0).context("failed reading child session id")?;
            if !session_id.trim().is_empty() {
                return Ok(Some(session_id));
            }
        }
    }

    Ok(None)
}

#[cfg(unix)]
fn query_threads_fallback(
    conn: &Connection,
    parent_session_id: &str,
    session_marker: &str,
    poll_started_at: i64,
) -> Result<Option<String>> {
    let sql = "SELECT id FROM threads \
               WHERE created_at >= ?1 \
                 AND instr(lower(first_user_message), lower(?2)) > 0 \
                 AND instr(lower(first_user_message), lower(?3)) > 0 \
               ORDER BY created_at DESC \
               LIMIT 1";

    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(_) => return Ok(None),
    };

    let mut rows = stmt
        .query(rusqlite::params![
            poll_started_at,
            session_marker,
            parent_session_id
        ])
        .context("failed querying fallback threads table")?;

    if let Some(row) = rows.next().context("failed reading fallback row")? {
        let id: String = row.get(0).context("failed reading threads.id")?;
        if !id.trim().is_empty() {
            return Ok(Some(id));
        }
    }

    Ok(None)
}

#[cfg(unix)]
fn list_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .context("failed preparing sqlite table listing")?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .context("failed reading sqlite table names")?;

    let mut names = Vec::new();
    for row in rows {
        names.push(row.context("failed decoding sqlite table name")?);
    }
    Ok(names)
}

#[cfg(unix)]
fn table_columns(conn: &Connection, table_name: &str) -> Result<Vec<String>> {
    let Some(table_q) = quote_sql_ident(table_name) else {
        return Ok(Vec::new());
    };

    let sql = format!("PRAGMA table_info({table_q})");
    let mut stmt = conn
        .prepare(&sql)
        .context("failed preparing PRAGMA table_info")?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed reading table columns")?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.context("failed decoding column name")?);
    }
    Ok(columns)
}

#[cfg(unix)]
fn pick_column<'a>(columns: &'a [String], candidates: &[&str]) -> Option<&'a str> {
    candidates.iter().find_map(|candidate| {
        columns
            .iter()
            .find(|column| column.eq_ignore_ascii_case(candidate))
            .map(|s| s.as_str())
    })
}

#[cfg(unix)]
fn quote_sql_ident(ident: &str) -> Option<String> {
    static IDENT_RE: OnceLock<Regex> = OnceLock::new();
    let re = IDENT_RE.get_or_init(|| {
        Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("identifier regex is valid")
    });
    if !re.is_match(ident) {
        return None;
    }
    Some(format!("\"{ident}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_validation_rejects_malicious_session_id() {
        assert!(is_valid_ulid_session_id("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(!is_valid_ulid_session_id(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV;rm -rf /"
        ));
        assert!(!is_valid_ulid_session_id("../01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    }

    #[test]
    fn pty_fork_config_defaults_are_correct() {
        let cfg = PtyForkConfig::default();
        assert!(!cfg.codex_auto_trust);
        assert_eq!(cfg.poll_timeout_secs, DEFAULT_POLL_TIMEOUT_SECS);
        assert_eq!(cfg.session_marker, DEFAULT_SESSION_MARKER);
    }

    #[test]
    fn trust_dialog_detection_degrades_when_auto_trust_disabled() {
        let output = "Security check: Do you trust this folder before continuing?";
        let result = trust_policy_result(output, false);
        assert!(matches!(result, Some(PtyForkResult::Degraded { .. })));
    }

    #[tokio::test]
    async fn version_detection_missing_codex_binary_degrades_gracefully() {
        let cfg = PtyForkConfig::default();
        let valid_ulid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let missing = Path::new("/tmp/definitely-missing-codex-binary");

        let result = fork_codex_session(valid_ulid, missing, &cfg)
            .await
            .expect("fork should return result, not panic");

        assert!(matches!(result, PtyForkResult::Degraded { .. }));
    }
}
