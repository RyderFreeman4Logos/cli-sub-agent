use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use csa_resource::isolation_plan::IsolationPlan;

use super::transport_meta::start_memory_monitor;

const SUMMARY_MAX_CHARS: usize = 200;

/// Result of [`run_acp_sandboxed`] that preserves peak memory even on failure.
pub(super) struct AcpSandboxedResult {
    pub result: csa_acp::AcpResult<csa_acp::transport::AcpOutput>,
    /// Peak memory from cgroup `memory.peak`, available even when the ACP
    /// session fails (OOM, timeout, init error).
    pub peak_memory_mb: Option<u64>,
    /// True only when `spawn_sandboxed()` itself failed (the process never
    /// started).  False when the sandboxed process started but then failed
    /// during execution (OOM, timeout, init error, prompt failure).
    /// Callers should only fall back to unsandboxed execution when this is true.
    pub sandbox_spawn_failed: bool,
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
    idle_timeout: Duration,
    initial_response_timeout: Option<Duration>,
    init_timeout: Duration,
    termination_grace_period: Duration,
    isolation_plan: &IsolationPlan,
    tool_name: &str,
    session_id: &str,
    stream_stdout_to_stderr: bool,
    output_spool: Option<&Path>,
    output_spool_max_bytes: u64,
    output_spool_keep_rotated: bool,
    tool_output_compaction: Option<csa_acp::ToolOutputCompactionConfig>,
    cancellation: Option<csa_process::ExecutionCancellation>,
) -> AcpSandboxedResult {
    use csa_acp::AcpConnection;
    use csa_acp::connection::{AcpConnectionOptions, AcpSandboxRequest, AcpSpawnRequest};

    if cancellation
        .as_ref()
        .is_some_and(|cancellation| cancellation.is_cancelled())
    {
        return AcpSandboxedResult {
            result: Err(csa_acp::AcpError::Cancelled),
            peak_memory_mb: None,
            sandbox_spawn_failed: false,
        };
    }

    let diagnostic_path =
        super::transport_meta::memory_soft_limit_diagnostic_path(working_dir, session_id);
    if let Some(path) = diagnostic_path.as_deref() {
        csa_resource::memory_monitor::clear_soft_limit_diagnostic(path);
    }

    let (connection, sandbox_handle) = match AcpConnection::spawn_sandboxed(
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
            env_overrides: None,
        }),
    )
    .await
    {
        Ok(pair) => pair,
        Err(e) => {
            // Spawn failed before we have a sandbox handle - no peak memory.
            return AcpSandboxedResult {
                result: Err(e),
                peak_memory_mb: None,
                sandbox_spawn_failed: true,
            };
        }
    };

    // Start memory monitor immediately after spawn, before initialize()/session
    // setup, so cold-start memory usage is also tracked.
    let memory_monitor = sandbox_handle
        .scope_name()
        .zip(connection.child_pid())
        .and_then(|(scope, pid)| {
            start_memory_monitor(
                scope,
                pid,
                isolation_plan,
                termination_grace_period,
                diagnostic_path.clone(),
            )
        });

    // Inner block: all fallible operations after spawn. peak_memory_mb is
    // captured regardless of success or failure.
    let inner = run_acp_sandboxed_inner(
        &connection,
        memory_monitor,
        system_prompt,
        resume_session_id,
        meta,
        prompt,
        idle_timeout,
        initial_response_timeout,
        stream_stdout_to_stderr,
        output_spool,
        output_spool_max_bytes,
        output_spool_keep_rotated,
        tool_output_compaction,
        working_dir,
    );
    let inner_result = if let Some(cancellation) = cancellation.as_ref() {
        tokio::select! {
            result = inner => result,
            _ = cancellation.cancelled() => {
                match connection.kill().await {
                    Ok(()) => Err(csa_acp::AcpError::Cancelled),
                    Err(error) => Err(error),
                }
            }
        }
    } else {
        inner.await
    };

    let exit_signal = match &inner_result {
        Err(csa_acp::AcpError::ProcessExited { signal, .. }) => *signal,
        _ => None,
    };

    // Capture peak memory and check for OOM BEFORE the sandbox handle is
    // dropped (which stops the cgroup scope). Note: `run_acp_sandboxed` is
    // called inside `spawn_blocking`, so synchronous systemctl queries are
    // acceptable here.
    let peak_memory_mb = sandbox_handle.memory_peak_mb();
    let oom_diagnosis = sandbox_handle.oom_diagnosis_with_signal(exit_signal);
    if let Some(ref hint) = oom_diagnosis {
        tracing::error!(tool = tool_name, "{hint}");
    }
    if let Some(peak) = peak_memory_mb {
        tracing::info!(
            tool = tool_name,
            peak_memory_mb = peak,
            "cgroup peak memory recorded"
        );
    }

    // Enrich error with OOM diagnosis if applicable.
    let mut result = async {
        match inner_result {
            Ok((prompt_result, acp_session_id)) => {
                let mut exit_code = connection.exit_code().await?.unwrap_or(0);
                let mut stderr = connection.stderr();
                if prompt_result.timed_out {
                    exit_code = 137;
                    if !stderr.is_empty() && !stderr.ends_with('\n') {
                        stderr.push('\n');
                    }
                    let is_initial =
                        prompt_result.exit_reason.as_deref() == Some("initial_response_timeout");
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

                Ok(csa_acp::transport::AcpOutput {
                    output: prompt_result.output,
                    stderr,
                    events: prompt_result.events,
                    session_id: acp_session_id,
                    exit_code,
                    exit_reason: prompt_result.exit_reason,
                    metadata: prompt_result.metadata,
                    peak_memory_mb,
                })
            }
            Err(e) => {
                if let Some(hint) = &oom_diagnosis {
                    // Construct a typed ProcessExited error so callers retain
                    // programmatic access to exit code and signal fields.
                    let mut stderr = connection.stderr();
                    if !stderr.is_empty() && !stderr.ends_with('\n') {
                        stderr.push('\n');
                    }
                    stderr.push_str(&format!("OOM detected: {hint}\n"));
                    stderr.push_str(&format!("original error: {e}\n"));
                    Err(csa_acp::AcpError::ProcessExited {
                        code: 137,
                        signal: Some(9),
                        stderr,
                    })
                } else {
                    Err(e)
                }
            }
        }
    }
    .await;

    if let Err(cleanup_error) = connection.kill().await {
        result = Err(cleanup_error);
    }

    // sandbox_handle dropped here, cleaning up cgroup scope if applicable.
    AcpSandboxedResult {
        result,
        peak_memory_mb,
        sandbox_spawn_failed: false,
    }
}

/// Inner helper: session setup + prompt execution. Returns the prompt result
/// and session ID so the caller can capture peak memory regardless of outcome.
#[allow(clippy::too_many_arguments)]
async fn run_acp_sandboxed_inner(
    connection: &csa_acp::AcpConnection,
    memory_monitor: Option<csa_resource::memory_monitor::MemoryMonitorHandle>,
    system_prompt: Option<&str>,
    resume_session_id: Option<&str>,
    meta: Option<serde_json::Map<String, serde_json::Value>>,
    prompt: &str,
    idle_timeout: Duration,
    initial_response_timeout: Option<Duration>,
    stream_stdout_to_stderr: bool,
    output_spool: Option<&Path>,
    output_spool_max_bytes: u64,
    output_spool_keep_rotated: bool,
    tool_output_compaction: Option<csa_acp::ToolOutputCompactionConfig>,
    working_dir: &Path,
) -> csa_acp::AcpResult<(csa_acp::connection::PromptResult, String)> {
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
                tool_output_compaction,
            },
        )
        .await;

    // Stop memory monitor before capturing peak memory (done by caller).
    if let Some(monitor) = memory_monitor {
        monitor.stop().await;
    }

    result.map(|r| (r, acp_session_id))
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn acp_fixture(pid_file: &Path, prompt_error: bool) -> String {
        let prompt_response = if prompt_error {
            r#"printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"prompt failed"}}\n' "$id""#
        } else {
            r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id""#
        };
        r#"
trap '' TERM
sh -c 'while :; do sleep 1; done' &
printf '%s\n' "$!" > "__PID__"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[ ]*\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
      ;;
    *'"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"fixture-session"}}\n' "$id"
      ;;
    *'"session/prompt"'*)
      __PROMPT_RESPONSE__
      ;;
  esac
done
"#
        .replace("__PID__", &pid_file.display().to_string())
        .replace("__PROMPT_RESPONSE__", prompt_response)
    }

    async fn assert_terminated(pid: i32) {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
            "sandboxed ACP descendant must be terminated before return; stat={stat}"
        );
    }

    #[tokio::test]
    async fn non_cgroup_sandbox_outcomes_terminate_term_resistant_descendants() {
        let plan = csa_resource::IsolationPlanBuilder::new(csa_resource::EnforcementMode::Off)
            .build()
            .expect("non-cgroup isolation plan");
        assert_eq!(plan.resource, csa_resource::ResourceCapability::None);

        for (name, prompt_error) in [("success", false), ("error", true)] {
            let temp = tempfile::tempdir().expect("tempdir");
            let pid_file = temp.path().join(format!("{name}.pid"));
            let args = vec!["-c".to_string(), acp_fixture(&pid_file, prompt_error)];
            let sandboxed = tokio::time::timeout(
                Duration::from_secs(5),
                run_acp_sandboxed(
                    "sh",
                    &args,
                    temp.path(),
                    &HashMap::new(),
                    None,
                    None,
                    None,
                    "prompt",
                    Duration::from_secs(1),
                    None,
                    Duration::from_secs(1),
                    Duration::from_millis(20),
                    &plan,
                    "fixture",
                    "session",
                    false,
                    None,
                    1024,
                    false,
                    None,
                    None,
                ),
            )
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "sandboxed ACP outcome must return bounded: name={name} pid={:?}",
                    std::fs::read_to_string(&pid_file).ok()
                )
            });

            assert!(!sandboxed.sandbox_spawn_failed, "{name}");
            if prompt_error {
                assert!(
                    matches!(&sandboxed.result, Err(csa_acp::AcpError::PromptFailed(_))),
                    "{name}: result={:?}",
                    sandboxed.result
                );
            } else {
                let output = sandboxed.result.expect("successful sandboxed ACP prompt");
                assert_eq!(output.exit_reason.as_deref(), Some("end_turn"));
            }
            let pid = std::fs::read_to_string(&pid_file)
                .expect("descendant publishes pid after TERM trap")
                .trim()
                .parse()
                .expect("numeric descendant pid");
            assert_terminated(pid).await;
        }
    }
}
