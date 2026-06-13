use std::path::Path;

use anyhow::Result;
use csa_core::types::OutputFormat;
use csa_process::ExecutionResult;

use crate::error_hints::sandbox_fs_denial_hint;
use crate::pipeline_sandbox::filesystem_sandbox_active;

pub(super) fn enrich_ephemeral_signal_diagnostics(result: &mut ExecutionResult) {
    if result.exit_signal.is_none() {
        return;
    }
    let Some(diagnostic) = crate::session_kill_diagnostics::diagnose_ephemeral_signal_kill(
        result.exit_code,
        result.terminal_reason.as_deref(),
    ) else {
        return;
    };

    let line = diagnostic
        .stderr_line()
        .unwrap_or_else(|| diagnostic.ephemeral_line());
    result.kill_hint = Some(diagnostic.hint.as_result_hint().to_string());
    result.resource_diagnostics = Some(line.clone());
    result.summary = line.clone();
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result.stderr_output.push_str(&line);
    result.stderr_output.push('\n');
}

pub(super) fn emit_run_result_output(
    project_root: &Path,
    output_format: OutputFormat,
    executed_session_id: Option<&str>,
    result: &ExecutionResult,
    large_diff_warning: Option<&csa_session::LargeDiffWarningReport>,
) -> Result<()> {
    match output_format {
        OutputFormat::Text => {
            print!("{}", render_run_text_output(result, large_diff_warning));
            if executed_session_id.is_none()
                && result.exit_code != 0
                && result.exit_signal.is_some()
                && let Some(line) = result.resource_diagnostics.as_deref()
            {
                eprintln!("{line}");
            }
            if result.exit_code != 0
                && let Some(sid) = executed_session_id
                && sandbox_fs_denial_hint(&result.stderr_output, &result.output, true, sid)
                    .is_some()
            {
                let fs_sandbox_active = csa_session::load_session(project_root, sid)
                    .ok()
                    .and_then(|session| {
                        session
                            .sandbox_info
                            .as_ref()
                            .map(|info| filesystem_sandbox_active(Some(info)))
                    })
                    .unwrap_or(false);
                if let Some(hint) = sandbox_fs_denial_hint(
                    &result.stderr_output,
                    &result.output,
                    fs_sandbox_active,
                    sid,
                ) {
                    eprintln!("{hint}");
                }
            }
        }
        OutputFormat::Json => {
            let json = render_run_json_output(result, large_diff_warning)?;
            println!("{json}");
        }
    }

    Ok(())
}

pub(super) fn render_run_json_output(
    result: &ExecutionResult,
    large_diff_warning: Option<&csa_session::LargeDiffWarningReport>,
) -> Result<String> {
    let mut value = serde_json::to_value(result)?;
    if let Some(warning) = large_diff_warning
        && let serde_json::Value::Object(fields) = &mut value
    {
        fields.insert(
            "large_diff_warning".to_string(),
            serde_json::to_value(warning)?,
        );
        fields.insert(
            "large_diff_warning_block".to_string(),
            serde_json::Value::String(crate::run_cmd::format_large_diff_warning_block(warning)),
        );
    }
    Ok(serde_json::to_string_pretty(&value)?)
}

pub(super) fn render_run_text_output(
    result: &ExecutionResult,
    large_diff_warning: Option<&csa_session::LargeDiffWarningReport>,
) -> String {
    let mut output = render_success_text_output(&result.output, result.exit_code);
    if let Some(warning) = large_diff_warning {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&crate::run_cmd::format_large_diff_warning_block(warning));
        output.push('\n');
    }
    output
}

fn render_success_text_output(raw_output: &str, exit_code: i32) -> String {
    if exit_code == 0
        && crate::codex_transcript_filter::first_non_empty_line_is_thread_started(raw_output)
    {
        return crate::codex_transcript_filter::extract_codex_json_event_text(raw_output)
            .unwrap_or_else(|| raw_output.to_string());
    }

    raw_output.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ephemeral_signal_diagnostic_adds_no_session_metadata() {
        let mut result = ExecutionResult {
            output: String::new(),
            stderr_output: "process killed by signal 9 (SIGKILL)\n".to_string(),
            summary: "process killed by signal 9 (SIGKILL)".to_string(),
            exit_code: 137,
            raw_process_exit_code: Some(137),
            exit_signal: Some(9),
            terminal_reason: Some("signal".to_string()),
            model_completed: Some(false),
            ..Default::default()
        };

        enrich_ephemeral_signal_diagnostics(&mut result);

        assert!(result.kill_hint.is_some());
        assert!(
            result
                .resource_diagnostics
                .as_deref()
                .is_some_and(|line| line.contains("CSA diagnostic")),
            "ephemeral signal result should carry caller-visible diagnostics"
        );
        assert!(
            result
                .stderr_output
                .contains("No persistent session metadata")
                || result.stderr_output.contains("signal kill hint"),
            "stderr should explain that there is no session result to inspect: {}",
            result.stderr_output
        );
        let json = serde_json::to_value(&result).expect("ExecutionResult should serialize");
        assert_eq!(json["exit_signal"], serde_json::json!(9));
        assert!(json["kill_hint"].is_string());
        assert!(json["resource_diagnostics"].is_string());
    }

    #[test]
    fn run_text_output_appends_large_diff_warning_block() {
        let result = ExecutionResult {
            output: "done\n".to_string(),
            summary: "done".to_string(),
            exit_code: 0,
            ..Default::default()
        };
        let warning = csa_session::LargeDiffWarningReport {
            changed_files: 9,
            changed_lines: 1_420,
            approx_diff_tokens: 18_000,
        };

        let rendered = render_run_text_output(&result, Some(&warning));

        assert!(rendered.starts_with("done\n"));
        assert!(rendered.contains(
            "<!-- CSA:LARGE_DIFF_WARNING changed_files=9 changed_lines=1420 approx_diff_tokens=18000 -->"
        ));
        assert!(rendered.contains("<!-- CSA:LARGE_DIFF_WARNING:END -->"));
    }

    #[test]
    fn run_text_output_omits_large_diff_warning_when_absent() {
        let result = ExecutionResult {
            output: "done\n".to_string(),
            summary: "done".to_string(),
            exit_code: 0,
            ..Default::default()
        };

        let rendered = render_run_text_output(&result, None);

        assert_eq!(rendered, "done\n");
    }

    #[test]
    fn run_text_output_filters_codex_internal_stale_todo_events_on_success() {
        let final_summary = "\
<!-- CSA:SECTION:summary -->
Implemented and committed.
<!-- CSA:SECTION:summary:END -->";
        let stale_todo = json!({
            "todos": [
                {"text": "implement fix", "completed": false},
                {"text": "run validation", "completed": false}
            ]
        })
        .to_string();
        let raw_output = [
            json!({"type": "thread.started", "thread_id": "thread_1"}).to_string(),
            json!({
                "type": "item.completed",
                "item": {"id": "item_1", "type": "agent_message", "text": final_summary}
            })
            .to_string(),
            json!({
                "type": "item.completed",
                "item": {"id": "item_2", "type": "tool_result", "text": stale_todo}
            })
            .to_string(),
            json!({"type": "turn.completed", "usage": {"input_tokens": 100}}).to_string(),
        ]
        .join("\n");
        let result = ExecutionResult {
            output: raw_output,
            summary: "Implemented and committed.".to_string(),
            exit_code: 0,
            terminal_reason: Some("turn.completed".to_string()),
            model_completed: Some(true),
            ..Default::default()
        };

        let rendered = render_run_text_output(&result, None);

        assert_eq!(rendered, final_summary);
        assert!(rendered.contains("Implemented and committed."));
        assert!(!rendered.contains("\"completed\":false"));
        assert!(!rendered.contains("turn.completed"));
        assert!(!rendered.trim_end().ends_with("\"completed\":false}]}}"));
    }

    #[test]
    fn run_text_output_preserves_codex_raw_output_on_failure() {
        let raw_output = [
            json!({"type": "thread.started", "thread_id": "thread_1"}).to_string(),
            json!({
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "tool_result",
                    "text": "{\"todos\":[{\"text\":\"debug\",\"completed\":false}]}"
                }
            })
            .to_string(),
            json!({"type": "turn.failed", "error": {"message": "tool failed"}}).to_string(),
        ]
        .join("\n");
        let result = ExecutionResult {
            output: raw_output.clone(),
            summary: "tool failed".to_string(),
            exit_code: 1,
            terminal_reason: Some("failed".to_string()),
            model_completed: Some(false),
            ..Default::default()
        };

        let rendered = render_run_text_output(&result, None);

        assert_eq!(rendered, raw_output);
        assert!(rendered.contains(r#"\"completed\":false"#));
        assert!(rendered.contains("turn.failed"));
    }

    #[test]
    fn run_json_output_includes_large_diff_warning_data_and_block() {
        let result = ExecutionResult {
            output: "done\n".to_string(),
            summary: "done".to_string(),
            exit_code: 0,
            ..Default::default()
        };
        let warning = csa_session::LargeDiffWarningReport {
            changed_files: 9,
            changed_lines: 1_420,
            approx_diff_tokens: 18_000,
        };

        let rendered = render_run_json_output(&result, Some(&warning))
            .expect("run JSON output should serialize");
        let json: serde_json::Value =
            serde_json::from_str(&rendered).expect("run JSON output should parse");

        assert_eq!(json["output"], serde_json::json!("done\n"));
        assert_eq!(json["large_diff_warning"]["changed_files"], 9);
        assert_eq!(json["large_diff_warning"]["changed_lines"], 1_420);
        assert_eq!(json["large_diff_warning"]["approx_diff_tokens"], 18_000);
        let block = json["large_diff_warning_block"]
            .as_str()
            .expect("large_diff_warning_block should be a string");
        assert!(block.contains(
            "<!-- CSA:LARGE_DIFF_WARNING changed_files=9 changed_lines=1420 approx_diff_tokens=18000 -->"
        ));
        assert!(block.contains("<!-- CSA:LARGE_DIFF_WARNING:END -->"));
    }

    #[test]
    fn run_json_output_omits_large_diff_warning_when_absent() {
        let result = ExecutionResult {
            output: "done\n".to_string(),
            summary: "done".to_string(),
            exit_code: 0,
            ..Default::default()
        };

        let rendered =
            render_run_json_output(&result, None).expect("run JSON output should serialize");
        let json: serde_json::Value =
            serde_json::from_str(&rendered).expect("run JSON output should parse");

        assert!(json.get("large_diff_warning").is_none());
        assert!(json.get("large_diff_warning_block").is_none());
    }
}
