//! Post-run commit policy helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use csa_core::transport_events::{SessionEvent, StreamingMetadata};
use csa_core::types::OutputFormat;
use std::collections::HashMap;

use super::git::PostRunCommitGuard;
use super::shell::{
    detect_git_commit_commands, detect_hook_bypass_env_usage, detect_lefthook_bypass_commands,
    detect_no_verify_commit_commands,
};

const POST_RUN_POLICY_BLOCKED_SUMMARY: &str =
    "post-run policy blocked: workspace mutated without commit";
const POST_RUN_POLICY_REF_UPDATE_FAILED_SUMMARY: &str =
    "post-run policy blocked: git commit was attempted but HEAD did not advance";
const POST_RUN_POLICY_UNVERIFIABLE_SUMMARY: &str =
    "post-run policy blocked: unable to verify workspace mutation state";
const POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY: &str =
    "post-run policy blocked: forbidden git commit --no-verify detected";
const POST_RUN_POLICY_FORBIDDEN_LEFTHOOK_BYPASS_SUMMARY: &str =
    "post-run policy blocked: forbidden LEFTHOOK=0/LEFTHOOK_SKIP bypass detected";
const ALLOW_NO_VERIFY_COMMIT_MARKER: &str = "allow_git_commit_no_verify=1";

pub(crate) fn resolve_hook_bypass_scan_enabled(
    cli_no_hook_bypass_scan: bool,
    config_hook_bypass_scan: Option<bool>,
) -> bool {
    !cli_no_hook_bypass_scan && config_hook_bypass_scan.unwrap_or(true)
}

#[cfg(test)]
pub(crate) fn is_post_run_commit_policy_block(summary: &str) -> bool {
    summary == POST_RUN_POLICY_BLOCKED_SUMMARY
        || summary == POST_RUN_POLICY_REF_UPDATE_FAILED_SUMMARY
        || summary == POST_RUN_POLICY_UNVERIFIABLE_SUMMARY
        || summary == POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY
        || summary == POST_RUN_POLICY_FORBIDDEN_LEFTHOOK_BYPASS_SUMMARY
}

pub(crate) fn is_post_run_commit_policy_gate_failure(
    result: &csa_process::ExecutionResult,
) -> bool {
    result
        .csa_gate_failure
        .as_deref()
        .is_some_and(is_post_run_commit_policy_gate_failure_reason)
}

fn is_post_run_commit_policy_gate_failure_reason(reason: &str) -> bool {
    matches!(
        reason,
        "commit-policy-uncommitted"
            | "commit-policy-ref-update"
            | "commit-policy-unverifiable"
            | "commit-policy-no-verify"
            | "commit-policy-lefthook-bypass"
    )
}

pub(crate) fn apply_post_run_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    recovery_tool: Option<&str>,
    require_commit_on_mutation: bool,
    git_commit_attempted: bool,
    commit_guard: Option<&PostRunCommitGuard>,
) {
    let Some(commit_guard) = commit_guard else {
        return;
    };

    let commit_ref_update_failed =
        git_commit_attempted && commit_guard.workspace_mutated && !commit_guard.head_changed;
    let enforce_closed_policy = commit_guard.workspace_mutated
        && !commit_guard.head_changed
        && (require_commit_on_mutation || commit_ref_update_failed);
    let guard_message = format_post_run_commit_guard_message(
        commit_guard,
        enforce_closed_policy,
        commit_ref_update_failed,
        recovery_tool,
    );

    if enforce_closed_policy {
        let previous_summary = result.summary.clone();
        let (gate_reason, blocked_summary) = if commit_ref_update_failed {
            (
                "commit-policy-ref-update",
                POST_RUN_POLICY_REF_UPDATE_FAILED_SUMMARY,
            )
        } else {
            ("commit-policy-uncommitted", POST_RUN_POLICY_BLOCKED_SUMMARY)
        };
        // CSA-own gate: the run left required/attempted commit work uncommitted.
        // Mark it so the #161 classifier treats the exit as authoritative-fatal,
        // preserving a more specific pre-existing failure exit code if the run
        // already failed.
        result.note_gate_failure(gate_reason);
        if !previous_summary.trim().is_empty() && previous_summary != blocked_summary {
            append_stderr_block(
                &mut result.stderr_output,
                &format!("Original summary before commit policy: {previous_summary}"),
            );
        }
        append_stderr_block(&mut result.stderr_output, blocked_summary);
    }

    match output_format {
        OutputFormat::Text => eprintln!("{guard_message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, &guard_message),
    }
}

pub(crate) struct PostSessionCommitPolicyArgs<'a> {
    pub(crate) output_format: &'a OutputFormat,
    pub(crate) prompt: &'a str,
    pub(crate) tool_name: &'a str,
    pub(crate) require_commit_on_mutation: bool,
    pub(crate) commit_guard: Option<&'a PostRunCommitGuard>,
    pub(crate) policy_evaluation_failed: bool,
    pub(crate) hook_bypass_scan_enabled: bool,
    pub(crate) executed_shell_commands: &'a [String],
    pub(crate) merged_env_ref: Option<&'a HashMap<String, String>>,
    pub(crate) execute_events_observed: bool,
}

pub(crate) fn apply_post_session_commit_policies(
    result: &mut csa_process::ExecutionResult,
    args: PostSessionCommitPolicyArgs<'_>,
) {
    let git_commit_attempted = !detect_git_commit_commands(args.executed_shell_commands).is_empty();
    apply_post_run_commit_policy(
        result,
        args.output_format,
        Some(args.tool_name),
        args.require_commit_on_mutation,
        git_commit_attempted,
        args.commit_guard,
    );
    apply_unverifiable_commit_policy(result, args.output_format, args.policy_evaluation_failed);
    if args.hook_bypass_scan_enabled {
        apply_no_verify_commit_policy(
            result,
            args.output_format,
            args.prompt,
            args.executed_shell_commands,
            args.execute_events_observed,
        );
        apply_lefthook_bypass_policy(
            result,
            args.output_format,
            args.executed_shell_commands,
            args.merged_env_ref,
            args.execute_events_observed,
        );
    }
}

pub(crate) fn apply_unverifiable_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    policy_evaluation_failed: bool,
) {
    if !policy_evaluation_failed {
        return;
    }

    let previous_summary = result.summary.clone();
    // CSA-own gate: unable to verify workspace mutation state. Mark it so the
    // #161 classifier treats the exit as authoritative-fatal, preserving a more
    // specific pre-existing failure exit code if the run already failed.
    result.note_gate_failure("commit-policy-unverifiable");
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_UNVERIFIABLE_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    append_stderr_block(
        &mut result.stderr_output,
        POST_RUN_POLICY_UNVERIFIABLE_SUMMARY,
    );

    let guard_message =
        "ERROR: strict commit policy could not verify workspace mutation state; run is blocked.";
    match output_format {
        OutputFormat::Text => eprintln!("{guard_message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, guard_message),
    }
}

pub(crate) fn apply_no_verify_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    prompt: &str,
    executed_shell_commands: &[String],
    _execute_events_observed: bool,
) {
    if prompt_allows_no_verify_commit(prompt) {
        return;
    }

    let matched_commands = detect_no_verify_commit_commands(executed_shell_commands);
    if matched_commands.is_empty() {
        return;
    }

    let previous_summary = result.summary.clone();
    // CSA-own gate: forbidden `git commit --no-verify` detected. Mark it so the
    // #161 classifier treats the exit as authoritative-fatal, preserving a more
    // specific pre-existing failure exit code if the run already failed.
    result.note_gate_failure("commit-policy-no-verify");
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    append_stderr_block(
        &mut result.stderr_output,
        POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY,
    );

    let mut message = String::from(
        "ERROR: forbidden git hook/signature bypass flag detected in executed shell commands.\n\
Forbidden forms include `git commit --no-verify`, `git commit -n`, `git commit --no-gpg-sign`, and `git push --no-verify`.\n\
If this is intentional, add `ALLOW_GIT_COMMIT_NO_VERIFY=1` to the prompt.\n\
Matched commands:",
    );
    for command in matched_commands {
        message.push_str("\n- ");
        message.push_str(&command);
    }
    match output_format {
        OutputFormat::Text => eprintln!("{message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, &message),
    }
}

/// Block the run if any executed command sets `LEFTHOOK=0` or `LEFTHOOK_SKIP`
/// to bypass pre-commit hooks.  This enforces AGENTS.md rule 029
/// (hook-bypass-prevention).
pub(crate) fn apply_lefthook_bypass_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    executed_shell_commands: &[String],
    execution_env: Option<&HashMap<String, String>>,
    _execute_events_observed: bool,
) {
    let mut matched_commands = detect_lefthook_bypass_commands(executed_shell_commands);
    matched_commands.extend(detect_hook_bypass_env_usage(
        executed_shell_commands,
        execution_env,
    ));
    if matched_commands.is_empty() {
        return;
    }

    let previous_summary = result.summary.clone();
    // CSA-own gate: forbidden LEFTHOOK bypass detected. Mark it so the #161
    // classifier treats the exit as authoritative-fatal, preserving a more
    // specific pre-existing failure exit code if the run already failed.
    result.note_gate_failure("commit-policy-lefthook-bypass");
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_FORBIDDEN_LEFTHOOK_BYPASS_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    append_stderr_block(
        &mut result.stderr_output,
        POST_RUN_POLICY_FORBIDDEN_LEFTHOOK_BYPASS_SUMMARY,
    );

    let mut message = String::from(
        "ERROR: forbidden hook-bypass environment detected in executed shell commands or process env.\n\
Hook bypass is absolutely prohibited (AGENTS.md rule 029).\n\
Matched commands:",
    );
    for command in matched_commands {
        message.push_str("\n- ");
        message.push_str(&command);
    }
    match output_format {
        OutputFormat::Text => eprintln!("{message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, &message),
    }
}

pub(crate) fn format_post_run_commit_guard_message(
    guard: &PostRunCommitGuard,
    enforce_closed_policy: bool,
    commit_ref_update_failed: bool,
    recovery_tool: Option<&str>,
) -> String {
    let severity = if enforce_closed_policy {
        "ERROR"
    } else {
        "WARNING"
    };
    let reason = if commit_ref_update_failed {
        "git commit was attempted but HEAD did not advance; work remains uncommitted"
    } else if guard.head_changed {
        "run created commit(s) but still left uncommitted workspace mutations"
    } else {
        "run left uncommitted workspace mutations compared to start"
    };

    let mut lines = vec![format!("{severity}: csa run completed but {reason}.")];
    let recovery_command = recovery_tool
        .map(|tool| format!("csa run --tool {tool} --skill commit \"<scope>\""))
        .unwrap_or_else(|| "csa run --skill commit \"<scope>\"".to_string());
    lines.push(format!(
        "Next step: run `{recovery_command}` and continue with PR/review workflow."
    ));
    lines.push(
        "Nested commit runs inside an active CSA session are supported as lineage-scoped child sessions; unrelated writers remain serialized by the worktree lock."
            .to_string(),
    );
    if !guard.changed_paths.is_empty() {
        lines.push(format!("Changed paths: {}", guard.changed_paths.join(", ")));
    }
    lines.join("\n")
}

fn prompt_allows_no_verify_commit(prompt: &str) -> bool {
    prompt.lines().any(|line| {
        let normalized = strip_marker_line_prefix(line).trim().to_ascii_lowercase();
        normalized == ALLOW_NO_VERIFY_COMMIT_MARKER
            || normalized == format!("policy override: {ALLOW_NO_VERIFY_COMMIT_MARKER}")
    })
}

fn strip_marker_line_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(stripped) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return stripped.trim_start();
    }

    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_count > 0 {
        let suffix = &trimmed[digit_count..];
        if let Some(stripped) = suffix
            .strip_prefix(". ")
            .or_else(|| suffix.strip_prefix(") "))
        {
            return stripped.trim_start();
        }
    }

    trimmed
}

pub(crate) fn extract_executed_shell_commands(
    metadata: &StreamingMetadata,
    events: &[SessionEvent],
) -> Vec<String> {
    if !metadata.extracted_commands.is_empty() {
        return dedupe_commands(metadata.extracted_commands.iter().cloned());
    }
    extract_executed_shell_commands_from_events(events)
}

pub(crate) fn execute_tool_calls_observed(
    metadata: &StreamingMetadata,
    events: &[SessionEvent],
) -> bool {
    metadata.has_execute_tool_calls || events_contain_execute_tool_calls(events)
}

pub(crate) fn extract_executed_shell_commands_from_events<T: serde::Serialize>(
    events: &[T],
) -> Vec<String> {
    let mut commands = Vec::new();
    for event in events {
        let Ok(value) = serde_json::to_value(event) else {
            continue;
        };
        collect_execute_titles_from_event_value(&value, &mut commands);
    }
    commands
}

pub(crate) fn events_contain_execute_tool_calls<T: serde::Serialize>(events: &[T]) -> bool {
    for event in events {
        let Ok(value) = serde_json::to_value(event) else {
            continue;
        };
        if event_value_contains_execute_kind(&value) {
            return true;
        }
    }
    false
}

fn event_value_contains_execute_kind(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            if map
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("execute"))
            {
                return true;
            }
            map.values().any(event_value_contains_execute_kind)
        }
        serde_json::Value::Array(values) => values.iter().any(event_value_contains_execute_kind),
        _ => false,
    }
}

fn collect_execute_titles_from_event_value(value: &serde_json::Value, commands: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            let kind = map.get("kind").and_then(serde_json::Value::as_str);
            let title = map.get("title").and_then(serde_json::Value::as_str);
            if let (Some(kind), Some(title)) = (kind, title)
                && kind.eq_ignore_ascii_case("execute")
            {
                let command = title.trim();
                if !command.is_empty() && !commands.iter().any(|existing| existing == command) {
                    commands.push(command.to_string());
                }
            }
            for child in map.values() {
                collect_execute_titles_from_event_value(child, commands);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_execute_titles_from_event_value(child, commands);
            }
        }
        _ => {}
    }
}

fn dedupe_commands<I>(commands: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut deduped = Vec::new();
    for command in commands {
        if !deduped.iter().any(|existing| existing == &command) {
            deduped.push(command);
        }
    }
    deduped
}

fn append_stderr_block(stderr_output: &mut String, block: &str) {
    if block.trim().is_empty() {
        return;
    }
    if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
    stderr_output.push_str(block);
    if !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
}
