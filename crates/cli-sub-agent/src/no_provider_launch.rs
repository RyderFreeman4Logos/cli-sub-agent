use std::path::Path;

use anyhow::Error;
use csa_config::ProjectConfig;
use csa_resource::{MemoryAdmissionError, ResourceGuard, ResourceLimits, memory_policy};
use csa_session::{
    MetaSessionState, NO_PROVIDER_LAUNCH_SCHEMA_VERSION, NoProviderLaunchDiagnostic,
    NoProviderLaunchMemoryDiagnostic,
};

use crate::cli::rewrite_cli_command_options;
use crate::resource_admission_soft_limit::MemorySoftLimitAdmissionError;
use crate::run_resource_overrides::RunResourceOverrides;

pub(crate) const HOST_MEMORY_ADMISSION_REASON: &str = "host_memory_admission";
const CLI_MEMORY_MAX_MIN_MB: u64 = 256;
const PREFERRED_RETRY_RESERVE_MB: u64 = 6_000;

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
    let memory = soft_limit_admission_diagnostic_memory(
        Path::new(&ctx.session.project_path),
        &ctx.session.meta_session_id,
        ctx.config,
        ctx.resource_overrides,
        error,
    );
    let mut guidance = error.guidance();
    guidance.extend(soft_limit_admission_guidance(ctx.tool_name, &memory));

    base_diagnostic(
        &ctx,
        error.role(),
        MemorySoftLimitAdmissionError::TERMINATION_REASON,
        memory,
        guidance,
    )
}

fn from_host_memory_admission(
    ctx: NoProviderLaunchContext<'_>,
    error: &MemoryAdmissionError,
) -> NoProviderLaunchDiagnostic {
    let memory = host_memory_diagnostic_memory(
        ctx.task_type,
        ctx.tool_name,
        ctx.config,
        ctx.resource_overrides,
        error,
    );
    let guidance = host_memory_guidance(ctx.task_type, ctx.tool_name, &memory);

    base_diagnostic(
        &ctx,
        role_from_task_type(ctx.task_type),
        error.kind.denial_class(),
        memory,
        guidance,
    )
}

pub(crate) fn host_memory_guidance_from_error(
    task_type: Option<&str>,
    tool_name: &str,
    config: Option<&ProjectConfig>,
    resource_overrides: RunResourceOverrides,
    error: &Error,
) -> Option<Vec<String>> {
    let host_memory = error.downcast_ref::<MemoryAdmissionError>()?;
    let memory = host_memory_diagnostic_memory(
        task_type,
        tool_name,
        config,
        resource_overrides,
        host_memory,
    );
    Some(host_memory_guidance(task_type, tool_name, &memory))
}

pub(crate) fn soft_limit_admission_guidance_from_error_with_argv(
    project_root: &Path,
    current_session_id: &str,
    tool_name: &str,
    config: Option<&ProjectConfig>,
    resource_overrides: RunResourceOverrides,
    error: &Error,
    argv: &[String],
) -> Option<Vec<String>> {
    let soft_limit = error.downcast_ref::<MemorySoftLimitAdmissionError>()?;
    let memory = soft_limit_admission_diagnostic_memory(
        project_root,
        current_session_id,
        config,
        resource_overrides,
        soft_limit,
    );
    let mut guidance = soft_limit.guidance();
    guidance.extend(soft_limit_admission_guidance_with_argv(
        tool_name, &memory, argv,
    ));
    Some(guidance)
}

fn soft_limit_admission_diagnostic_memory(
    project_root: &Path,
    current_session_id: &str,
    config: Option<&ProjectConfig>,
    resource_overrides: RunResourceOverrides,
    error: &MemorySoftLimitAdmissionError,
) -> NoProviderLaunchMemoryDiagnostic {
    let admission = crate::resource_admission::build_spawn_memory_admission(
        project_root,
        current_session_id,
        error.memory_max_mb(),
    );
    let mut resource_guard = ResourceGuard::new(ResourceLimits {
        min_free_memory_mb: resource_overrides.resolve_min_free_memory_mb(config),
    });
    let snapshot = resource_guard.memory_admission_retry_snapshot(admission);
    let retry_lower_bound_mb = Some(host_memory_retry_lower_bound_mb(Some(
        error.required_memory_max_mb(),
    )));
    let retry_feasible = retry_lower_bound_mb.map(|lower_bound_mb| {
        lower_bound_mb <= snapshot.retry_bounds.combined_upper_mb
            || reserve_delta_retry_from_bounds(
                snapshot.available_phys_mb,
                snapshot.reserve_mb,
                snapshot
                    .retry_bounds
                    .active_session_upper_mb
                    .unwrap_or(u64::MAX),
                lower_bound_mb,
            )
            .is_some_and(|retry| lower_bound_mb <= retry.adjusted_upper_mb)
    });

    NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb: Some(error.memory_max_mb()),
        soft_limit_percent: Some(error.soft_limit_percent()),
        soft_threshold_mb: Some(error.threshold_mb()),
        required_floor_mb: Some(error.required_threshold_mb()),
        required_memory_max_mb: Some(error.required_memory_max_mb()),
        reserve_mb: Some(snapshot.reserve_mb),
        available_memory_mb: Some(snapshot.available_phys_mb),
        required_available_mb: retry_lower_bound_mb
            .map(|lower_bound_mb| lower_bound_mb.saturating_add(snapshot.reserve_mb)),
        projected_spawn_mb: Some(snapshot.admission.projected_spawn_mb),
        active_session_rss_mb: Some(snapshot.admission.active_session_rss_mb),
        active_session_projected_mb: Some(snapshot.admission.active_session_projected_mb),
        active_session_count: Some(snapshot.admission.active_session_count),
        sampled_session_count: Some(snapshot.admission.sampled_session_count),
        retry_physical_upper_mb: Some(snapshot.retry_bounds.physical_upper_mb),
        retry_active_session_upper_mb: snapshot.retry_bounds.active_session_upper_mb,
        retry_combined_upper_mb: Some(snapshot.retry_bounds.combined_upper_mb),
        retry_lower_bound_mb,
        retry_feasible,
    }
}

fn host_memory_diagnostic_memory(
    task_type: Option<&str>,
    tool_name: &str,
    config: Option<&ProjectConfig>,
    resource_overrides: RunResourceOverrides,
    error: &MemoryAdmissionError,
) -> NoProviderLaunchMemoryDiagnostic {
    let effective_memory_max_mb = error
        .projected_spawn_mb
        .or_else(|| resource_overrides.resolve_memory_max_mb(config, tool_name));
    let soft_limit_percent = effective_memory_max_mb.map(|_| {
        config
            .and_then(|cfg| cfg.resources.soft_limit_percent)
            .unwrap_or(memory_policy::DEFAULT_SOFT_LIMIT_PERCENT)
    });
    let required_floor_mb =
        crate::resource_admission_soft_limit::codex_soft_limit_required_floor_mb(
            task_type, tool_name,
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
    let retry_lower_bound_mb = Some(host_memory_retry_lower_bound_mb(required_memory_max_mb));
    let retry_feasible = retry_lower_bound_mb.zip(error.retry_combined_upper_mb).map(
        |(lower_bound, upper_bound)| {
            lower_bound <= upper_bound
                || reserve_delta_retry(memory_available(error), lower_bound, error)
                    .is_some_and(|retry| lower_bound <= retry.adjusted_upper_mb)
        },
    );

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
        retry_physical_upper_mb: error.retry_physical_upper_mb,
        retry_active_session_upper_mb: error.retry_active_session_upper_mb,
        retry_combined_upper_mb: error.retry_combined_upper_mb,
        retry_lower_bound_mb,
        retry_feasible,
    }
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

fn suggested_retry_command(
    argv: &[String],
    memory_max_mb: u64,
    min_free_memory_mb: Option<u64>,
) -> String {
    let mut replacements = vec![("--memory-max-mb", memory_max_mb.to_string())];
    if let Some(min_free_memory_mb) = min_free_memory_mb {
        replacements.push(("--min-free-memory-mb", min_free_memory_mb.to_string()));
    }
    rewrite_cli_command_options(argv, &replacements).unwrap_or_else(|| {
        let min_free_memory = min_free_memory_mb
            .map(|value| format!(" --min-free-memory-mb {value}"))
            .unwrap_or_default();
        format!("csa --memory-max-mb {memory_max_mb}{min_free_memory}")
    })
}

fn role_from_task_type(task_type: Option<&str>) -> &'static str {
    match task_type {
        Some("reviewer_sub_session" | "review_fix_finding" | "review") => "reviewer",
        Some("run") | None => "writer",
        Some(_) => "session",
    }
}

fn soft_limit_admission_guidance(
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
) -> Vec<String> {
    let argv: Vec<String> = std::env::args().collect();
    soft_limit_admission_guidance_with_argv(tool_name, memory, &argv)
}

fn soft_limit_admission_guidance_with_argv(
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
    argv: &[String],
) -> Vec<String> {
    vec![
        "CSA: soft-limit admission rejected before provider launch; this is an \
         infrastructure/session-unavailable pre-exec denial and did not launch the provider or \
         mutate the worktree."
            .to_string(),
        host_memory_retry_feasibility(tool_name, memory, argv),
    ]
}

fn host_memory_guidance(
    task_type: Option<&str>,
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
) -> Vec<String> {
    let argv: Vec<String> = std::env::args().collect();
    host_memory_guidance_with_argv(task_type, tool_name, memory, &argv)
}

fn host_memory_guidance_with_argv(
    task_type: Option<&str>,
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
    argv: &[String],
) -> Vec<String> {
    let feasibility = host_memory_retry_feasibility(tool_name, memory, argv);
    match role_from_task_type(task_type) {
        "reviewer" => {
            let mut guidance = vec![
                "Host memory admission failed before the reviewer provider launched; this is infrastructure no-verdict, not PASS/FAIL.".to_string(),
                "Host admission uses physical MemAvailable only; swap/combined memory is diagnostic and is not counted because launching a reviewer into swap can still OOM or terminate before a verdict.".to_string(),
            ];
            guidance.push(feasibility);
            guidance.push(
                "If no reviewer fallback runs, fail closed and use native/manual review after one bounded retry."
                    .to_string(),
            );
            guidance
        }
        "writer" => vec![
            "Host memory admission failed before the writer provider launched; no provider-side worktree mutation occurred.".to_string(),
            "Host admission uses physical MemAvailable only; swap/combined memory is diagnostic and is not counted toward the pre-spawn gate.".to_string(),
            feasibility,
            "For dirty-work recovery, avoid repeated CSA retries in the same memory envelope; after one same-class retry, switch to documented recovery/manual fallback.".to_string(),
        ],
        _ => vec![
            "Host memory admission failed before the provider launched; retry after freeing memory or lowering only safe resource overrides.".to_string(),
            feasibility,
        ],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReserveDeltaRetry {
    reserve_mb: u64,
    adjusted_upper_mb: u64,
}

fn host_memory_retry_lower_bound_mb(required_memory_max_mb: Option<u64>) -> u64 {
    required_memory_max_mb.unwrap_or(CLI_MEMORY_MAX_MIN_MB)
}

fn memory_available(error: &MemoryAdmissionError) -> u64 {
    error.available_phys_mb
}

fn reserve_delta_retry(
    available_mb: u64,
    lower_bound_mb: u64,
    error: &MemoryAdmissionError,
) -> Option<ReserveDeltaRetry> {
    let active_upper_mb = error.retry_active_session_upper_mb.unwrap_or(u64::MAX);
    reserve_delta_retry_from_bounds(
        available_mb,
        error.reserve_mb,
        active_upper_mb,
        lower_bound_mb,
    )
}

fn reserve_delta_retry_from_bounds(
    available_mb: u64,
    current_reserve_mb: u64,
    active_upper_mb: u64,
    lower_bound_mb: u64,
) -> Option<ReserveDeltaRetry> {
    if active_upper_mb < lower_bound_mb {
        return None;
    }
    let max_reserve_mb = available_mb.checked_sub(lower_bound_mb)?;
    if max_reserve_mb == 0 {
        return None;
    }
    let preferred_reserve_mb = current_reserve_mb.clamp(1, PREFERRED_RETRY_RESERVE_MB);
    let reserve_mb = preferred_reserve_mb.min(max_reserve_mb);
    let adjusted_upper_mb = available_mb.saturating_sub(reserve_mb).min(active_upper_mb);
    (lower_bound_mb <= adjusted_upper_mb).then_some(ReserveDeltaRetry {
        reserve_mb,
        adjusted_upper_mb,
    })
}

fn host_memory_retry_feasibility(
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
    argv: &[String],
) -> String {
    let lower_bound_mb = memory
        .retry_lower_bound_mb
        .unwrap_or_else(|| host_memory_retry_lower_bound_mb(memory.required_memory_max_mb));
    let current_upper_mb = memory
        .retry_combined_upper_mb
        .or_else(|| {
            memory
                .available_memory_mb
                .zip(memory.reserve_mb)
                .map(|(available_mb, reserve_mb)| {
                    let physical_upper_mb = available_mb.saturating_sub(reserve_mb);
                    memory
                        .retry_active_session_upper_mb
                        .map_or(physical_upper_mb, |active_upper| {
                            physical_upper_mb.min(active_upper)
                        })
                })
        })
        .unwrap_or(0);
    let physical_upper_mb = memory
        .retry_physical_upper_mb
        .or_else(|| {
            memory
                .available_memory_mb
                .zip(memory.reserve_mb)
                .map(|(available_mb, reserve_mb)| available_mb.saturating_sub(reserve_mb))
        })
        .unwrap_or(current_upper_mb);
    let active_upper_mb = memory.retry_active_session_upper_mb;
    let current_reserve_mb = memory.reserve_mb.unwrap_or(0);
    let current_cap_mb = memory.effective_memory_max_mb.or(memory.projected_spawn_mb);
    let lower_reason = retry_lower_bound_reason(memory);
    let bounds = format_retry_bounds(
        lower_bound_mb,
        current_upper_mb,
        physical_upper_mb,
        active_upper_mb,
        current_cap_mb,
        current_reserve_mb,
        lower_reason,
    );
    if lower_bound_mb <= current_upper_mb {
        let host_required_mb = lower_bound_mb.saturating_add(current_reserve_mb);
        let retry_command = suggested_retry_command(argv, lower_bound_mb, None);
        return format!(
            "Retry feasibility: feasible now. {bounds} Suggested retry command: {retry_command}. \
             For dev2merge/mktd child steps, add the same flag to csa plan run or the workflow's \
             child csa run/review step. Config delta: set tools.{tool_name}.memory_max_mb = \
             {lower_bound_mb} or resources.memory_max_mb = {lower_bound_mb}. \
             host_required={host_required_mb}MB."
        );
    }

    let available_mb = memory.available_memory_mb.unwrap_or(0);
    let active_upper_for_retry = active_upper_mb.unwrap_or(u64::MAX);
    if let Some(retry) = reserve_delta_retry_from_bounds(
        available_mb,
        current_reserve_mb,
        active_upper_for_retry,
        lower_bound_mb,
    ) {
        let host_required_mb = lower_bound_mb.saturating_add(retry.reserve_mb);
        let retry_command = suggested_retry_command(argv, lower_bound_mb, Some(retry.reserve_mb));
        return format!(
            "Retry feasibility: feasible with reserve delta. {bounds} Current reserve has no \
             valid window because lower_bound={lower_bound_mb}MB > current_upper={current_upper_mb}MB; \
             lowering reserve opens retry_window={lower_bound_mb}..={adjusted_upper}MB. \
             Suggested retry command: {retry_command}. For dev2merge/mktd child steps, add the \
             same flags to csa plan run or the workflow's child csa run/review step. Config delta: \
             set tools.{tool_name}.memory_max_mb = {lower_bound_mb} and \
             resources.min_free_memory_mb = {reserve_mb}. \
             host_required={host_required_mb}MB <= physical_available={available_mb}MB.",
            adjusted_upper = retry.adjusted_upper_mb,
            reserve_mb = retry.reserve_mb,
        );
    }

    let blocker = if active_upper_for_retry < lower_bound_mb {
        format!(
            "active-session upper {active_upper_for_retry}MB is below lower_bound={lower_bound_mb}MB; wait for active CSA sessions to finish or use a different configured tool"
        )
    } else if available_mb < lower_bound_mb {
        format!(
            "physical MemAvailable {available_mb}MB is below lower_bound={lower_bound_mb}MB even with reserve=0; free host memory or use a lower-floor tool"
        )
    } else {
        format!(
            "no positive reserve can satisfy lower_bound={lower_bound_mb}MB and upper_bound={current_upper_mb}MB"
        )
    };
    format!(
        "Retry feasibility: infeasible. {bounds} No feasible retry window exists \
         because {blocker}. Do not retry with another memory_max_mb inside this \
         envelope; wait/free memory, reduce active CSA-session pressure, or switch tool/config."
    )
}

fn retry_lower_bound_reason(memory: &NoProviderLaunchMemoryDiagnostic) -> &'static str {
    if memory.required_memory_max_mb.is_some() {
        "role/tool soft-limit floor"
    } else {
        "CLI minimum"
    }
}

fn format_retry_bounds(
    lower_bound_mb: u64,
    current_upper_mb: u64,
    physical_upper_mb: u64,
    active_upper_mb: Option<u64>,
    current_cap_mb: Option<u64>,
    current_reserve_mb: u64,
    lower_reason: &str,
) -> String {
    let active_upper = active_upper_mb
        .map(|upper| format!("{upper}MB"))
        .unwrap_or_else(|| "unknown".to_string());
    let current_cap = current_cap_mb
        .map(|cap| format!("{cap}MB"))
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "lower_bound={lower_bound_mb}MB ({lower_reason}); current_upper={current_upper_mb}MB \
         (physical/reserve upper={physical_upper_mb}MB, active-session upper={active_upper}); \
         current_projected_spawn_cap={current_cap}; current_min_free_memory_mb={current_reserve_mb}."
    )
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

#[cfg(test)]
#[path = "no_provider_launch_tests.rs"]
mod tests;
