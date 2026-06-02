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
//!    with `CSA_SESSION_DIR` and `CSA_RESULT_TOML_PATH_CONTRACT` env vars injected
//! 3. Wait for Claude TUI readiness (poll tmux pane for `❯` prompt)
//! 4. Write prompt to a temp file, then send a short file-read instruction
//!    through tmux
//! 5. Discover new JSONL: diff snapshot to find newly created `.jsonl` file
//! 6. Tail JSONL until `type=system subtype=turn_duration` marker
//! 7. Read output: prefer `output/result.toml` (if child wrote it), fall back to JSONL text
//! 8. Symlink Claude JSONL → `<session_dir>/output/claude-conversation.jsonl` for audit
//! 9. Kill tmux session on Drop (`TmuxCleanupGuard`)
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
    prompt_file_path: PathBuf,
}

impl Drop for TmuxCleanupGuard {
    fn drop(&mut self) {
        remove_prompt_file(&self.prompt_file_path);
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

#[path = "transport_tmux_jsonl.rs"]
mod jsonl;
#[cfg(test)]
use jsonl::parse_jsonl_line;
use jsonl::{validate_jsonl_schema, watch_jsonl_for_turn};

// ── Prompt delivery ───────────────────────────────────────────────────────────

/// Return the temp-file path used to hand a prompt to Claude Code.
fn prompt_file_path_for_session(session_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("csa-prompt-{session_name}.md"))
}

fn write_prompt_file(prompt_file_path: &Path, prompt: &str) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(prompt_file_path)
        .with_context(|| format!("creating tmux prompt file {}", prompt_file_path.display()))?;
    use std::io::Write;
    file.write_all(prompt.as_bytes())
        .with_context(|| format!("writing tmux prompt file {}", prompt_file_path.display()))?;
    Ok(())
}

fn remove_prompt_file(prompt_file_path: &Path) {
    if let Err(e) = fs::remove_file(prompt_file_path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            error = %e,
            path = %prompt_file_path.display(),
            "tmux transport: failed to remove prompt file"
        );
    }
}

fn prompt_file_instruction(prompt_file_path: &Path) -> String {
    format!(
        "Read and execute the prompt in this file: {}\n\
         Treat the file contents as the full user request, and do not summarize the file first.",
        prompt_file_path.display()
    )
}

/// Deliver `prompt` to the tmux session by writing it to `prompt_file_path`,
/// then submitting a short instruction that references the file.
///
/// This avoids tmux input-buffer limits and keeps prompt escaping out of the
/// interactive transport path.
///
/// A delay between paste-buffer and Enter is required because Claude Code's
/// TUI uses bracketed-paste mode: the paste event must finish processing before
/// Enter can submit the message.
async fn deliver_prompt(session_name: &str, prompt: &str, prompt_file_path: &Path) -> Result<()> {
    write_prompt_file(prompt_file_path, prompt)?;
    let instruction = prompt_file_instruction(prompt_file_path);

    // Use a named buffer (session_name) to avoid cross-session races when
    // multiple CSA tmux sessions run concurrently.
    let buffer_name = session_name;

    // load-buffer reads from stdin; pipe only the short file instruction in.
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
            .write_all(instruction.as_bytes())
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
    sleep(Duration::from_millis(200)).await;

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

// ── Result contract ──────────────────────────────────────────────────────────

/// Read the `output/result.toml` contract artifact if the child agent wrote one.
/// Checks both top-level `summary` (canonical CSA `SessionResult` schema) and
/// nested `[result].summary` for robustness. Returns `None` when the file is
/// absent or contains neither variant so callers fall back to JSONL text.
fn try_read_contract_result(session_dir: &Path) -> Option<String> {
    let path = csa_session::contract_result_path(session_dir);
    let contents = fs::read_to_string(&path).ok()?;
    let table: toml::Table = toml::from_str(&contents).ok()?;
    let summary = table
        .get("summary")
        .and_then(|v| v.as_str())
        .or_else(|| {
            table
                .get("result")
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("summary"))
                .and_then(|v| v.as_str())
        })
        .map(String::from)?;
    tracing::debug!(
        path = %path.display(),
        summary_len = summary.len(),
        "tmux transport: read result.toml contract output"
    );
    Some(summary)
}

const JSONL_AUDIT_LINK_NAME: &str = "claude-conversation.jsonl";

/// Create a symlink from `<session_dir>/output/claude-conversation.jsonl` to
/// Claude's JSONL conversation log.  Best-effort: logs a warning on failure.
fn create_jsonl_audit_symlink(session_dir: &Path, jsonl_path: &Path) {
    let output_dir = session_dir.join("output");
    if let Err(e) = fs::create_dir_all(&output_dir) {
        tracing::warn!(error = %e, "tmux transport: failed to create output dir for JSONL symlink");
        return;
    }
    let link = output_dir.join(JSONL_AUDIT_LINK_NAME);
    if link.exists() {
        return;
    }
    #[cfg(unix)]
    if let Err(e) = std::os::unix::fs::symlink(jsonl_path, &link) {
        tracing::warn!(
            error = %e,
            target = %jsonl_path.display(),
            link = %link.display(),
            "tmux transport: failed to create JSONL audit symlink"
        );
    } else {
        tracing::debug!(
            link = %link.display(),
            target = %jsonl_path.display(),
            "tmux transport: created JSONL audit symlink"
        );
    }
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

        // Strip inherited CSA/hook env vars, then re-inject the current
        // session's values via extra_env (populated by execute_session).
        for var in crate::executor::executor_env::STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }
        csa_core::env::scrub_subtree_contract_env_tokio(&mut cmd);

        if let Some(env) = extra_env {
            // execute_session passes a trusted merged child env: generic input
            // was scrubbed before fresh CSA session/pin values were inserted.
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
    ///
    /// When `session` is provided, the transport injects the full CSA session env
    /// (`CSA_SESSION_ID`, `CSA_DEPTH`, `CSA_PROJECT_ROOT`, `CSA_TOOL`,
    /// `CSA_RESULT_TOML_PATH_CONTRACT`, etc.) into the tmux environment, enables
    /// result.toml reading as the preferred output source, and creates a JSONL
    /// symlink in the session output directory for xurl/recall audit access.
    async fn execute_session(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        idle_timeout_seconds: u64,
        session: Option<&MetaSessionState>,
    ) -> Result<TransportResult> {
        let session_name = Self::session_name();
        let prompt_file_path = prompt_file_path_for_session(&session_name);
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

        // Merge caller-provided env with full CSA session env vars, aligned
        // with inject_cli_session_env in transport_cli.rs.
        let (merged_env, session_dir) = {
            let mut env = extra_env.cloned().unwrap_or_default();
            csa_core::env::scrub_subtree_contract_env_map(&mut env);
            let mut dir = None;
            if let Some(session) = session {
                env.insert("CSA_SESSION_ID".into(), session.meta_session_id.clone());
                env.insert(
                    "CSA_DEPTH".into(),
                    (session.genealogy.depth + 1).to_string(),
                );
                env.insert("CSA_PROJECT_ROOT".into(), session.project_path.clone());
                env.insert("CSA_TOOL".into(), "claude-code".into());
                if let Ok(current_tool) = std::env::var("CSA_TOOL") {
                    env.insert("CSA_PARENT_TOOL".into(), current_tool);
                }
                if let Some(parent) = session.genealogy.parent_session_id.as_deref() {
                    env.insert("CSA_PARENT_SESSION".into(), parent.to_string());
                }
                if let Ok(session_dir_path) = csa_session::manager::get_session_dir(
                    Path::new(&session.project_path),
                    &session.meta_session_id,
                ) {
                    env.insert(
                        "CSA_SESSION_DIR".into(),
                        session_dir_path.to_string_lossy().into_owned(),
                    );
                    env.insert(
                        csa_session::RESULT_TOML_PATH_CONTRACT_ENV.into(),
                        csa_session::contract_result_path(&session_dir_path)
                            .to_string_lossy()
                            .into_owned(),
                    );
                    dir = Some(session_dir_path);
                }
                if let Some(parent) = session.genealogy.parent_session_id.as_deref()
                    && let Ok(parent_dir) = csa_session::manager::get_session_dir(
                        Path::new(&session.project_path),
                        parent,
                    )
                {
                    env.insert(
                        "CSA_PARENT_SESSION_DIR".into(),
                        parent_dir.to_string_lossy().into_owned(),
                    );
                }
            }
            // #1741: apply CSA's trusted subtree pin LAST, after every generic
            // merge (which stripped the pin keys) — the only writer of the pin
            // keys in the tmux child env.
            if let Some(pin) = subtree_pin {
                for (key, value) in pin.pin_env_entries() {
                    env.insert(key.to_string(), value);
                }
            }
            (env, dir)
        };

        Self::spawn_tmux(
            &session_name,
            work_dir,
            Some(&merged_env),
            model_override,
            thinking_budget,
        )
        .await?;
        // RAII guard: kills the session when this function exits (normal or panic).
        let _guard = TmuxCleanupGuard {
            session_name: session_name.clone(),
            prompt_file_path: prompt_file_path.clone(),
        };

        Self::wait_for_readiness(&session_name).await?;
        tracing::debug!(session = %session_name, "tmux transport: Claude ready");

        deliver_prompt(&session_name, prompt, &prompt_file_path).await?;
        tracing::debug!(
            session = %session_name,
            prompt_len = prompt.len(),
            prompt_file = %prompt_file_path.display(),
            "tmux transport: prompt delivered"
        );

        // JSONL is created after Claude processes the first prompt.
        let (jsonl_path, provider_session_id) =
            discover_new_jsonl(&jsonl_dir, &before_snapshot).await?;
        remove_prompt_file(&prompt_file_path);
        tracing::debug!(
            jsonl = %jsonl_path.display(),
            session_id = %provider_session_id,
            "tmux transport: discovered JSONL log"
        );

        // Schema validation: confirm the JSONL format matches expectations.
        if let Err(e) = validate_jsonl_schema(&jsonl_path) {
            tracing::warn!(error = %e, "tmux transport: JSONL schema validation failed");
            return Err(e);
        }

        let jsonl_fallback_text = watch_jsonl_for_turn(&jsonl_path, idle_timeout_seconds).await?;

        tracing::debug!(
            session = %session_name,
            jsonl_text_len = jsonl_fallback_text.len(),
            "tmux transport: turn complete"
        );

        // Prefer result.toml written by the child agent (if session_dir is set
        // and the prompt instructed Claude to write there). Fall back to JSONL
        // text extraction when result.toml is absent.
        let output_text = match &session_dir {
            Some(dir) => try_read_contract_result(dir).unwrap_or(jsonl_fallback_text),
            None => jsonl_fallback_text,
        };

        // Symlink Claude's JSONL into the CSA session output dir so xurl/recall
        // can locate the conversation log by CSA session ID.
        if let Some(dir) = &session_dir {
            create_jsonl_audit_symlink(dir, &jsonl_path);
        }

        tracing::debug!(
            session = %session_name,
            output_len = output_text.len(),
            "tmux transport: output resolved"
        );

        Ok(TransportResult {
            execution: ExecutionResult {
                summary: output_text.clone(),
                output: output_text,
                stderr_output: String::new(),
                exit_code: 0,
                peak_memory_mb: None,
                ..Default::default()
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
        self.execute_session(
            prompt,
            &work_dir,
            extra_env,
            options.subtree_pin.as_ref(),
            options.idle_timeout_seconds,
            Some(session),
        )
        .await
    }

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        _stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        _initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        self.execute_session(
            prompt,
            work_dir,
            extra_env,
            subtree_pin,
            idle_timeout_seconds,
            None,
        )
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
