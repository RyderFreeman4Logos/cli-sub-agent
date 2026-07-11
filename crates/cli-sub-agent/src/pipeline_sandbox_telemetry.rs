#[derive(Serialize)]
struct SandboxCapabilityAudit {
    capability: &'static str,
    exposed_paths: Vec<String>,
    reason: &'static str,
    timestamp: String,
}

fn write_user_daemon_ipc_audit_artifact(
    session_dir: &Path,
    plan: &IsolationPlan,
) -> Result<(), String> {
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir).map_err(|err| {
        format!(
            "Failed to create sandbox capability audit output directory '{}': {err}",
            output_dir.display()
        )
    })?;
    let audit = SandboxCapabilityAudit {
        capability: "user-daemon-ipc",
        exposed_paths: user_daemon_ipc_exposed_paths(plan),
        reason: "verification session requested user daemon restart capability",
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let payload = serde_json::to_string_pretty(&audit)
        .map_err(|err| format!("Failed to serialize sandbox capability audit artifact: {err}"))?;
    let audit_path = output_dir.join("sandbox-capability-audit.json");
    fs::write(&audit_path, format!("{payload}\n")).map_err(|err| {
        format!(
            "Failed to write sandbox capability audit artifact '{}': {err}",
            audit_path.display()
        )
    })
}

fn user_daemon_ipc_exposed_paths(plan: &IsolationPlan) -> Vec<String> {
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) else {
        return Vec::new();
    };
    [runtime_dir.join("bus"), runtime_dir.join("systemd/private")]
        .into_iter()
        .filter(|expected| {
            plan.readable_paths.iter().any(|path| path == expected)
                || plan
                    .writable_paths
                    .iter()
                    .any(|path| expected.starts_with(path))
        })
        .map(|path| path.display().to_string())
        .collect()
}

/// Record sandbox telemetry in session state (first turn only).
///
/// If sandbox options are present, detects the active capability and writes a
/// `SandboxInfo` snapshot. If runtime preparation resolves to no sandbox,
/// clears any transient pre-spawn admission marker.
pub(crate) fn record_sandbox_telemetry(
    execute_options: &ExecuteOptions,
    session: &mut MetaSessionState,
) -> bool {
    let Some(sandbox_context) = execute_options.sandbox.as_ref() else {
        return crate::resource_admission::clear_spawn_memory_projection(session);
    };

    let capability = csa_resource::detect_resource_capability();
    let mode = match capability {
        csa_resource::ResourceCapability::CgroupV2 => "cgroup",
        csa_resource::ResourceCapability::Setrlimit => "rlimit",
        csa_resource::ResourceCapability::None => "none",
    };
    let memory: Option<u64> = sandbox_context.isolation_plan.memory_max_mb;

    // Capture filesystem isolation mode from the isolation plan.
    let fs_mode = Some(match sandbox_context.isolation_plan.filesystem {
        csa_resource::FilesystemCapability::Bwrap => "bwrap".to_string(),
        csa_resource::FilesystemCapability::Landlock => "landlock".to_string(),
        csa_resource::FilesystemCapability::None => "none".to_string(),
    });

    let readonly = Some(sandbox_context.isolation_plan.readonly_project_root);

    let sandbox_info = csa_session::SandboxInfo {
        mode: mode.to_string(),
        memory_max_mb: memory,
        filesystem_mode: fs_mode.clone(),
        readonly_project_root: readonly,
    };
    if session.sandbox_info.as_ref() == Some(&sandbox_info) {
        return false;
    }

    session.sandbox_info = Some(sandbox_info);

    info!(
        session = %session.meta_session_id,
        sandbox_mode = mode,
        memory_max_mb = ?memory,
        filesystem_mode = ?fs_mode,
        "Sandbox telemetry recorded in session state"
    );
    true
}

pub(crate) fn filesystem_sandbox_active(sandbox_info: Option<&csa_session::SandboxInfo>) -> bool {
    sandbox_info
        .and_then(|info| info.filesystem_mode.as_deref())
        .is_some_and(|mode| mode != "none")
}

/// Best-effort diagnostic: if tool stderr contains EACCES / "Permission denied"
/// and a filesystem sandbox was active, emit a `tracing::warn!` with hints.
///
/// This is intentionally lenient — false positives (non-sandbox permission errors)
/// are acceptable because we only log, never alter the execution result.
pub(crate) fn check_sandbox_permission_errors(
    stderr: &str,
    sandbox_info: Option<&csa_session::SandboxInfo>,
) {
    let Some(info) = sandbox_info else { return };
    if !filesystem_sandbox_active(Some(info)) {
        return;
    }

    let lower = stderr.to_ascii_lowercase();
    if !lower.contains("permission denied") && !lower.contains("eacces") {
        return;
    }

    let readonly = info.readonly_project_root.unwrap_or(false);
    warn!(
        filesystem_mode = info.filesystem_mode.as_deref().unwrap_or("unknown"),
        readonly_project_root = readonly,
        "Tool stderr contains 'Permission denied' — this may be caused by the \
         filesystem sandbox. Check writable_paths in .csa/config.toml \
         [tools.<name>.filesystem_sandbox] or pass --no-fs-sandbox to disable."
    );
}
