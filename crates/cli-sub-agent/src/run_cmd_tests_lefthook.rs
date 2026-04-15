use super::*;
use crate::run_cmd::shell::{
    command_contains_forbidden_lefthook_bypass, segment_contains_forbidden_lefthook_bypass,
};
use csa_core::types::OutputFormat;

#[test]
fn apply_lefthook_bypass_policy_blocks_inline_lefthook_zero() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
fn command_contains_forbidden_lefthook_bypass_ignores_quoted_argument_mentions() {
    assert!(!command_contains_forbidden_lefthook_bypass(
        "echo \"do not set LEFTHOOK=0\""
    ));
    assert!(!command_contains_forbidden_lefthook_bypass(
        "grep -r \"LEFTHOOK=0\" ."
    ));
    assert!(!command_contains_forbidden_lefthook_bypass(
        "echo \"LEFTHOOK_SKIP is forbidden\""
    ));
    assert!(!command_contains_forbidden_lefthook_bypass(
        "sed -i 's/LEFTHOOK=0//' file"
    ));
    assert!(!command_contains_forbidden_lefthook_bypass(
        "echo 'cat <<EOF\nLEFTHOOK=0\nEOF'"
    ));
}

#[test]
fn command_contains_forbidden_lefthook_bypass_still_blocks_real_prefix_assignments() {
    assert!(command_contains_forbidden_lefthook_bypass(
        "LEFTHOOK=0 git commit -m \"msg\""
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "FOO=1 LEFTHOOK=0 git commit -m \"msg\""
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "A=1 B=2 LEFTHOOK_SKIP=pre-commit git push"
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "env LEFTHOOK=0 git commit"
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "export LEFTHOOK=0"
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "sh -c \"export LEFTHOOK=0; git commit\""
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "sh -c \"FOO=1 LEFTHOOK=0 git commit\""
    ));
}

#[test]
fn command_contains_forbidden_lefthook_bypass_blocks_shell_wrapped_separator_resets() {
    assert!(command_contains_forbidden_lefthook_bypass(
        "sh -c \"git status; LEFTHOOK=0 git commit\""
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "sh -c \"git rev-parse HEAD && LEFTHOOK=0 git commit\""
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "sh -c \"git commit && export LEFTHOOK=0\""
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "sh -c \"echo hello | LEFTHOOK=0 git commit\""
    ));
    assert!(!command_contains_forbidden_lefthook_bypass(
        "sh -c \"echo 'LEFTHOOK=0'; git commit\""
    ));
}

#[test]
fn command_contains_forbidden_lefthook_bypass_blocks_wrapper_prefixed_env_forms() {
    assert!(command_contains_forbidden_lefthook_bypass(
        "command env LEFTHOOK=0 git commit"
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "command env A=1 LEFTHOOK=0 git commit"
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "time env LEFTHOOK=0 git commit"
    ));
    assert!(command_contains_forbidden_lefthook_bypass(
        "sudo env LEFTHOOK=0 git commit"
    ));
}

#[test]
fn segment_contains_forbidden_lefthook_bypass_ignores_argument_mentions() {
    assert!(!segment_contains_forbidden_lefthook_bypass(
        "echo \"do not set LEFTHOOK=0\""
    ));
    assert!(!segment_contains_forbidden_lefthook_bypass(
        "grep -r \"LEFTHOOK=0\" ."
    ));
    assert!(!segment_contains_forbidden_lefthook_bypass(
        "echo \"LEFTHOOK_SKIP is forbidden\""
    ));
    assert!(!segment_contains_forbidden_lefthook_bypass(
        "sed -i 's/LEFTHOOK=0//' file"
    ));
    assert!(!segment_contains_forbidden_lefthook_bypass(
        "echo 'cat <<EOF\nLEFTHOOK=0\nEOF'"
    ));
}

#[test]
fn segment_contains_forbidden_lefthook_bypass_still_blocks_real_prefix_assignments() {
    assert!(segment_contains_forbidden_lefthook_bypass(
        "LEFTHOOK=0 git commit -m \"msg\""
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "FOO=1 LEFTHOOK=0 git commit -m \"msg\""
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "A=1 B=2 LEFTHOOK_SKIP=pre-commit git push"
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "env LEFTHOOK=0 git commit"
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "export LEFTHOOK=0"
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sh -c \"export LEFTHOOK=0; git commit\""
    ));
}

#[test]
fn segment_contains_forbidden_lefthook_bypass_blocks_wrapper_prefixed_env_forms() {
    assert!(segment_contains_forbidden_lefthook_bypass(
        "command env LEFTHOOK=0 git commit"
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "command env A=1 LEFTHOOK=0 git commit"
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "time env LEFTHOOK=0 git commit"
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sudo env LEFTHOOK=0 git commit"
    ));
}

#[test]
fn segment_contains_forbidden_lefthook_bypass_blocks_shell_wrapped_multi_assignment_prefixes() {
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sh -c \"FOO=1 LEFTHOOK=0 git commit\""
    ));
}

#[test]
fn segment_contains_forbidden_lefthook_bypass_blocks_shell_wrapped_separator_resets() {
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sh -c \"git status; LEFTHOOK=0 git commit\""
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sh -c \"git rev-parse HEAD && LEFTHOOK=0 git commit\""
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sh -c \"git commit && export LEFTHOOK=0\""
    ));
    assert!(segment_contains_forbidden_lefthook_bypass(
        "sh -c \"echo hello | LEFTHOOK=0 git commit\""
    ));
    assert!(!segment_contains_forbidden_lefthook_bypass(
        "sh -c \"echo 'LEFTHOOK=0'; git commit\""
    ));
}

#[test]
fn apply_lefthook_bypass_policy_allows_normal_git_commands() {
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
        peak_memory_mb: None,
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
