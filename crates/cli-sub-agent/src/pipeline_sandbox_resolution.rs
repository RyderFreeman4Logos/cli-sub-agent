use super::*;

pub(super) fn resolve_sandbox_options_with_capability_source(
    input: SandboxResolveInput<'_>,
    resource_overrides: RunResourceOverrides,
    resource_capability: impl Fn() -> csa_resource::ResourceCapability,
    filesystem_capability: impl Fn() -> csa_resource::FilesystemCapability,
    runtime_session_dir: Option<PathBuf>,
) -> SandboxResolution {
    let SandboxResolveInput {
        config,
        tool_name,
        session_id,
        project_root,
        stream_mode,
        idle_timeout_seconds,
        liveness_dead_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox,
        allow_user_daemon_ipc,
        readonly_project_root,
        extra_writable,
        extra_readable,
        execution_env,
    } = input;
    let has_memory_max_override = resource_overrides.has_memory_max_override();
    let has_explicit_cli_memory_max = resource_overrides.has_explicit_cli_memory_max();

    let default_resources = csa_config::ResourcesConfig::default();
    let stdin_write_timeout_seconds = config
        .map(|cfg| cfg.resources.stdin_write_timeout_seconds)
        .unwrap_or(default_resources.stdin_write_timeout_seconds);
    let acp_init_timeout_seconds = config
        .map(|cfg| cfg.acp.init_timeout_seconds)
        .unwrap_or(csa_config::AcpConfig::default().init_timeout_seconds);
    let acp_crash_max_attempts = config.map_or_else(
        || csa_config::ExecutionConfig::default().resolved_acp_crash_max_attempts(),
        |cfg| cfg.execution.resolved_acp_crash_max_attempts(),
    );
    let termination_grace_period_seconds = config
        .map(|cfg| cfg.resources.termination_grace_period_seconds)
        .unwrap_or(default_resources.termination_grace_period_seconds);
    let mut execute_options = ExecuteOptions::new(stream_mode, idle_timeout_seconds)
        .with_acp_crash_max_attempts(acp_crash_max_attempts)
        .with_liveness_dead_seconds(liveness_dead_seconds)
        .with_stdin_write_timeout_seconds(stdin_write_timeout_seconds)
        .with_acp_init_timeout_seconds(acp_init_timeout_seconds)
        .with_termination_grace_period_seconds(termination_grace_period_seconds)
        .with_initial_response_timeout_seconds(initial_response_timeout_seconds);

    let Some(cfg) = config else {
        // No project config — apply profile-based defaults for heavyweight tools.
        let defaults = csa_config::default_sandbox_for_tool(tool_name);
        execute_options = execute_options.with_setting_sources(defaults.setting_sources);

        if memory_override::default_off_allows_unsandboxed(
            defaults.enforcement,
            has_memory_max_override,
        ) {
            return SandboxResolution::Ok(Box::new(execute_options));
        }

        let Some(memory_max_mb) = resource_overrides.resolve_memory_max_mb(None, tool_name) else {
            return SandboxResolution::Ok(Box::new(execute_options));
        };

        let resource_cap = resource_capability();
        let fs_cap = if no_fs_sandbox {
            csa_resource::FilesystemCapability::None
        } else {
            filesystem_capability()
        };
        if let Some(message) = memory_override::capability_error_if_unenforced(
            tool_name,
            has_explicit_cli_memory_max,
            resource_cap,
        ) {
            return SandboxResolution::RequiredButUnavailable(message);
        }
        if matches!(resource_cap, csa_resource::ResourceCapability::None) {
            warn!(
                tool = tool_name,
                "No sandbox capability available; skipping enforcement for profile defaults"
            );
            return SandboxResolution::Ok(Box::new(execute_options));
        }

        // Build IsolationPlan via builder (BestEffort for profile defaults).
        let tool_state_dirs = csa_config::default_tool_state_dirs();
        let mut builder = IsolationPlanBuilder::new(ResourceEnforcementMode::BestEffort)
            .with_resource_capability(resource_cap)
            .with_filesystem_capability(fs_cap)
            .with_execution_env(execution_env)
            .with_resource_limits(
                Some(memory_max_mb),
                defaults.memory_swap_max_mb,
                None, // pids_max not available from profile defaults
            )
            .with_readonly_project_root(readonly_project_root)
            .with_project_root(project_root);
        if let Some(session_dir) = runtime_session_dir.as_deref() {
            builder = builder.with_tool_defaults_and_state_dirs(
                tool_name,
                project_root,
                session_dir,
                Some(&tool_state_dirs),
            );
        }
        if allow_user_daemon_ipc {
            builder = builder.with_user_daemon_ipc();
        }

        // CSA runtime writable paths.
        if runtime_session_dir.is_some() && !no_fs_sandbox {
            builder = match add_execution_env_writable_paths(builder, execution_env, project_root) {
                Ok(builder) => builder,
                Err(message) => return SandboxResolution::RequiredButUnavailable(message),
            };
            if let Ok(project_state_root) = csa_session::manager::get_session_root(project_root) {
                builder = builder.with_writable_path(project_state_root);
            }
            if let Ok(slots) = csa_config::GlobalConfig::slots_dir() {
                builder = builder.with_writable_path(slots);
            }
            // CLI --extra-writable / --expose-readable (no-config path).
            if !extra_writable.is_empty() {
                let resolved = match writable_sources::resolve_and_prepare_writable_sources(
                    extra_writable,
                    project_root,
                    "--extra-writable",
                ) {
                    Ok(paths) => paths,
                    Err(message) => {
                        return SandboxResolution::RequiredButUnavailable(message);
                    }
                };
                for path in resolved {
                    builder = builder.with_writable_path(path);
                }
            }
            if !extra_readable.is_empty() {
                let resolved = match csa_resource::isolation_plan::validate_readable_paths(
                    extra_readable,
                    project_root,
                ) {
                    Ok(paths) => paths,
                    Err(e) => {
                        return SandboxResolution::RequiredButUnavailable(format!(
                            "--expose-readable validation failed: {e}"
                        ));
                    }
                };
                for path in resolved {
                    builder = builder.with_readable_path(path);
                }
            }
        }

        let plan = match spawn_admission::build_isolation_plan(builder, tool_name) {
            Ok(plan) => plan,
            Err(message) => return SandboxResolution::RequiredButUnavailable(message),
        };
        if let Some(message) =
            memory_override::plan_error_if_unenforced(tool_name, has_explicit_cli_memory_max, &plan)
        {
            return SandboxResolution::RequiredButUnavailable(message);
        }
        if allow_user_daemon_ipc
            && let Some(session_dir) = runtime_session_dir.as_deref()
            && let Err(message) = write_user_daemon_ipc_audit_artifact(session_dir, &plan)
        {
            return SandboxResolution::RequiredButUnavailable(message);
        }

        execute_options = execute_options.with_sandbox(SandboxContext {
            isolation_plan: plan,
            tool_name: tool_name.to_string(),
            session_id: session_id.to_string(),
            best_effort: true, // Profile defaults always use best-effort
        });

        return SandboxResolution::Ok(Box::new(execute_options));
    };

    execute_options = execute_options.with_setting_sources(cfg.tool_setting_sources(tool_name));

    // Use per-tool enforcement mode (profile-aware) instead of global-only.
    let enforcement = match memory_override::resolve_config_enforcement(
        cfg,
        tool_name,
        has_memory_max_override,
        has_explicit_cli_memory_max,
    ) {
        Ok(Some(enforcement)) => enforcement,
        Ok(None) => {
            return SandboxResolution::Ok(Box::new(execute_options));
        }
        Err(message) => return SandboxResolution::RequiredButUnavailable(message),
    };

    let Some(memory_max_mb) = resource_overrides.resolve_memory_max_mb(Some(cfg), tool_name) else {
        if matches!(enforcement, csa_config::EnforcementMode::Required) {
            return SandboxResolution::RequiredButUnavailable(format!(
                "Sandbox enforcement is required for tool '{tool_name}' but no memory_max_mb is configured. \
                 Set --memory-max-mb, resources.memory_max_mb, or tools.{tool_name}.memory_max_mb."
            ));
        }
        info!(
            tool = %tool_name,
            enforcement = ?enforcement,
            "Sandbox enforcement active but no memory_max_mb configured; skipping isolation"
        );
        return SandboxResolution::Ok(Box::new(execute_options));
    };

    // Memory limit exists — detect capabilities and build IsolationPlan.
    let resource_cap = resource_capability();
    if let Some(message) = memory_override::capability_error_if_unenforced(
        tool_name,
        has_explicit_cli_memory_max,
        resource_cap,
    ) {
        return SandboxResolution::RequiredButUnavailable(message);
    }

    // Resolve filesystem enforcement independently from resource enforcement.
    // tool_fs_enforcement_mode already handles the full priority chain:
    //   tool-level > safety-net auto-promote > global [filesystem_sandbox].
    let fs_enforcement = if no_fs_sandbox {
        ResourceEnforcementMode::Off
    } else {
        let effective_mode = cfg
            .tool_fs_enforcement_mode(tool_name)
            .unwrap_or_else(|| "best-effort".to_string());
        match effective_mode.as_str() {
            "off" => ResourceEnforcementMode::Off,
            "required" => ResourceEnforcementMode::Required,
            _ => ResourceEnforcementMode::BestEffort,
        }
    };

    let fs_cap = if matches!(fs_enforcement, ResourceEnforcementMode::Off) {
        csa_resource::FilesystemCapability::None
    } else {
        filesystem_capability()
    };

    // Map config enforcement mode to resource enforcement mode.
    let resource_enforcement = match enforcement {
        csa_config::EnforcementMode::Required => ResourceEnforcementMode::Required,
        csa_config::EnforcementMode::BestEffort => ResourceEnforcementMode::BestEffort,
        csa_config::EnforcementMode::Off => ResourceEnforcementMode::Off,
    };

    match enforcement {
        csa_config::EnforcementMode::Required => {
            if resource_cap == csa_resource::ResourceCapability::None {
                return SandboxResolution::RequiredButUnavailable(
                    "Sandbox required but no capability detected (no cgroup v2 or setrlimit). \
                     Set enforcement_mode = \"off\" or \"best-effort\" to proceed without isolation."
                        .to_string(),
                );
            }
        }
        csa_config::EnforcementMode::BestEffort => {
            if resource_cap == csa_resource::ResourceCapability::None {
                warn!(
                    tool = %tool_name,
                    "Sandbox configured but no capability detected; proceeding without isolation"
                );
            }
        }
        csa_config::EnforcementMode::Off => {} // already filtered above
    }

    let memory_swap_max_mb = cfg.sandbox_memory_swap_max_mb(tool_name);
    let pids_max = cfg.sandbox_pids_max();

    // Per-tool filesystem sandbox: check for REPLACE-semantics writable paths.
    let per_tool_writable = if runtime_session_dir.is_some() && !no_fs_sandbox {
        match writable_sources::resolve_per_tool_writable_sources(cfg, tool_name, project_root) {
            Ok(paths) => paths,
            Err(message) => {
                return SandboxResolution::RequiredButUnavailable(message);
            }
        }
    } else {
        None
    };
    let per_tool_readable = if runtime_session_dir.is_some() && !no_fs_sandbox {
        cfg.sandbox_readable_paths(tool_name)
    } else {
        None
    };

    // When per-tool writable paths are set, project root becomes read-only
    // (the per-tool paths provide fine-grained write access instead).
    let effective_readonly = readonly_project_root || per_tool_writable.is_some();

    let mut builder = IsolationPlanBuilder::new(resource_enforcement)
        .with_filesystem_enforcement(fs_enforcement)
        .with_resource_capability(resource_cap)
        .with_filesystem_capability(fs_cap)
        .with_execution_env(execution_env)
        .with_resource_limits(Some(memory_max_mb), memory_swap_max_mb, pids_max)
        .with_readonly_project_root(effective_readonly)
        .with_project_root(project_root)
        .with_soft_limit_percent(cfg.resources.soft_limit_percent)
        .with_memory_monitor_interval(cfg.resources.memory_monitor_interval_seconds);
    if let Some(session_dir) = runtime_session_dir.as_deref() {
        builder = builder.with_tool_defaults_and_state_dirs(
            tool_name,
            project_root,
            session_dir,
            Some(&cfg.tool_state_dirs),
        );
    }
    if allow_user_daemon_ipc {
        builder = builder.with_user_daemon_ipc();
    }

    // CSA runtime paths must survive per-tool REPLACE semantics so fork-call
    // session creation and slot locks still work.
    if runtime_session_dir.is_some() && !no_fs_sandbox {
        builder = match add_execution_env_writable_paths(builder, execution_env, project_root) {
            Ok(builder) => builder,
            Err(message) => return SandboxResolution::RequiredButUnavailable(message),
        };
        if let Ok(project_state_root) = csa_session::manager::get_session_root(project_root) {
            builder = builder.with_writable_path(project_state_root);
        }
        if let Ok(slots) = csa_config::GlobalConfig::slots_dir() {
            builder = builder.with_writable_path(slots);
        }
    }

    if runtime_session_dir.is_some() && !no_fs_sandbox {
        if let Some(ref paths) = per_tool_writable {
            for path in paths {
                builder = builder.with_writable_path(path.clone());
            }
        } else {
            // No per-tool override — apply global extra_writable paths.
            if !cfg.filesystem_sandbox.extra_writable.is_empty() {
                let resolved = match writable_sources::resolve_config_extra_writable_sources(
                    cfg,
                    project_root,
                ) {
                    Ok(paths) => paths,
                    Err(message) => {
                        return SandboxResolution::RequiredButUnavailable(message);
                    }
                };
                for path in resolved {
                    builder = builder.with_writable_path(path);
                }
            }
        }

        if let Some(ref paths) = per_tool_readable {
            let resolved =
                match csa_resource::isolation_plan::validate_readable_paths(paths, project_root) {
                    Ok(paths) => paths,
                    Err(e) => {
                        return SandboxResolution::RequiredButUnavailable(format!(
                            "Per-tool readable_paths validation failed for '{tool_name}': {e}"
                        ));
                    }
                };
            for path in resolved {
                builder = builder.with_readable_path(path);
            }
        }
    }

    // CLI --extra-writable paths: always appended (APPEND semantics, not REPLACE).
    if runtime_session_dir.is_some() && !no_fs_sandbox && !extra_writable.is_empty() {
        let resolved = match writable_sources::resolve_and_prepare_writable_sources(
            extra_writable,
            project_root,
            "--extra-writable",
        ) {
            Ok(paths) => paths,
            Err(message) => {
                return SandboxResolution::RequiredButUnavailable(message);
            }
        };
        for path in resolved {
            builder = builder.with_writable_path(path);
        }
    }

    // CLI --expose-readable paths: always appended after config resolution.
    if runtime_session_dir.is_some() && !no_fs_sandbox && !extra_readable.is_empty() {
        let resolved = match csa_resource::isolation_plan::validate_readable_paths(
            extra_readable,
            project_root,
        ) {
            Ok(paths) => paths,
            Err(e) => {
                return SandboxResolution::RequiredButUnavailable(format!(
                    "--expose-readable validation failed: {e}"
                ));
            }
        };
        for path in resolved {
            builder = builder.with_readable_path(path);
        }
    }

    let plan = match spawn_admission::build_isolation_plan(builder, tool_name) {
        Ok(plan) => plan,
        Err(message) => return SandboxResolution::RequiredButUnavailable(message),
    };
    if let Some(message) =
        memory_override::plan_error_if_unenforced(tool_name, has_explicit_cli_memory_max, &plan)
    {
        return SandboxResolution::RequiredButUnavailable(message);
    }
    if allow_user_daemon_ipc
        && let Some(session_dir) = runtime_session_dir.as_deref()
        && let Err(message) = write_user_daemon_ipc_audit_artifact(session_dir, &plan)
    {
        return SandboxResolution::RequiredButUnavailable(message);
    }

    info!(
        tool = %tool_name,
        enforcement = ?enforcement,
        resource_cap = %resource_cap,
        filesystem_cap = %fs_cap,
        memory_max_mb,
        "Sandbox isolation plan resolved"
    );

    execute_options = execute_options.with_sandbox(SandboxContext {
        isolation_plan: plan,
        tool_name: tool_name.to_string(),
        session_id: session_id.to_string(),
        best_effort: matches!(enforcement, csa_config::EnforcementMode::BestEffort),
    });

    SandboxResolution::Ok(Box::new(execute_options))
}
