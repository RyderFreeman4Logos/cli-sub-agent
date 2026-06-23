use anyhow::Error;
use csa_config::ProjectConfig;
use csa_resource::{MemoryAdmissionError, memory_policy};
use csa_session::{
    MetaSessionState, NO_PROVIDER_LAUNCH_SCHEMA_VERSION, NoProviderLaunchDiagnostic,
    NoProviderLaunchMemoryDiagnostic,
};

use crate::resource_admission_soft_limit::MemorySoftLimitAdmissionError;
use crate::run_resource_overrides::RunResourceOverrides;

pub(crate) const HOST_MEMORY_ADMISSION_REASON: &str = "host_memory_admission";

pub(crate) struct NoProviderLaunchContext<'a> {
    pub(crate) session: &'a MetaSessionState,
    pub(crate) tool_name: &'a str,
    pub(crate) task_type: Option<&'a str>,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) resource_overrides: RunResourceOverrides,
}

pub(crate) fn diagnostic_from_error(
    ctx: NoProviderLaunchContext<'_>,
    error: &Error,
) -> Option<NoProviderLaunchDiagnostic> {
    if let Some(soft_limit) = error.downcast_ref::<MemorySoftLimitAdmissionError>() {
        return Some(from_soft_limit_admission(ctx, soft_limit));
    }
    if let Some(host_memory) = error.downcast_ref::<MemoryAdmissionError>() {
        return Some(from_host_memory_admission(ctx, host_memory));
    }
    None
}

fn from_soft_limit_admission(
    ctx: NoProviderLaunchContext<'_>,
    error: &MemorySoftLimitAdmissionError,
) -> NoProviderLaunchDiagnostic {
    base_diagnostic(
        &ctx,
        error.role(),
        MemorySoftLimitAdmissionError::TERMINATION_REASON,
        NoProviderLaunchMemoryDiagnostic {
            effective_memory_max_mb: Some(error.memory_max_mb()),
            soft_limit_percent: Some(error.soft_limit_percent()),
            soft_threshold_mb: Some(error.threshold_mb()),
            required_floor_mb: Some(error.required_threshold_mb()),
            required_memory_max_mb: Some(error.required_memory_max_mb()),
            ..Default::default()
        },
        error.guidance(),
    )
}

fn from_host_memory_admission(
    ctx: NoProviderLaunchContext<'_>,
    error: &MemoryAdmissionError,
) -> NoProviderLaunchDiagnostic {
    let effective_memory_max_mb = error.projected_spawn_mb.or_else(|| {
        ctx.resource_overrides
            .resolve_memory_max_mb(ctx.config, ctx.tool_name)
    });
    let soft_limit_percent = effective_memory_max_mb.map(|_| {
        ctx.config
            .and_then(|cfg| cfg.resources.soft_limit_percent)
            .unwrap_or(memory_policy::DEFAULT_SOFT_LIMIT_PERCENT)
    });
    let required_floor_mb =
        crate::resource_admission_soft_limit::codex_soft_limit_required_floor_mb(
            ctx.task_type,
            ctx.tool_name,
        );
    let required_memory_max_mb =
        required_floor_mb
            .zip(soft_limit_percent)
            .and_then(|(floor, percent)| {
                memory_policy::required_memory_max_for_soft_limit_mb(floor, percent)
            });
    let soft_threshold_mb = effective_memory_max_mb
        .zip(soft_limit_percent)
        .and_then(|(cap, percent)| memory_policy::soft_limit_threshold_mb(cap, percent));

    base_diagnostic(
        &ctx,
        role_from_task_type(ctx.task_type),
        error.kind.denial_class(),
        NoProviderLaunchMemoryDiagnostic {
            effective_memory_max_mb,
            soft_limit_percent,
            soft_threshold_mb,
            required_floor_mb,
            required_memory_max_mb,
            reserve_mb: Some(error.reserve_mb),
            available_memory_mb: Some(error.available_phys_mb),
            required_available_mb: error.required_available_mb,
            projected_spawn_mb: error.projected_spawn_mb,
            active_session_rss_mb: error.active_session_rss_mb,
            active_session_projected_mb: error.active_session_projected_mb,
            active_session_count: error.active_session_count,
            sampled_session_count: error.sampled_session_count,
        },
        host_memory_guidance(ctx.task_type),
    )
}

fn base_diagnostic(
    ctx: &NoProviderLaunchContext<'_>,
    role: &str,
    denial_class: &str,
    memory: NoProviderLaunchMemoryDiagnostic,
    guidance: Vec<String>,
) -> NoProviderLaunchDiagnostic {
    NoProviderLaunchDiagnostic {
        schema_version: NO_PROVIDER_LAUNCH_SCHEMA_VERSION,
        session_id: ctx.session.meta_session_id.clone(),
        timestamp: chrono::Utc::now(),
        tool: ctx.tool_name.to_string(),
        role: role.to_string(),
        session_class: ctx.task_type.map(str::to_string),
        denial_class: denial_class.to_string(),
        no_provider_launch: true,
        provider_side_effects: false,
        head_sha: ctx.session.git_head_at_creation.clone(),
        scope: None,
        range: None,
        memory,
        guidance,
    }
}

pub(crate) fn role_from_task_type(task_type: Option<&str>) -> &'static str {
    match task_type {
        Some("reviewer_sub_session" | "review_fix_finding" | "review") => "reviewer",
        Some("run") | None => "writer",
        Some(_) => "session",
    }
}

fn host_memory_guidance(task_type: Option<&str>) -> Vec<String> {
    match role_from_task_type(task_type) {
        "reviewer" => vec![
            "Host memory admission failed before the reviewer provider launched; this is infrastructure no-verdict, not PASS/FAIL.".to_string(),
            "Retry the same cap with a lower --min-free-memory-mb only when the host reserve is intentionally conservative; otherwise wait/free memory or use another configured reviewer.".to_string(),
            "If no reviewer fallback runs, fail closed and use native/manual review after one bounded retry.".to_string(),
        ],
        "writer" => vec![
            "Host memory admission failed before the writer provider launched; no provider-side worktree mutation occurred.".to_string(),
            "Lower only the reserve when safe, wait/free memory, or reduce the projected spawn cap without dropping below the role soft-limit floor.".to_string(),
            "For dirty-work recovery, avoid repeated CSA retries in the same memory envelope; after one same-class retry, switch to documented recovery/manual fallback.".to_string(),
        ],
        _ => vec![
            "Host memory admission failed before the provider launched; retry after freeing memory or lowering only safe resource overrides.".to_string(),
        ],
    }
}

pub(crate) fn enrich_review_diagnostic(
    diagnostic: &mut NoProviderLaunchDiagnostic,
    head_sha: &str,
    scope: &str,
) {
    if diagnostic.head_sha.as_deref().is_none_or(str::is_empty) && !head_sha.is_empty() {
        diagnostic.head_sha = Some(head_sha.to_string());
    }
    diagnostic.scope = Some(scope.to_string());
    diagnostic.range = scope.strip_prefix("range:").map(str::to_string);
}
