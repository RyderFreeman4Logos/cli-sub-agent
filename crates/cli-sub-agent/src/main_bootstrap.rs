use crate::cli::Commands;

/// Resolve the effective minimum timeout from project and global configs.
///
/// Priority: project `[execution].min_timeout_seconds` > global > compile-time default.
/// Config loading errors are silently ignored (fall back to compile-time default).
pub(crate) fn resolve_effective_min_timeout() -> u64 {
    let compile_default = csa_config::ExecutionConfig::default_min_timeout();

    // Try to load project config (merged with user-level).
    // This is the same merged config that pipeline uses, so project overrides global
    // via the standard TOML deep-merge path.
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(Some(config)) = csa_config::ProjectConfig::load(&cwd)
        && !config.execution.is_default()
    {
        return config.execution.min_timeout_seconds;
    }

    // Fall back to global config.
    if let Ok(global) = csa_config::GlobalConfig::load()
        && !global.execution.is_default()
    {
        return global.execution.min_timeout_seconds;
    }

    compile_default
}

pub(crate) fn should_attempt_auto_weave_upgrade(command: &Commands) -> bool {
    // Only write-capable execution commands need upgraded weave patterns.
    // Management/read-only commands stay available even when weave is unhealthy.
    match command {
        Commands::Run { .. }
        | Commands::Hunt(_)
        | Commands::Arch(_)
        | Commands::Triage(_)
        | Commands::Mktsk(_) => true,
        // Review is a gate: stale weave.lock should warn, not rewrite the repo
        // before verdict artifacts are produced.
        Commands::Review(_) => false,
        Commands::Debate(_) | Commands::Batch { .. } | Commands::Plan { .. } => true,
        Commands::ClaudeSubAgent(_) | Commands::McpServer => true,
        _ => false,
    }
}

pub(crate) async fn maybe_auto_weave_upgrade(command: &Commands) {
    let has_weave_lock = std::env::current_dir()
        .map(|cwd| cwd.join("weave.lock").exists())
        .unwrap_or(false);

    let auto_upgrade = has_weave_lock
        && should_attempt_auto_weave_upgrade(command)
        && std::env::current_dir()
            .ok()
            .and_then(|cwd| csa_config::ProjectConfig::load(&cwd).ok().flatten())
            .map(|cfg| cfg.execution.auto_weave_upgrade)
            .unwrap_or_else(|| {
                csa_config::GlobalConfig::load()
                    .map(|g| g.execution.auto_weave_upgrade)
                    .unwrap_or(false)
            });

    if auto_upgrade {
        let mut success = false;
        let mut delay = std::time::Duration::from_secs(1);

        for attempt in 0..3 {
            let result = tokio::process::Command::new("weave")
                .arg("upgrade")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            let ok = result.as_ref().map(|s| s.success()).unwrap_or(false);
            if ok {
                success = true;
                break;
            }
            if attempt < 2 {
                tracing::debug!("weave upgrade attempt {attempt} failed, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }

        if !success {
            tracing::debug!(
                "auto weave upgrade failed after 3 attempts (non-fatal). \
                 Disable with [execution] auto_weave_upgrade = false"
            );
        }
    }
}

/// Check the local weave.lock version without preventing startup on failure.
pub(crate) fn check_weave_lock_version_alignment() {
    if let Ok(cwd) = std::env::current_dir() {
        let registry = csa_config::default_registry();
        match csa_config::check_version(
            &cwd,
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_VERSION"),
            &registry,
        ) {
            Ok(result) => {
                if let Some(warning) = csa_config::weave_lock::format_version_check_warning(&result)
                {
                    eprintln!("{warning}");
                }
            }
            Err(e) => {
                tracing::debug!("weave.lock version check failed: {e:#}");
            }
        }
    }
}

/// Migrate legacy XDG paths opportunistically, preserving the CLI's manual fallback.
pub(crate) fn migrate_legacy_xdg_paths_if_needed() {
    let legacy_xdg_paths = csa_config::paths::legacy_paths_requiring_migration();
    if !legacy_xdg_paths.is_empty() {
        match csa_config::migrate::run_xdg_migration() {
            Ok(()) => {
                tracing::debug!(
                    "auto-migrated {} legacy XDG path(s)",
                    legacy_xdg_paths.len()
                );
            }
            Err(e) => {
                eprintln!(
                    "WARNING: failed to auto-migrate legacy XDG paths: {e:#}. Run `csa migrate` manually."
                );
            }
        }
    }
}

pub(crate) fn link_bug_class_pipeline() {
    let _ = crate::bug_class::BugClassCandidate::aggregate_from_review_artifacts(&[]);
    crate::bug_class::link_bug_class_pipeline_symbols();
}
