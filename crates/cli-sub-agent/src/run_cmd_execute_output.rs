use std::path::Path;

use anyhow::Result;
use csa_core::types::OutputFormat;
use csa_process::ExecutionResult;

use crate::error_hints::sandbox_fs_denial_hint;
use crate::pipeline_sandbox::filesystem_sandbox_active;

pub(super) fn emit_run_result_output(
    project_root: &Path,
    output_format: OutputFormat,
    executed_session_id: Option<&str>,
    result: &ExecutionResult,
) -> Result<()> {
    match output_format {
        OutputFormat::Text => {
            print!("{}", result.output);
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
