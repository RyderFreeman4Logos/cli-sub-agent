use std::path::Path;

use weave::compiler::PlanStep;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OrchestratorHandoff {
    ManualResume,
    AwaitUser,
}

pub(crate) fn shell_escape_for_command(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn format_plan_resume_command(
    project_root: &Path,
    workflow_path: &Path,
    journal_path: Option<&Path>,
) -> String {
    if let Some(jp) = journal_path {
        let display = jp.to_string_lossy();
        format!(
            "csa plan run --sa-mode true --resume {}",
            shell_escape_for_command(&display)
        )
    } else {
        let display_path = workflow_path
            .strip_prefix(project_root)
            .unwrap_or(workflow_path);
        let display = display_path.to_string_lossy();
        format!(
            "csa plan run --sa-mode true {}",
            shell_escape_for_command(&display)
        )
    }
}

pub(super) fn orchestrator_handoff_mode(step: &PlanStep) -> Option<OrchestratorHandoff> {
    match step.tool.as_deref().map(str::trim) {
        Some(tool) if tool.eq_ignore_ascii_case("manual") => {
            Some(OrchestratorHandoff::ManualResume)
        }
        Some(tool) if tool.eq_ignore_ascii_case("await-user") => {
            Some(OrchestratorHandoff::AwaitUser)
        }
        _ => None,
    }
}

pub(super) fn format_orchestrator_message(step: &PlanStep, mode: OrchestratorHandoff) -> String {
    let marker = match mode {
        OrchestratorHandoff::ManualResume => format!("MANUAL_STEP: {}", step.title),
        OrchestratorHandoff::AwaitUser => format!("AWAIT_USER: {}", step.title),
    };
    let guidance = step.prompt.trim();
    if guidance.is_empty() {
        marker
    } else {
        format!("{marker}\n{guidance}")
    }
}

/// Find the next step in the plan after the current step.
///
/// Returns the first step with an ID greater than the current step's ID,
/// which is the sequential successor in a linear workflow.
pub(super) fn find_next_step<'a>(
    current: &PlanStep,
    steps: &'a [PlanStep],
) -> Option<&'a PlanStep> {
    steps.iter().find(|s| s.id > current.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_plan_resume_command_uses_journal_path_when_available() {
        let project_root = Path::new("/tmp/workspace");
        let workflow_path = Path::new("/tmp/workspace/patterns/dev2merge/workflow.toml");
        let journal_path = Path::new("/tmp/workspace/.csa/state/plan/dev2merge.journal.json");

        assert_eq!(
            format_plan_resume_command(project_root, workflow_path, Some(journal_path)),
            "csa plan run --sa-mode true --resume '/tmp/workspace/.csa/state/plan/dev2merge.journal.json'"
        );
    }

    #[test]
    fn format_plan_resume_command_falls_back_to_workflow_path() {
        let project_root = Path::new("/tmp/workspace");
        let workflow_path = Path::new("/tmp/workspace/patterns/dev2merge/workflow.toml");

        assert_eq!(
            format_plan_resume_command(project_root, workflow_path, None),
            "csa plan run --sa-mode true 'patterns/dev2merge/workflow.toml'"
        );
    }

    #[test]
    fn format_plan_resume_command_escapes_special_characters() {
        let project_root = Path::new("/tmp/workspace");
        let workflow_path = Path::new("/tmp/workspace/patterns/weird name's/workflow.toml");

        assert_eq!(
            format_plan_resume_command(project_root, workflow_path, None),
            "csa plan run --sa-mode true 'patterns/weird name'\\''s/workflow.toml'"
        );
    }

    #[test]
    fn format_plan_resume_command_escapes_journal_path_special_characters() {
        let project_root = Path::new("/tmp/workspace");
        let workflow_path = Path::new("/tmp/workspace/workflow.toml");
        let journal_path = Path::new("/tmp/workspace/.csa/state/plan/weird name's.journal.json");

        assert_eq!(
            format_plan_resume_command(project_root, workflow_path, Some(journal_path)),
            "csa plan run --sa-mode true --resume '/tmp/workspace/.csa/state/plan/weird name'\\''s.journal.json'"
        );
    }
}
