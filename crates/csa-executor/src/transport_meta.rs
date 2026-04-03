//! ACP session meta construction, environment building, and sandboxed execution.
//!
//! Extracted from `transport.rs` to keep module sizes manageable.
//! This module handles meta/env construction and the sandboxed ACP run helper,
//! while `transport.rs` retains trait definitions and Transport dispatch.

use std::collections::HashMap;
use std::path::Path;

use csa_acp::SessionConfig;
use csa_resource::isolation_plan::IsolationPlan;
use csa_session::state::MetaSessionState;

use super::AcpTransport;

const SUMMARY_MAX_CHARS: usize = 200;

impl AcpTransport {
    pub(crate) fn build_system_prompt(session_config: Option<&SessionConfig>) -> Option<String> {
        let config = session_config?;
        let mut sections = Vec::new();

        if !config.no_load.is_empty() {
            sections.push(format!("No-load skills: {}", config.no_load.join(", ")));
        }
        if !config.extra_load.is_empty() {
            sections.push(format!(
                "Extra-load skills: {}",
                config.extra_load.join(", ")
            ));
        }
        if let Some(tier) = &config.tier {
            sections.push(format!("Tier: {tier}"));
        }
        if !config.models.is_empty() {
            sections.push(format!("Model candidates: {}", config.models.join(", ")));
        }
        if !config.mcp_servers.is_empty() {
            let servers = config
                .mcp_servers
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            sections.push(format!("MCP servers: {servers}"));
        }

        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n"))
        }
    }

    /// Build ACP session meta from setting_sources and MCP config.
    ///
    /// - setting sources: `{"claudeCode":{"options":{"settingSources": [...]}}}`
    /// - MCP servers: `{"claudeCode":{"options":{"mcpServers": {...}}}}`
    /// - when proxy socket exists, `mcpServers` contains a single `csa-mcp-hub` entry.
    pub(crate) fn build_session_meta(
        setting_sources: Option<&[String]>,
        session_config: Option<&SessionConfig>,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let mut options = serde_json::Map::new();
        if let Some(sources) = setting_sources {
            options.insert(
                "settingSources".to_string(),
                serde_json::Value::Array(
                    sources
                        .iter()
                        .map(|source| serde_json::Value::String(source.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(cfg) = session_config {
            let mcp_servers = csa_acp::mcp_proxy_client::resolve_mcp_meta_servers(cfg);
            let non_empty_servers = mcp_servers
                .as_object()
                .map(|servers| !servers.is_empty())
                .unwrap_or(false);
            if non_empty_servers {
                options.insert("mcpServers".to_string(), mcp_servers);
            }
        }
        if options.is_empty() {
            return None;
        }

        let mut claude_code = serde_json::Map::new();
        claude_code.insert("options".to_string(), serde_json::Value::Object(options));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "claudeCode".to_string(),
            serde_json::Value::Object(claude_code),
        );
        Some(meta)
    }

    pub(crate) fn build_env(
        &self,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
    ) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert(
            "CSA_SESSION_ID".to_string(),
            session.meta_session_id.clone(),
        );
        env.insert(
            "CSA_DEPTH".to_string(),
            (session.genealogy.depth + 1).to_string(),
        );
        env.insert("CSA_PROJECT_ROOT".to_string(), session.project_path.clone());
        // CSA_SESSION_DIR: absolute path to the session state directory
        if let Ok(dir) = csa_session::manager::get_session_dir(
            Path::new(&session.project_path),
            &session.meta_session_id,
        ) {
            env.insert(
                "CSA_SESSION_DIR".to_string(),
                dir.to_string_lossy().into_owned(),
            );
        } else {
            tracing::warn!("failed to compute CSA_SESSION_DIR for ACP env");
        }

        env.insert("CSA_TOOL".to_string(), self.tool_name.clone());
        // Mark this process as a CSA subprocess so child tools can detect
        // recursion risk (e.g. claude-code reading CLAUDE.md rules that say
        // "use csa review"). See GitHub issue #272.
        env.insert("CSA_IS_SUBPROCESS".to_string(), "1".to_string());
        if let Ok(parent_tool) = std::env::var("CSA_TOOL") {
            env.insert("CSA_PARENT_TOOL".to_string(), parent_tool);
        }
        if let Some(parent_session) = &session.genealogy.parent_session_id {
            env.insert("CSA_PARENT_SESSION".to_string(), parent_session.clone());
        }

        if let Some(extra) = extra_env {
            env.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        }

        // Inject merge guard: prepend a `gh` wrapper to PATH that blocks
        // `gh pr merge` unless pr-bot has completed.  This is deterministic
        // environment-level enforcement — the tool subprocess cannot bypass it.
        //
        // Only applied to ACP tools (claude-code, codex) which have autonomous
        // bash execution.  Legacy tools (gemini-cli, opencode) are text-mode
        // and cannot independently call `gh pr merge`.
        csa_hooks::merge_guard::inject_merge_guard_env(&mut env);

        env
    }
}

/// Run an ACP prompt with sandbox isolation.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_acp_sandboxed(
    command: &str,
    args: &[String],
    working_dir: &Path,
    env: &HashMap<String, String>,
    system_prompt: Option<&str>,
    resume_session_id: Option<&str>,
    meta: Option<serde_json::Map<String, serde_json::Value>>,
    prompt: &str,
    idle_timeout: std::time::Duration,
    initial_response_timeout: Option<std::time::Duration>,
    init_timeout: std::time::Duration,
    termination_grace_period: std::time::Duration,
    isolation_plan: &IsolationPlan,
    tool_name: &str,
    session_id: &str,
    stream_stdout_to_stderr: bool,
    output_spool: Option<&Path>,
    output_spool_max_bytes: u64,
    output_spool_keep_rotated: bool,
) -> csa_acp::AcpResult<csa_acp::transport::AcpOutput> {
    use csa_acp::AcpConnection;
    use csa_acp::connection::{AcpConnectionOptions, AcpSandboxRequest, AcpSpawnRequest};

    let (connection, sandbox_handle) = AcpConnection::spawn_sandboxed(
        AcpSpawnRequest {
            command,
            args,
            working_dir,
            env,
            options: AcpConnectionOptions {
                init_timeout,
                termination_grace_period,
            },
        },
        Some(AcpSandboxRequest {
            isolation_plan,
            tool_name,
            session_id,
        }),
    )
    .await?;

    connection.initialize().await?;

    let acp_session_id = if let Some(resume_id) = resume_session_id {
        tracing::debug!(
            resume_session_id = resume_id,
            "loading ACP session (sandboxed)"
        );
        match connection.load_session(resume_id, Some(working_dir)).await {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(
                    resume_session_id = resume_id,
                    error = %error,
                    "Failed to resume sandboxed ACP session, creating new session"
                );
                connection
                    .new_session(system_prompt, Some(working_dir), meta.clone())
                    .await?
            }
        }
    } else {
        connection
            .new_session(system_prompt, Some(working_dir), meta.clone())
            .await?
    };

    let result = connection
        .prompt_with_io(
            &acp_session_id,
            prompt,
            idle_timeout,
            initial_response_timeout,
            csa_acp::connection::PromptIoOptions {
                stream_stdout_to_stderr,
                output_spool,
                spool_max_bytes: output_spool_max_bytes,
                keep_rotated_spool: output_spool_keep_rotated,
            },
        )
        .await;

    // Check for OOM BEFORE the sandbox handle is dropped (which stops the
    // cgroup scope).  Note: `run_acp_sandboxed` is called inside
    // `spawn_blocking`, so synchronous systemctl queries are acceptable here.
    let oom_diagnosis = sandbox_handle.oom_diagnosis();
    if let Some(ref hint) = oom_diagnosis {
        tracing::error!(tool = tool_name, "{hint}");
    }

    let result = result.map_err(|e| {
        if let Some(hint) = &oom_diagnosis {
            // Construct a typed ProcessExited error so callers retain
            // programmatic access to exit code and signal fields.
            let mut stderr = connection.stderr();
            if !stderr.is_empty() && !stderr.ends_with('\n') {
                stderr.push('\n');
            }
            stderr.push_str(&format!("OOM detected: {hint}\n"));
            stderr.push_str(&format!("original error: {e}\n"));
            csa_acp::AcpError::ProcessExited {
                code: 137,
                signal: Some(9),
                stderr,
            }
        } else {
            e
        }
    })?;

    let mut exit_code = connection.exit_code().await?.unwrap_or(0);
    let mut stderr = connection.stderr();
    if result.timed_out {
        exit_code = 137;
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        let is_initial = result.exit_reason.as_deref() == Some("initial_response_timeout");
        let timeout_secs = if is_initial {
            initial_response_timeout.unwrap_or(idle_timeout).as_secs()
        } else {
            idle_timeout.as_secs()
        };
        let label = if is_initial {
            "initial response timeout"
        } else {
            "idle timeout"
        };
        stderr.push_str(&format!(
            "{label}: no ACP events/stderr for {timeout_secs}s; process killed",
        ));
        stderr.push('\n');
    }

    // sandbox_handle dropped here, cleaning up cgroup scope if applicable.

    Ok(csa_acp::transport::AcpOutput {
        output: result.output,
        stderr,
        events: result.events,
        session_id: acp_session_id,
        exit_code,
        metadata: result.metadata,
    })
}

pub(super) fn build_summary(stdout: &str, stderr: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        return truncate_line(last_non_empty_line(stdout), SUMMARY_MAX_CHARS);
    }

    let stdout_line = last_non_empty_line(stdout);
    if !stdout_line.is_empty() {
        return truncate_line(stdout_line, SUMMARY_MAX_CHARS);
    }

    let stderr_line = last_non_empty_line(stderr);
    if !stderr_line.is_empty() {
        return truncate_line(stderr_line, SUMMARY_MAX_CHARS);
    }

    format!("exit code {exit_code}")
}

fn last_non_empty_line(output: &str) -> &str {
    output
        .lines()
        .rev()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with("<!-- CSA:SECTION:")
        })
        .unwrap_or_default()
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    line.chars().take(max_chars).collect()
}
