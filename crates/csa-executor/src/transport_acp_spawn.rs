#[derive(Clone)]
struct AcpPromptRunRequest {
    tool_name: String,
    acp_command: String,
    acp_args: Vec<String>,
    prompt: String,
    working_dir: std::path::PathBuf,
    env: HashMap<String, String>,
    system_prompt: Option<String>,
    resume_session_id: Option<String>,
    session_meta: Option<serde_json::Map<String, serde_json::Value>>,
    sandbox_plan: Option<csa_resource::isolation_plan::IsolationPlan>,
    sandbox_tool_name: Option<String>,
    sandbox_session_id: Option<String>,
    sandbox_best_effort: bool,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    acp_init_timeout_seconds: u64,
    termination_grace_period_seconds: u64,
    stream_stdout_to_stderr: bool,
    output_spool: Option<std::path::PathBuf>,
    output_spool_max_bytes: u64,
    output_spool_keep_rotated: bool,
    acp_payload_debug_path: Option<std::path::PathBuf>,
    gemini_classification_env: Option<HashMap<String, String>>,
    gemini_env_allowlist_applied: String,
    memory_max_mb: Option<u64>,
}

#[derive(Debug)]
struct GeminiAcpMcpRetryOutcome<T> {
    value: T,
    warning_summary: Option<String>,
}

impl AcpTransport {
    async fn run_acp_prompt(
        request: AcpPromptRunRequest,
    ) -> Result<csa_acp::transport::AcpOutput> {
        let classify_request = request.clone();
        let output =
            tokio::task::spawn_blocking(move || -> Result<csa_acp::transport::AcpOutput> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| anyhow!("failed to build ACP runtime: {e}"))?;

                if let Some(ref plan) = request.sandbox_plan {
                    let tool_name = request.sandbox_tool_name.as_deref().unwrap_or("");
                    let sess_id = request.sandbox_session_id.as_deref().unwrap_or("");
                    let sr = rt.block_on(run_acp_sandboxed(
                        &request.acp_command,
                        &request.acp_args,
                        &request.working_dir,
                        &request.env,
                        request.system_prompt.as_deref(),
                        request.resume_session_id.as_deref(),
                        request.session_meta.clone(),
                        &request.prompt,
                        std::time::Duration::from_secs(request.idle_timeout_seconds),
                        request
                            .initial_response_timeout_seconds
                            .map(std::time::Duration::from_secs),
                        std::time::Duration::from_secs(request.acp_init_timeout_seconds),
                        std::time::Duration::from_secs(
                            request.termination_grace_period_seconds,
                        ),
                        plan,
                        tool_name,
                        sess_id,
                        request.stream_stdout_to_stderr,
                        request.output_spool.as_deref(),
                        request.output_spool_max_bytes,
                        request.output_spool_keep_rotated,
                    ));
                    match sr {
                        transport_meta::AcpSandboxedResult {
                            result: Ok(mut output),
                            peak_memory_mb,
                            ..
                        } => {
                            output.peak_memory_mb = output.peak_memory_mb.or(peak_memory_mb);
                            Ok(output)
                        }
                        transport_meta::AcpSandboxedResult {
                            result: Err(e),
                            sandbox_spawn_failed: true,
                            ..
                        } if request.sandbox_best_effort => {
                            tracing::warn!(
                                "sandboxed ACP spawn failed in best-effort mode, retrying unsandboxed: {e:#}"
                            );
                            rt.block_on(csa_acp::transport::run_prompt_with_io(
                                &request.acp_command,
                                &request.acp_args,
                                &request.working_dir,
                                &request.env,
                                csa_acp::transport::AcpSessionStart {
                                    system_prompt: request.system_prompt.as_deref(),
                                    resume_session_id: request.resume_session_id.as_deref(),
                                    meta: request.session_meta.clone(),
                                    ..Default::default()
                                },
                                &request.prompt,
                                csa_acp::transport::AcpRunOptions {
                                    idle_timeout: std::time::Duration::from_secs(
                                        request.idle_timeout_seconds,
                                    ),
                                    initial_response_timeout: request
                                        .initial_response_timeout_seconds
                                        .map(std::time::Duration::from_secs),
                                    init_timeout: std::time::Duration::from_secs(
                                        request.acp_init_timeout_seconds,
                                    ),
                                    termination_grace_period: std::time::Duration::from_secs(
                                        request.termination_grace_period_seconds,
                                    ),
                                    io: csa_acp::transport::AcpOutputIoOptions {
                                        stream_stdout_to_stderr: request.stream_stdout_to_stderr,
                                        output_spool: request.output_spool.as_deref(),
                                        spool_max_bytes: request.output_spool_max_bytes,
                                        keep_rotated_spool: request.output_spool_keep_rotated,
                                    },
                                },
                            ))
                            .map_err(|e| anyhow!("ACP transport (unsandboxed fallback) failed: {e}"))
                        }
                        transport_meta::AcpSandboxedResult {
                            result: Err(e),
                            peak_memory_mb,
                            ..
                        } => Err(PeakMemoryContext(peak_memory_mb)
                            .into_anyhow(format!("sandboxed ACP: {e}"))),
                    }
                } else {
                    rt.block_on(csa_acp::transport::run_prompt_with_io(
                        &request.acp_command,
                        &request.acp_args,
                        &request.working_dir,
                        &request.env,
                        csa_acp::transport::AcpSessionStart {
                            system_prompt: request.system_prompt.as_deref(),
                            resume_session_id: request.resume_session_id.as_deref(),
                            meta: request.session_meta.clone(),
                            ..Default::default()
                        },
                        &request.prompt,
                        csa_acp::transport::AcpRunOptions {
                            idle_timeout: std::time::Duration::from_secs(
                                request.idle_timeout_seconds,
                            ),
                            initial_response_timeout: request
                                .initial_response_timeout_seconds
                                .map(std::time::Duration::from_secs),
                            init_timeout: std::time::Duration::from_secs(
                                request.acp_init_timeout_seconds,
                            ),
                            termination_grace_period: std::time::Duration::from_secs(
                                request.termination_grace_period_seconds,
                            ),
                            io: csa_acp::transport::AcpOutputIoOptions {
                                stream_stdout_to_stderr: request.stream_stdout_to_stderr,
                                output_spool: request.output_spool.as_deref(),
                                spool_max_bytes: request.output_spool_max_bytes,
                                keep_rotated_spool: request.output_spool_keep_rotated,
                            },
                        },
                    ))
                    .map_err(|e| anyhow!("ACP transport failed: {e}"))
                }
            })
            .await
            .map_err(classify_join_error)?
            .map_err(|error| {
                if classify_request.tool_name == "gemini-cli" {
                    let error_display = format!("{error:#}");
                    if is_gemini_acp_init_failure(&error_display) {
                        let classification = classify_gemini_acp_init_failure(
                            &error_display,
                            classify_request
                                .gemini_classification_env
                                .as_ref()
                                .expect("gemini classification env"),
                        );
                        tracing::warn!(
                            classified_reason = classification.code,
                            memory_max_mb = classify_request.memory_max_mb,
                            env_allowlist_applied = %classify_request.gemini_env_allowlist_applied,
                            missing_env_vars = %classification.missing_env_vars.join(","),
                            "classified gemini ACP initialization failure"
                        );
                        return format_gemini_acp_init_failure(
                            &classification,
                            error,
                            classify_request.memory_max_mb,
                        );
                    }
                }
                if let Some(path) = classify_request.acp_payload_debug_path.as_deref() {
                    error.context(format!(
                        "ACP payload debug written to {}",
                        path.display()
                    ))
                } else {
                    error
                }
            })?;

        Ok(output)
    }

    fn degraded_mcp_retry_hint(diagnostic: &McpInitDiagnostic, disable_all: bool) -> String {
        if diagnostic.unhealthy_servers.is_empty() {
            format!(
                "Gemini ACP degraded-MCP retry (#840) disabled all configured MCP servers after generic init crash (disable_all={disable_all})"
            )
        } else {
            format!(
                "Gemini ACP degraded-MCP retry (#840) followed unhealthy MCP servers: {}",
                diagnostic.unhealthy_servers.join(", ")
            )
        }
    }

    async fn execute_gemini_acp_with_degraded_mcp_retry<
        T,
        Spawn,
        SpawnFut,
        Diagnose,
        Disable,
        Classify,
    >(
        runtime_home: &Path,
        path_override: Option<std::ffi::OsString>,
        allow_degraded_mcp: bool,
        mut spawn_once: Spawn,
        diagnose: Diagnose,
        disable: Disable,
        classify_init_failure: Classify,
    ) -> Result<GeminiAcpMcpRetryOutcome<T>>
    where
        Spawn: FnMut() -> SpawnFut,
        SpawnFut: std::future::Future<Output = Result<T>>,
        Diagnose: Fn(&Path, Option<&std::ffi::OsStr>) -> McpInitDiagnostic,
        Disable: Fn(&Path, &McpInitDiagnostic, bool) -> Result<()>,
        Classify: Fn(&anyhow::Error) -> Option<GeminiAcpInitFailureClassification>,
    {
        let issue_url = "https://github.com/RyderFreeman4Logos/cli-sub-agent/issues/840";
        let mut warning_summary = None;
        let preflight = diagnose(runtime_home, path_override.as_deref());
        let mut already_degraded = false;

        if allow_degraded_mcp && !preflight.unhealthy_servers.is_empty() {
            disable(runtime_home, &preflight, false)?;
            warning_summary = Some(format_mcp_init_warning_summary(&preflight, false));
            already_degraded = true;
            tracing::warn!(
                issue = issue_url,
                unhealthy_servers = %preflight.unhealthy_servers.join(","),
                "gemini-cli ACP preflight found unhealthy MCP servers; degrading mirrored runtime before spawn"
            );
        }

        match spawn_once().await {
            Ok(value) => Ok(GeminiAcpMcpRetryOutcome {
                value,
                warning_summary,
            }),
            Err(error) if !allow_degraded_mcp || already_degraded => Err(error),
            Err(error) => {
                let Some(classification) = classify_init_failure(&error) else {
                    return Err(error);
                };
                if classification.code != "gemini_acp_init_handshake_timeout" {
                    return Err(error);
                }

                let diagnostic = diagnose(runtime_home, path_override.as_deref());
                let disable_all = diagnostic.unhealthy_servers.is_empty();
                disable(runtime_home, &diagnostic, disable_all)?;
                warning_summary =
                    Some(format_mcp_init_warning_summary(&diagnostic, disable_all));
                tracing::warn!(
                    issue = issue_url,
                    unhealthy_servers = %if diagnostic.unhealthy_servers.is_empty() {
                        "unknown".to_string()
                    } else {
                        diagnostic.unhealthy_servers.join(",")
                    },
                    disable_all,
                    "gemini-cli ACP init crash matched generic bucket; retrying once with degraded MCP"
                );

                match spawn_once().await {
                    Ok(value) => Ok(GeminiAcpMcpRetryOutcome {
                        value,
                        warning_summary,
                    }),
                    Err(retry_error) => Err(retry_error
                        .context(Self::degraded_mcp_retry_hint(&diagnostic, disable_all))),
                }
            }
        }
    }
}
