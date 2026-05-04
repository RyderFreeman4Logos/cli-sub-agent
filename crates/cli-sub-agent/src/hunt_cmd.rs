use anyhow::{Result, anyhow};
use csa_core::types::{OutputFormat, ToolArg};

const DIAGNOSTIC_PROMPT_PREFIX: &str = r#"DIAGNOSTIC MODE — ROOT-CAUSE-FIRST DEBUGGING

You are in diagnostic hunt mode. You MUST follow this strict sequence:

PHASE 1 — INVESTIGATE (read-only)
- Read code, logs, error messages
- Grep for patterns, inspect git history
- You MUST NOT modify any files in this phase

PHASE 2 — REPRODUCE
- Write a minimal failing test or reproduction script
- Run it to confirm the bug exists
- The test MUST fail demonstrating the bug

PHASE 3 — DIAGNOSE
- State the root cause in ONE sentence
- Format: ROOT_CAUSE: <your one-sentence diagnosis>
- Explain WHY this causes the observed behavior

PHASE 4 — FIX
- Only NOW may you modify production code
- The fix must be minimal and targeted
- Run the reproduction test to verify the fix works

SELF-DECEPTION CHECKLIST (before Phase 4):
- [ ] Have I actually reproduced the bug, or am I guessing?
- [ ] Is my root cause specific (names a file and line), not vague?
- [ ] Could there be a different root cause I haven't considered?"#;

pub(crate) fn build_hunt_prompt(description: &str) -> String {
    format!("{DIAGNOSTIC_PROMPT_PREFIX}\n\nBUG DESCRIPTION:\n{description}")
}

pub(crate) async fn handle_hunt(
    description: String,
    tool: Option<String>,
    timeout: u64,
    allow_base_branch_working: bool,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    let tool = tool
        .map(|raw| {
            raw.parse::<ToolArg>()
                .map_err(|err| anyhow!("invalid hunt tool `{raw}`: {err}"))
        })
        .transpose()?;
    let prompt = build_hunt_prompt(&description);
    let stream_mode = if matches!(output_format, OutputFormat::Text) {
        csa_process::StreamMode::TeeToStderr
    } else {
        csa_process::StreamMode::BufferOnly
    };

    crate::run_cmd::handle_run(
        tool,
        None,
        None,
        None,
        Some(prompt),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        false,
        allow_base_branch_working,
        None,
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        None,
        None,
        Some(timeout),
        false,
        false,
        None,
        current_depth,
        output_format,
        stream_mode,
        None,
        false,
        false,
        Vec::new(),
        Vec::new(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::build_hunt_prompt;

    #[test]
    fn build_hunt_prompt_prepends_diagnostic_state_machine() {
        let prompt = build_hunt_prompt("the command exits before writing the result");

        assert!(prompt.starts_with("DIAGNOSTIC MODE — ROOT-CAUSE-FIRST DEBUGGING"));
        assert!(prompt.contains("PHASE 1 — INVESTIGATE (read-only)"));
        assert!(prompt.contains("ROOT_CAUSE: <your one-sentence diagnosis>"));
        assert!(prompt.ends_with("the command exits before writing the result"));
    }
}
