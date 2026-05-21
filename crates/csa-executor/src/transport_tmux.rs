//! TmuxTransport — runs Claude Code inside a detached tmux session.
//!
//! ## Why tmux?
//!
//! Anthropic's interactive billing pool (unlimited) applies to terminal-based
//! Claude Code sessions. The tmux transport wraps Claude in a real interactive
//! terminal session so CSA usage is billed there rather than against Agent SDK
//! credits (June 2026 cap). This is an **Experimental** transport — billing
//! classification may change.
//!
//! ## Lifecycle
//!
//! 1. Spawn `tmux new-session -d -s csa-<ULID> -- claude --dangerously-skip-permissions`
//! 2. Get pane PID: `tmux display-message -p '#{pane_pid}'`
//! 3. Discover Claude session: `~/.claude/sessions/<pid>.json` → `sessionId`
//! 4. Find JSONL log: `~/.claude/projects/**/<sessionId>.jsonl`
//! 5. Wait for JSONL creation (readiness, 30s timeout)
//! 6. Send prompt: `tmux load-buffer` → `paste-buffer` → `send-keys Enter`
//! 7. Tail JSONL until `type=system subtype=turn_duration` marker
//! 8. Kill tmux session on Drop (`TmuxCleanupGuard`)
//!
//! ## Limitations
//!
//! - Incompatible with bwrap/landlock filesystem sandbox (tmux spawns outside
//!   the sandbox namespace). Set `[filesystem_sandbox] enforcement_mode = "off"`
//!   when using this transport.
//! - No session resume: each invocation creates a fresh Claude session.
//! - No fork support.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use csa_core::transport_events::StreamingMetadata;
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::Deserialize;
use tokio::time::sleep;

use crate::executor::Executor;

use super::{
    ResolvedTimeout, Transport, TransportCapabilities, TransportMode, TransportOptions,
    TransportResult,
};

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
const SESSION_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

// ── RAII cleanup guard ────────────────────────────────────────────────────────

/// Kills the tmux session on Drop so no orphan sessions are left behind.
struct TmuxCleanupGuard {
    session_name: String,
}

impl Drop for TmuxCleanupGuard {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.session_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ── Session discovery ─────────────────────────────────────────────────────────

/// Claude Code session metadata from `~/.claude/sessions/<pid>.json`.
#[derive(Debug, Deserialize)]
struct ClaudeSessionMeta {
    #[serde(rename = "sessionId")]
    session_id: String,
}

fn claude_root() -> Result<PathBuf> {
    let home = home_dir().context("cannot determine HOME directory")?;
    Ok(home.join(".claude"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Read `~/.claude/sessions/<pid>.json` to get Claude's session ID, then verify
/// that PID <pid> still runs `claude` (guards against PID reuse).
fn read_session_meta(pid: u32) -> Result<ClaudeSessionMeta> {
    let root = claude_root()?;
    let meta_path = root.join("sessions").join(format!("{pid}.json"));

    if !meta_path.exists() {
        bail!(
            "Claude session metadata not yet written at {}",
            meta_path.display()
        );
    }

    // Verify cmdline to catch PID reuse before trusting the metadata.
    let cmdline_path = format!("/proc/{pid}/cmdline");
    let cmdline = fs::read_to_string(&cmdline_path)
        .unwrap_or_default()
        .replace('\0', " ");
    if !cmdline.to_lowercase().contains("claude") {
        bail!(
            "PID {pid} cmdline does not contain 'claude' (got: {:?}); \
             possible PID reuse — not safe to read session metadata",
            &cmdline[..cmdline.len().min(120)]
        );
    }

    let content =
        fs::read_to_string(&meta_path).with_context(|| meta_path.display().to_string())?;
    let meta: ClaudeSessionMeta =
        serde_json::from_str(&content).with_context(|| format!("parse {}", meta_path.display()))?;
    Ok(meta)
}

/// Walk `~/.claude/projects/` one level deep and return the path for
/// `<session_id>.jsonl` when found.  Checks `sessions-index.json` first for
/// efficiency, then falls back to filename search.
fn find_jsonl_path(claude_root: &Path, session_id: &str) -> Option<PathBuf> {
    let projects = claude_root.join("projects");
    if !projects.exists() {
        return None;
    }

    let needle = format!("{session_id}.jsonl");
    let Ok(project_dirs) = fs::read_dir(&projects) else {
        return None;
    };

    for entry in project_dirs.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Fast path: check sessions-index.json for the session_id.
        let index_path = path.join("sessions-index.json");
        if index_path.exists()
            && let Some(jsonl) = find_in_sessions_index(&index_path, session_id)
            && jsonl.exists()
        {
            return Some(jsonl);
        }

        // Filename-based fallback.
        let candidate = path.join(&needle);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

#[derive(Deserialize)]
struct SessionsIndex {
    #[serde(default)]
    entries: Vec<SessionsIndexEntry>,
}

#[derive(Deserialize)]
struct SessionsIndexEntry {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "fullPath")]
    full_path: Option<PathBuf>,
}

fn find_in_sessions_index(index_path: &Path, session_id: &str) -> Option<PathBuf> {
    let content = fs::read_to_string(index_path).ok()?;
    let index: SessionsIndex = serde_json::from_str(&content).ok()?;
    index
        .entries
        .into_iter()
        .find(|e| e.session_id == session_id)
        .and_then(|e| e.full_path)
}

// ── JSONL watcher ─────────────────────────────────────────────────────────────

/// Validate that the first few JSONL events contain the expected fields
/// (`type`, `sessionId`, `timestamp`).  Fails fast if the schema has changed.
fn validate_jsonl_schema(jsonl_path: &Path) -> Result<()> {
    let file = fs::File::open(jsonl_path).with_context(|| jsonl_path.display().to_string())?;
    let reader = std::io::BufReader::new(file);
    let mut checked = 0u32;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                checked += 1;
                continue;
            }
        };
        // Require at minimum a `type` field in the first parseable event.
        if value.get("type").is_none() {
            bail!(
                "Incompatible Claude JSONL schema at {}: first event lacks 'type' field. \
                 Claude Code may have changed its conversation log format.",
                jsonl_path.display()
            );
        }
        checked += 1;
        if checked >= 3 {
            break;
        }
    }
    Ok(())
}

/// Events extracted from the JSONL watcher.
#[derive(Debug)]
enum JsonlEvent {
    AssistantText(String),
    TurnDuration,
    CompactBoundary,
}

/// Parse a single JSONL line into a `JsonlEvent`.
fn parse_jsonl_line(line: &str) -> Option<JsonlEvent> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let event_type = value.get("type")?.as_str()?;

    match event_type {
        "assistant" => {
            let text = extract_assistant_text(&value).unwrap_or_default();
            Some(JsonlEvent::AssistantText(text))
        }
        "system" => {
            let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            match subtype {
                "turn_duration" => Some(JsonlEvent::TurnDuration),
                "compact_boundary" => Some(JsonlEvent::CompactBoundary),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Extract the text content from an `assistant` JSONL event.
///
/// Claude's conversation log stores assistant text in:
/// `{"type": "assistant", "message": {"content": [{"type": "text", "text": "..."}]}}`
fn extract_assistant_text(value: &serde_json::Value) -> Option<String> {
    let message = value.get("message")?;
    let content = message.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                block.get("text").and_then(|t| t.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() { None } else { Some(text) }
}

/// Poll the JSONL file until a `turn_duration` event is seen, then return
/// the collected assistant text from that turn.
///
/// Handles:
/// - File not yet existing: retries with `POLL_INTERVAL` backoff.
/// - `compact_boundary`: resets byte offset to 0 (Claude rewrote the log).
/// - EOF without event: continues polling.
/// - `idle_timeout_seconds`: returns error if no `turn_duration` in time.
async fn watch_jsonl_for_turn(jsonl_path: &Path, idle_timeout_seconds: u64) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(idle_timeout_seconds);
    let mut byte_offset: u64 = 0;
    let mut collected_text = String::new();

    loop {
        if Instant::now() > deadline {
            bail!(
                "JSONL watcher timed out after {}s waiting for turn_duration; \
                 collected {} chars of text so far",
                idle_timeout_seconds,
                collected_text.len()
            );
        }

        // Try to open and read new data from the current offset.
        match fs::File::open(jsonl_path) {
            Err(_) => {
                sleep(POLL_INTERVAL).await;
                continue;
            }
            Ok(mut file) => {
                if file.seek(SeekFrom::Start(byte_offset)).is_err() {
                    // File may have been truncated (compaction); reset.
                    byte_offset = 0;
                    collected_text.clear();
                    sleep(POLL_INTERVAL).await;
                    continue;
                }

                let reader = std::io::BufReader::new(&mut file);
                let mut advanced = false;

                for line in reader.lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    // Advance offset by line bytes + newline.
                    byte_offset += line.len() as u64 + 1;
                    advanced = true;

                    match parse_jsonl_line(&line) {
                        Some(JsonlEvent::AssistantText(text)) => {
                            collected_text.push_str(&text);
                        }
                        Some(JsonlEvent::TurnDuration) => {
                            return Ok(collected_text);
                        }
                        Some(JsonlEvent::CompactBoundary) => {
                            // Claude compacted context; restart from new file beginning.
                            byte_offset = 0;
                            collected_text.clear();
                            tracing::debug!(
                                path = %jsonl_path.display(),
                                "tmux transport: JSONL compact_boundary detected; resetting watcher"
                            );
                        }
                        None => {}
                    }
                }

                if !advanced {
                    sleep(POLL_INTERVAL).await;
                }
            }
        }
    }
}

// ── Prompt delivery ───────────────────────────────────────────────────────────

/// Deliver `prompt` to the tmux session via load-buffer / paste-buffer / Enter.
///
/// This is safer than `send-keys` for prompts containing special characters,
/// shell metacharacters, multi-byte UTF-8, or large payloads.
fn deliver_prompt(session_name: &str, prompt: &str) -> Result<()> {
    // Use a named buffer (session_name) to avoid cross-session races when
    // multiple CSA tmux sessions run concurrently.
    let buffer_name = session_name;

    // load-buffer reads from stdin; pipe the raw prompt text in.
    let mut load = Command::new("tmux")
        .args(["load-buffer", "-b", buffer_name, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("tmux load-buffer failed to spawn")?;

    if let Some(stdin) = load.stdin.take() {
        use std::io::Write;
        let mut stdin = stdin;
        stdin
            .write_all(prompt.as_bytes())
            .context("writing prompt to tmux load-buffer stdin")?;
    }
    let status = load.wait().context("tmux load-buffer wait")?;
    if !status.success() {
        bail!("tmux load-buffer exited with {status}");
    }

    // Paste the named buffer into the target pane (deletes it after paste).
    let status = Command::new("tmux")
        .args(["paste-buffer", "-b", buffer_name, "-d", "-t", session_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("tmux paste-buffer")?;
    if !status.success() {
        bail!("tmux paste-buffer exited with {status}");
    }

    // Send Enter to submit the prompt.
    let status = Command::new("tmux")
        .args(["send-keys", "-t", session_name, "Enter"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("tmux send-keys Enter")?;
    if !status.success() {
        bail!("tmux send-keys Enter exited with {status}");
    }

    Ok(())
}

// ── TmuxTransport ─────────────────────────────────────────────────────────────

/// Executes Claude Code inside a detached tmux session and reads output via the
/// JSONL conversation log.
#[derive(Debug, Clone)]
pub struct TmuxTransport {
    executor: Executor,
}

impl TmuxTransport {
    pub fn new(executor: Executor) -> Self {
        Self { executor }
    }

    /// Build a ULID-based tmux session name.
    fn session_name() -> String {
        format!("csa-{}", csa_session::new_session_id())
    }

    /// Spawn a detached tmux session running `claude --dangerously-skip-permissions`.
    async fn spawn_tmux(
        session_name: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        model_override: Option<&str>,
        thinking_budget: Option<&crate::model_spec::ThinkingBudget>,
    ) -> Result<()> {
        let work_dir_str = work_dir.to_str().context("work_dir is not valid UTF-8")?;

        let mut claude_args = vec![
            "claude".to_string(),
            "--dangerously-skip-permissions".to_string(),
        ];
        if let Some(model) = model_override {
            claude_args.push("--model".into());
            claude_args.push(model.to_string());
        }
        if let Some(budget) = thinking_budget
            && let Some(level) = budget.claude_effort()
        {
            claude_args.push("--effort".into());
            claude_args.push(level.to_string());
        }

        let mut tmux_args: Vec<&str> = vec![
            "new-session",
            "-d",
            "-s",
            session_name,
            "-c",
            work_dir_str,
            "--",
        ];
        let claude_arg_refs: Vec<&str> = claude_args.iter().map(String::as_str).collect();
        tmux_args.extend_from_slice(&claude_arg_refs);

        let mut cmd = tokio::process::Command::new("tmux");
        cmd.args(&tmux_args);

        // Strip CSA recursion guards from the child's environment.
        for var in [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "LEFTHOOK",
            "LEFTHOOK_SKIP",
            "CSA_SESSION_ID",
            "CSA_SESSION_DIR",
            "CSA_PARENT_SESSION",
            "CSA_PARENT_SESSION_DIR",
            "CSA_DAEMON_SESSION_DIR",
        ] {
            cmd.env_remove(var);
        }

        if let Some(env) = extra_env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let status = cmd.status().await.context("tmux new-session")?;
        if !status.success() {
            bail!("tmux new-session exited with {status}");
        }
        Ok(())
    }

    /// Get the PID of the process running in the given tmux pane.
    async fn pane_pid(session_name: &str) -> Result<u32> {
        let output = tokio::process::Command::new("tmux")
            .args(["display-message", "-p", "-t", session_name, "#{pane_pid}"])
            .output()
            .await
            .context("tmux display-message")?;
        let raw = String::from_utf8_lossy(&output.stdout);
        raw.trim()
            .parse::<u32>()
            .context("pane_pid from tmux is not a valid u32")
    }

    /// Poll until `~/.claude/sessions/<pid>.json` exists and yields a session ID,
    /// then find the corresponding JSONL log in `~/.claude/projects/`.
    async fn discover_jsonl(session_name: &str) -> Result<(PathBuf, String)> {
        let deadline = Instant::now() + SESSION_DISCOVERY_TIMEOUT;
        let mut backoff = Duration::from_millis(200);
        let root = claude_root()?;

        loop {
            let pid = Self::pane_pid(session_name).await?;

            if let Ok(meta) = read_session_meta(pid) {
                if let Some(jsonl) = find_jsonl_path(&root, &meta.session_id) {
                    return Ok((jsonl, meta.session_id));
                }
                tracing::debug!(
                    session_id = %meta.session_id,
                    "tmux transport: JSONL not yet written; retrying"
                );
            }

            if Instant::now() + backoff > deadline {
                bail!(
                    "tmux transport: Claude Code did not produce a session file within {}s. \
                     Ensure `claude` is installed and accessible in the tmux environment.",
                    SESSION_DISCOVERY_TIMEOUT.as_secs()
                );
            }
            sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
    }

    /// Wait for the JSONL file to be created (Claude is ready for input).
    async fn wait_for_readiness(jsonl_path: &Path) -> Result<()> {
        let deadline = Instant::now() + READINESS_TIMEOUT;
        loop {
            if jsonl_path.exists() {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!(
                    "tmux transport: Claude Code did not create its JSONL log at {} \
                     within {}s. Claude may have failed to start.",
                    jsonl_path.display(),
                    READINESS_TIMEOUT.as_secs()
                );
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    /// Full session lifecycle: spawn → discover → ready → prompt → watch → result.
    async fn execute_session(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        idle_timeout_seconds: u64,
    ) -> Result<TransportResult> {
        let session_name = Self::session_name();
        tracing::debug!(
            session = %session_name,
            work_dir = %work_dir.display(),
            "tmux transport: spawning session"
        );

        let (model_override, thinking_budget) = match &self.executor {
            Executor::ClaudeCode {
                model_override,
                thinking_budget,
                ..
            } => (model_override.as_deref(), thinking_budget.as_ref()),
            _ => (None, None),
        };
        Self::spawn_tmux(
            &session_name,
            work_dir,
            extra_env,
            model_override,
            thinking_budget,
        )
        .await?;
        // RAII guard: kills the session when this function exits (normal or panic).
        let _guard = TmuxCleanupGuard {
            session_name: session_name.clone(),
        };

        let (jsonl_path, provider_session_id) = Self::discover_jsonl(&session_name).await?;
        tracing::debug!(
            jsonl = %jsonl_path.display(),
            session_id = %provider_session_id,
            "tmux transport: discovered JSONL log"
        );

        Self::wait_for_readiness(&jsonl_path).await?;

        // Schema validation: confirm the JSONL format matches expectations.
        if let Err(e) = validate_jsonl_schema(&jsonl_path) {
            tracing::warn!(error = %e, "tmux transport: JSONL schema validation failed");
            bail!(e);
        }

        deliver_prompt(&session_name, prompt)?;
        tracing::debug!(
            session = %session_name,
            prompt_len = prompt.len(),
            "tmux transport: prompt delivered"
        );

        let output_text = watch_jsonl_for_turn(&jsonl_path, idle_timeout_seconds).await?;

        tracing::debug!(
            session = %session_name,
            output_len = output_text.len(),
            "tmux transport: turn complete"
        );

        Ok(TransportResult {
            execution: ExecutionResult {
                summary: output_text.clone(),
                output: output_text,
                stderr_output: String::new(),
                exit_code: 0,
                peak_memory_mb: None,
            },
            provider_session_id: Some(provider_session_id),
            events: Vec::new(),
            metadata: StreamingMetadata::default(),
        })
    }
}

#[async_trait]
impl Transport for TmuxTransport {
    fn mode(&self) -> TransportMode {
        TransportMode::Tmux
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            streaming: false,
            session_resume: false,
            session_fork: false,
            typed_events: false,
        }
    }

    async fn execute(
        &self,
        prompt: &str,
        _tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        tracing::debug!(tool = %self.executor.tool_name(), "tmux transport: execute");

        if options.sandbox.is_some() {
            tracing::warn!(
                "tmux transport: sandbox options are ignored — tmux sessions run \
                 outside the sandbox namespace. Set [filesystem_sandbox] \
                 enforcement_mode = \"off\" to suppress this warning."
            );
        }

        let work_dir = PathBuf::from(&session.project_path);
        self.execute_session(prompt, &work_dir, extra_env, options.idle_timeout_seconds)
            .await
    }

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        _stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        _initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        self.execute_session(prompt, work_dir, extra_env, idle_timeout_seconds)
            .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── csa gc integration ────────────────────────────────────────────────────────

/// Name prefix for all CSA-owned tmux sessions.
const CSA_TMUX_SESSION_PREFIX: &str = "csa-";

/// List all tmux sessions whose name starts with `csa-`.
///
/// Returns `(session_name, session_id)` pairs where `session_id` is the ULID
/// suffix after the `csa-` prefix.
pub fn list_csa_tmux_sessions() -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output();

    let output = match output {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // tmux not installed — no sessions to report.
            return Ok(Vec::new());
        }
        Err(e) => return Err(e).context("tmux list-sessions"),
        Ok(o) => o,
    };

    if !output.status.success() {
        // No sessions exist (tmux exits non-zero when no server is running).
        return Ok(Vec::new());
    }

    let sessions = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|name| name.starts_with(CSA_TMUX_SESSION_PREFIX))
        .map(String::from)
        .collect();

    Ok(sessions)
}

/// Kill a single tmux session by name. No-op if the session no longer exists.
pub fn kill_tmux_session(session_name: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["kill-session", "-t", session_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("tmux kill-session")?;
    // Non-zero exit is normal when the session is already gone.
    let _ = status;
    Ok(())
}

/// Reap orphan `csa-*` tmux sessions whose corresponding CSA session is no
/// longer active.  Called by `csa gc`.
///
/// `active_session_ids` is the set of CSA session ULIDs that are still alive.
/// Any tmux session named `csa-<ULID>` where `<ULID>` is not in this set is
/// considered orphaned and will be killed (unless `dry_run` is true).
pub fn reap_orphan_tmux_sessions(
    active_session_ids: &std::collections::HashSet<String>,
    dry_run: bool,
) -> Result<TmuxReapStats> {
    let sessions = list_csa_tmux_sessions()?;
    let mut stats = TmuxReapStats::default();

    for session_name in &sessions {
        let ulid = session_name
            .strip_prefix(CSA_TMUX_SESSION_PREFIX)
            .unwrap_or(session_name);

        if active_session_ids.contains(ulid) {
            continue;
        }

        stats.orphans_found += 1;
        tracing::info!(
            session = %session_name,
            dry_run,
            "tmux transport gc: orphan session"
        );

        if !dry_run {
            kill_tmux_session(session_name)?;
            stats.orphans_killed += 1;
        }
    }

    Ok(stats)
}

/// Statistics from a tmux gc sweep.
#[derive(Debug, Default, Clone)]
pub struct TmuxReapStats {
    pub orphans_found: u64,
    pub orphans_killed: u64,
}

#[cfg(test)]
#[path = "transport_tmux_tests.rs"]
mod tests;
