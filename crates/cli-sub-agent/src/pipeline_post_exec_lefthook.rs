use csa_session::SessionResult;

pub(super) fn maybe_record_core_hookspath_conflict(
    result: &mut csa_process::ExecutionResult,
    session_result: &mut SessionResult,
) {
    let Some(hint) = crate::error_hints::lefthook_core_hookspath_conflict_hint(
        &result.stderr_output,
        &result.output,
    ) else {
        return;
    };

    append_unique_warning(&mut result.warnings, hint);
    append_unique_warning(&mut session_result.warnings, hint);
    if !result.stderr_output.contains(hint) {
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(hint);
        result.stderr_output.push('\n');
    }
}

fn append_unique_warning(warnings: &mut Vec<String>, warning: &str) {
    if !warnings.iter().any(|existing| existing == warning) {
        warnings.push(warning.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_result() -> SessionResult {
        let now = chrono::Utc::now();
        SessionResult {
            post_exec_gate: None,
            status: "failure".to_string(),
            exit_code: 1,
            summary: "exit code 1".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn records_core_hookspath_conflict_hint_in_session_result_warnings() {
        let mut execution = csa_process::ExecutionResult {
            stderr_output: "Error: core.hooksPath is set locally to '/x/.git/hooks'\nhint: Unset it:\nhint:   git config --unset-all --local core.hooksPath\ngit commit failed\n".to_string(),
            exit_code: 1,
            summary: "git commit failed".to_string(),
            ..Default::default()
        };
        let mut result = session_result();

        maybe_record_core_hookspath_conflict(&mut execution, &mut result);

        let warning = result
            .warnings
            .iter()
            .find(|warning| warning.contains("core.hooksPath is set locally"))
            .expect("core.hooksPath warning should be recorded");
        assert!(
            warning.contains("git config --unset-all --local core.hooksPath"),
            "got: {warning}"
        );
        assert!(
            warning.contains("Staged work may be uncommitted"),
            "got: {warning}"
        );
        assert!(
            execution
                .stderr_output
                .contains("git config --unset-all --local core.hooksPath"),
            "stderr should include actionable hint: {}",
            execution.stderr_output
        );
    }

    #[test]
    fn ignores_hookspath_template_without_commit_attempt() {
        let mut execution = csa_process::ExecutionResult {
            stderr_output: "Error: core.hooksPath is set locally to '/x/.git/hooks'\nhint: Unset it:\nhint:   git config --unset-all --local core.hooksPath\n".to_string(),
            exit_code: 0,
            summary: "non-commit success".to_string(),
            ..Default::default()
        };
        let mut result = session_result();

        maybe_record_core_hookspath_conflict(&mut execution, &mut result);

        assert!(
            result.warnings.is_empty(),
            "non-commit hooksPath template alone must not warn: {:?}",
            result.warnings
        );
    }

    #[test]
    fn ignores_unrelated_commit_failure_output() {
        let mut execution = csa_process::ExecutionResult {
            stderr_output: "error: Recipe `clippy` failed on line 12 with exit code 1".to_string(),
            exit_code: 1,
            summary: "clippy failed".to_string(),
            ..Default::default()
        };
        let mut result = session_result();

        maybe_record_core_hookspath_conflict(&mut execution, &mut result);

        assert!(result.warnings.is_empty(), "got: {:?}", result.warnings);
        assert!(
            !execution
                .stderr_output
                .contains("git config --unset-all --local core.hooksPath"),
            "stderr should not include core.hooksPath hint: {}",
            execution.stderr_output
        );
    }
}
