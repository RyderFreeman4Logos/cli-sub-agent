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
    // Only execution commands need upgraded weave patterns.
    // All management/read-only commands stay available even when weave is unhealthy.
    match command {
        Commands::Run { .. } | Commands::Hunt(_) | Commands::Arch(_) | Commands::Triage(_) => true,
        Commands::Review(args) => !args.check_verdict,
        Commands::Debate(_) | Commands::Batch { .. } | Commands::Plan { .. } => true,
        Commands::ClaudeSubAgent(_) | Commands::McpServer => true,
        _ => false,
    }
}

pub(crate) fn link_bug_class_pipeline() {
    let _ = crate::bug_class::BugClassCandidate::aggregate_from_review_artifacts(&[]);
    crate::bug_class::link_bug_class_pipeline_symbols();
}
