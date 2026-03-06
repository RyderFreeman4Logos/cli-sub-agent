//! Post-run commit policy helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use csa_core::types::OutputFormat;

use super::git::PostRunCommitGuard;
use super::shell::{
    detect_no_verify_commit_commands, detect_no_verify_commit_commands_from_tool_output,
};

const POST_RUN_POLICY_BLOCKED_SUMMARY: &str =
    "post-run policy blocked: workspace mutated without commit";
const POST_RUN_POLICY_UNVERIFIABLE_SUMMARY: &str =
    "post-run policy blocked: unable to verify workspace mutation state";
const POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY: &str =
    "post-run policy blocked: forbidden git commit --no-verify detected";
const ALLOW_NO_VERIFY_COMMIT_MARKER: &str = "allow_git_commit_no_verify=1";

pub(crate) fn is_post_run_commit_policy_block(summary: &str) -> bool {
    summary == POST_RUN_POLICY_BLOCKED_SUMMARY
        || summary == POST_RUN_POLICY_UNVERIFIABLE_SUMMARY
        || summary == POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY
}

pub(crate) fn apply_post_run_commit_policy(
    result: &mut csa_process::ExecutionResult,
    output_format: &OutputFormat,
    require_commit_on_mutation: bool,
    commit_guard: Option<&PostRunCommitGuard>,
) {
    let Some(commit_guard) = commit_guard else {
        return;
    };

    let enforce_closed_policy =
        require_commit_on_mutation && commit_guard.workspace_mutated && !commit_guard.head_changed;
    let guard_message = format_post_run_commit_guard_message(commit_guard, enforce_closed_policy);

    if enforce_closed_policy {
        let previous_summary = result.summary.clone();
        if result.exit_code == 0 {
            result.exit_code = 1;
        }
        if !previous_summary.trim().is_empty()
            && previous_summary != POST_RUN_POLICY_BLOCKED_SUMMARY
        {
            append_stderr_block(
                &mut result.stderr_output,
                &format!("Original summary before commit policy: {previous_summary}"),
            );
        }
        result.summary = POST_RUN_POLICY_BLOCKED_SUMMARY.to_string();
    }

    match output_format {
        OutputFormat::Text => eprintln!("{guard_message}"),
        OutputFormat::Json => append_stderr_block(&mut result.stderr_output, &guard_message),
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
    if result.exit_code == 0 {
        result.exit_code = 1;
    }
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_UNVERIFIABLE_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    result.summary = POST_RUN_POLICY_UNVERIFIABLE_SUMMARY.to_string();

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
    execute_events_observed: bool,
) {
    if prompt_allows_no_verify_commit(prompt) {
        return;
    }

    let mut matched_commands = detect_no_verify_commit_commands(executed_shell_commands);
    if matched_commands.is_empty() && !execute_events_observed {
        matched_commands = detect_no_verify_commit_commands_from_tool_output(
            result,
            !executed_shell_commands.is_empty(),
        );
    }
    if matched_commands.is_empty() {
        return;
    }

    let previous_summary = result.summary.clone();
    if result.exit_code == 0 {
        result.exit_code = 1;
    }
    if !previous_summary.trim().is_empty()
        && previous_summary != POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY
    {
        append_stderr_block(
            &mut result.stderr_output,
            &format!("Original summary before commit policy: {previous_summary}"),
        );
    }
    result.summary = POST_RUN_POLICY_FORBIDDEN_NO_VERIFY_SUMMARY.to_string();

    let mut message = String::from(
        "ERROR: forbidden `git commit --no-verify` (or `git commit -n`) detected in executed shell commands.\n\
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

pub(crate) fn format_post_run_commit_guard_message(
    guard: &PostRunCommitGuard,
    enforce_closed_policy: bool,
) -> String {
    let severity = if enforce_closed_policy {
        "ERROR"
    } else {
        "WARNING"
    };
    let reason = if guard.head_changed {
        "run created commit(s) but still left uncommitted workspace mutations"
    } else {
        "run left uncommitted workspace mutations compared to start"
    };

    let mut lines = vec![format!("{severity}: csa run completed but {reason}.")];
    lines.push(
        "Next step: run `csa run --skill commit \"<scope>\"` and continue with PR/review workflow."
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
