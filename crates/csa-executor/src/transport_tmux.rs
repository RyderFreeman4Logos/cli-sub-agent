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
//! 1. Snapshot existing `.jsonl` files in `~/.claude/projects/<escaped-path>/`
//! 2. Spawn `tmux new-session -d -s csa-<ULID> -- /path/to/claude --dangerously-skip-permissions`
//! 3. Wait for Claude TUI readiness (poll tmux pane for `❯` prompt)
//! 4. Send prompt: `tmux load-buffer` → `paste-buffer` → `send-keys Enter`
//! 5. Discover new JSONL: diff snapshot to find newly created `.jsonl` file
//! 6. Tail JSONL until `type=system subtype=turn_duration` marker
//! 7. Kill tmux session on Drop (`TmuxCleanupGuard`)
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

fn claude_root() -> Result<PathBuf> {
    let home = home_dir().context("cannot determine HOME directory")?;
    Ok(home.join(".claude"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Escape a project path the way Claude Code does for its
/// `~/.claude/projects/` directory names: replace every `/` with `-`.
fn escape_project_path(work_dir: &Path) -> String {
    work_dir.to_string_lossy().replace('/', "-")
}

/// Return the Claude projects directory for the given work_dir.
fn project_jsonl_dir(work_dir: &Path) -> Result<PathBuf> {
    let root = claude_root()?;
    let escaped = escape_project_path(work_dir);
    Ok(root.join("projects").join(escaped))
}

/// Snapshot all `.jsonl` files in the project's Claude directory.
fn snapshot_jsonl_files(project_dir: &Path) -> std::collections::HashSet<PathBuf> {
    let mut set = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                set.insert(path);
            }
        }
    }
    set
}

/// Poll until a new `.jsonl` file appears that wasn't in `before`.
/// Returns the path and the session ID (UUID filename stem).
async fn discover_new_jsonl(
    project_dir: &Path,
    before: &std::collections::HashSet<PathBuf>,
) -> Result<(PathBuf, String)> {
    discover_new_jsonl_with_timeout(project_dir, before, SESSION_DISCOVERY_TIMEOUT).await
}

async fn discover_new_jsonl_with_timeout(
    project_dir: &Path,
    before: &std::collections::HashSet<PathBuf>,
    timeout: Duration,
) -> Result<(PathBuf, String)> {
    let deadline = Instant::now() + timeout;
    let mut backoff = Duration::from_millis(500);

    loop {
        if let Ok(entries) = fs::read_dir(project_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && !before.contains(&path)
                {
                    let session_id = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    return Ok((path, session_id));
                }
            }
        }

        if Instant::now() + backoff > deadline {
            bail!(
                "tmux transport: no new JSONL file appeared in {} within {}s. \
                 Claude Code may have failed to start or process the prompt.",
                project_dir.display(),
                timeout.as_secs()
            );
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(2));
    }
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
///
/// A delay between paste-buffer and Enter is required because Claude Code's
/// TUI uses bracketed-paste mode: the paste event must finish processing before
/// Enter can submit the message.
async fn deliver_prompt(session_name: &str, prompt: &str) -> Result<()> {
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

    // Wait for Claude Code's TUI to finish processing the bracketed paste.
    // Large prompts (>10K chars) need more time for the TUI to render.
    let paste_settle = if prompt.len() > 10_000 {
        Duration::from_millis(1000)
    } else {
        Duration::from_millis(200)
    };
    sleep(paste_settle).await;

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

    /// Resolve the absolute path of the `claude` binary, skipping shell aliases.
    fn resolve_claude_binary() -> Result<String> {
        let output = std::process::Command::new("bash")
            .args([
                "-c",
                "command -p which claude 2>/dev/null || which claude 2>/dev/null",
            ])
            .output()
            .context("resolving claude binary path")?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            bail!(
                "tmux transport: could not locate `claude` binary. \
                 Ensure Claude Code is installed and in PATH."
            );
        }
        Ok(path)
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
        let claude_bin = Self::resolve_claude_binary()?;

        let mut claude_args = vec![claude_bin, "--dangerously-skip-permissions".to_string()];
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

    /// Wait for Claude's TUI to finish initializing by checking tmux pane content.
    async fn wait_for_readiness(session_name: &str) -> Result<()> {
        let deadline = Instant::now() + READINESS_TIMEOUT;
        loop {
            let output = std::process::Command::new("tmux")
                .args(["capture-pane", "-t", session_name, "-p"])
                .output();
            if let Ok(out) = output {
                let text = String::from_utf8_lossy(&out.stdout);
                // Claude's TUI shows the input prompt line with "❯" when ready.
                if text.contains('❯') {
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                bail!(
                    "tmux transport: Claude Code did not become ready within {}s. \
                     Check tmux session '{session_name}' for errors.",
                    READINESS_TIMEOUT.as_secs()
                );
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    /// Full session lifecycle: snapshot → spawn → ready → prompt → discover → watch.
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

        // Snapshot existing JSONL files before spawning so we can diff later.
        let jsonl_dir = project_jsonl_dir(work_dir)?;
        let before_snapshot = snapshot_jsonl_files(&jsonl_dir);
        tracing::debug!(
            jsonl_dir = %jsonl_dir.display(),
            existing_count = before_snapshot.len(),
            "tmux transport: pre-spawn JSONL snapshot"
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

        Self::wait_for_readiness(&session_name).await?;
        tracing::debug!(session = %session_name, "tmux transport: Claude ready");

        deliver_prompt(&session_name, prompt).await?;
        tracing::debug!(
            session = %session_name,
            prompt_len = prompt.len(),
            "tmux transport: prompt delivered"
        );

        // JSONL is created after Claude processes the first prompt.
        let (jsonl_path, provider_session_id) =
            discover_new_jsonl(&jsonl_dir, &before_snapshot).await?;
        tracing::debug!(
            jsonl = %jsonl_path.display(),
            session_id = %provider_session_id,
            "tmux transport: discovered JSONL log"
        );

        // Schema validation: confirm the JSONL format matches expectations.
        if let Err(e) = validate_jsonl_schema(&jsonl_path) {
            tracing::warn!(error = %e, "tmux transport: JSONL schema validation failed");
            bail!(e);
        }

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
                "tmux transport: sandbox configuration is present but cannot be enforced — \
                 tmux sessions spawn outside the sandbox namespace. The sandbox is \
                 silently skipped for this transport."
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
