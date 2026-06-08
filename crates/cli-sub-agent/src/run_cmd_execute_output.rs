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
) -> Result<()> {
    match output_format {
        OutputFormat::Text => {
            print!("{}", result.output);
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
            let json = serde_json::to_string_pretty(result)?;
            println!("{json}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
