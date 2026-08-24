use super::require_commit;

pub(super) fn apply_uncommitted_changes_to_result(
    result: &mut csa_session::SessionResult,
    changes: csa_session::UncommittedChanges,
    large_diff_warning: Option<csa_session::LargeDiffWarningReport>,
    require_commit_contract_failure: bool,
    recovery: Option<csa_session::RequireCommitRecoveryDiagnostic>,
) {
    apply_uncommitted_changes_to_result_with_preserved_summary(
        result,
        changes,
        large_diff_warning,
        require_commit_contract_failure,
        recovery,
        None,
    );
}

pub(super) fn apply_uncommitted_changes_to_result_with_preserved_summary(
    result: &mut csa_session::SessionResult,
    changes: csa_session::UncommittedChanges,
    large_diff_warning: Option<csa_session::LargeDiffWarningReport>,
    require_commit_contract_failure: bool,
    recovery: Option<csa_session::RequireCommitRecoveryDiagnostic>,
    preserved_summary: Option<&str>,
) {
    result.uncommitted_changes = Some(changes);
    result.large_diff_warning = large_diff_warning;
    result.require_commit_recovery = recovery;
    if require_commit_contract_failure {
        let recovery = result.require_commit_recovery.take().unwrap_or_else(|| {
            require_commit::build_recovery_diagnostic_for_state(
                result,
                result.uncommitted_changes.as_ref(),
                false,
                None,
                None,
                None,
                require_commit::SandboxHookProbeState::Clear,
            )
        });
        apply_require_commit_contract_failure_to_result(result, recovery, preserved_summary);
    }
}

pub(super) fn apply_require_commit_contract_failure_to_result(
    result: &mut csa_session::SessionResult,
    recovery: csa_session::RequireCommitRecoveryDiagnostic,
    preserved_summary: Option<&str>,
) {
    remove_incidental_downgrade_warnings(&mut result.warnings);
    result.exit_code = 1;
    result.status = csa_session::SessionResult::status_from_exit_code(1);
    result.summary = preserved_summary
        .unwrap_or_else(|| require_commit::persisted_contract_failure_reason(&recovery))
        .to_string();
    result.require_commit_recovery = Some(recovery);
}

pub(super) fn typed_sandbox_hook_summary(
    summary: &str,
    sa_mode: bool,
    sandbox_hook_state: require_commit::SandboxHookProbeState<'_>,
) -> Option<String> {
    if !sa_mode
        || !matches!(
            sandbox_hook_state,
            require_commit::SandboxHookProbeState::Blocked
                | require_commit::SandboxHookProbeState::Retryable
        )
    {
        return None;
    }
    summary
        .trim_start()
        .starts_with("RebuildError:")
        .then(|| summary.to_string())
}

pub(super) fn mark_require_commit_contract_failure(
    result: &mut csa_process::ExecutionResult,
    sandbox_hook_state: require_commit::SandboxHookProbeState<'_>,
    preserved_summary: Option<&str>,
) {
    result.mark_gate_failure("writer-uncommitted");
    let reason = require_commit::contract_failure_reason(sandbox_hook_state);
    result.summary = preserved_summary.unwrap_or(reason).to_string();
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result.stderr_output.push_str(reason);
    result.stderr_output.push('\n');
}

fn remove_incidental_downgrade_warnings(warnings: &mut Vec<String>) {
    warnings.retain(|warning| !is_incidental_downgrade_warning(warning));
}

fn is_incidental_downgrade_warning(warning: &str) -> bool {
    warning.contains("incidental nonzero exit") && warning.contains("treated as success")
}
