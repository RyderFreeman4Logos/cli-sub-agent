use super::*;
use csa_core::types::OutputFormat;

#[test]
fn apply_lefthook_bypass_policy_blocks_inline_lefthook_zero() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["LEFTHOOK=0 git commit -m \"unsafe\"".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
    assert!(result.stderr_output.contains("Matched commands:"));
    assert!(result.stderr_output.contains("LEFTHOOK=0"));
}

#[test]
fn apply_lefthook_bypass_policy_blocks_env_lefthook_zero() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["env LEFTHOOK=0 git push".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
}

#[test]
fn apply_lefthook_bypass_policy_blocks_export_lefthook_zero() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["export LEFTHOOK=0".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
}

#[test]
fn apply_lefthook_bypass_policy_blocks_lefthook_skip() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands =
        vec!["LEFTHOOK_SKIP=pre-commit git commit -m \"unsafe\"".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
}

#[test]
fn apply_lefthook_bypass_policy_blocks_shell_wrapped_lefthook_bypass() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["bash -c \"LEFTHOOK=0 git commit -m unsafe\"".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
}

#[test]
fn apply_lefthook_bypass_policy_blocks_export_in_shell_wrapper() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands =
        vec!["bash -lc \"export LEFTHOOK=0; git commit -m unsafe\"".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
}

#[test]
fn apply_lefthook_bypass_policy_allows_normal_git_commands() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["git commit -m \"normal commit\"".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_lefthook_bypass_policy_allows_unrelated_env_vars() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["GIT_AUTHOR_NAME=bot git commit -m \"safe\"".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn apply_lefthook_bypass_policy_blocks_output_fallback_when_no_execute_events() {
    let mut result = ExecutionResult {
        output: "$ LEFTHOOK=0 git commit -m \"unsafe\"\n".to_string(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = Vec::new();

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.summary,
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    );
}

#[test]
fn apply_lefthook_bypass_policy_skips_output_fallback_when_execute_events_present() {
    let mut result = ExecutionResult {
        output: "$ LEFTHOOK=0 git commit -m \"mentioned only\"\n".to_string(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };
    let executed_shell_commands = vec!["git status".to_string()];

    apply_lefthook_bypass_policy(
        &mut result,
        &OutputFormat::Json,
        &executed_shell_commands,
        !executed_shell_commands.is_empty(),
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn is_post_run_commit_policy_block_detects_lefthook_bypass_summary() {
    assert!(is_post_run_commit_policy_block(
        "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected"
    ));
}
