use weave::compiler::PlanStep;

use super::StepTarget;

const STEP_FAILURE_STDERR_TAIL_LINES: usize = 20;
const STEP_FAILURE_STDERR_TAIL_MAX_CHARS: usize = 4000;

pub(super) fn describe_step_command(
    target: &StepTarget,
    step: &PlanStep,
    forwarded_session: Option<&str>,
) -> String {
    match target {
        StepTarget::DirectBash => {
            super::super::plan_cmd_exec::extract_bash_code_block(&step.prompt)
                .unwrap_or(&step.prompt)
                .to_string()
        }
        StepTarget::CsaTool {
            tool_name,
            model_spec,
            tier_name,
        } => {
            let mut parts = vec![format!("csa plan step via tool={}", tool_name.as_str())];
            if let Some(tier_name) = tier_name.as_deref() {
                parts.push(format!("tier={tier_name}"));
            }
            if let Some(model_spec) = model_spec.as_deref() {
                parts.push(format!("model_spec={model_spec}"));
            }
            if let Some(session) = forwarded_session {
                parts.push(format!("session={session}"));
            }
            parts.join(" ")
        }
        StepTarget::WeaveInclude => "weave include".to_string(),
        StepTarget::Note => "note".to_string(),
        StepTarget::Manual => "manual handoff".to_string(),
        StepTarget::AwaitUser => "await user".to_string(),
    }
}

pub(super) fn format_step_failure_error(exit_code: i32, stderr: &str) -> String {
    let mut error = format!("Exit code {exit_code}");
    if let Some(stderr_tail) = stderr_tail(stderr) {
        error.push_str(&format!(
            "\nstderr (last {STEP_FAILURE_STDERR_TAIL_LINES} lines):\n{stderr_tail}"
        ));
    }
    error
}

pub(super) fn stderr_tail(stderr: &str) -> Option<String> {
    let mut lines = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(STEP_FAILURE_STDERR_TAIL_LINES)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    let mut tail = lines.join("\n");
    if tail.len() > STEP_FAILURE_STDERR_TAIL_MAX_CHARS {
        let keep_from = tail
            .char_indices()
            .rev()
            .find_map(|(idx, _)| {
                (tail.len() - idx <= STEP_FAILURE_STDERR_TAIL_MAX_CHARS).then_some(idx)
            })
            .unwrap_or(0);
        tail = format!("...{}", &tail[keep_from..]);
    }
    Some(tail)
}
