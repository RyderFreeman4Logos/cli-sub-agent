use crate::compiler::{ExecutionPlan, FailAction};

use super::{common_prefix_len, format_fail_action, step_condition_atoms};

const DEFAULT_COLUMNS: usize = 100;
const MIN_COLUMNS: usize = 60;

pub fn render_ascii(plan: &ExecutionPlan) -> String {
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(DEFAULT_COLUMNS);
    render_ascii_with_width(plan, width)
}

pub fn render_ascii_with_width(plan: &ExecutionPlan, width: usize) -> String {
    let width = width.max(MIN_COLUMNS);
    let mut lines = Vec::new();

    lines.push(clamp_line(format!("Plan: {}", plan.name), width));
    if !plan.description.is_empty() {
        lines.push(clamp_line(plan.description.clone(), width));
    }

    if !plan.variables.is_empty() {
        lines.extend(render_variables_box(plan, width));
    }

    let mut prev_atoms = Vec::new();
    for step in &plan.steps {
        let atoms = step_condition_atoms(step);
        let common = common_prefix_len(&prev_atoms, &atoms);

        for (depth, atom) in atoms.iter().enumerate().skip(common) {
            let indent = "  ".repeat(depth);
            let branch = if atom.truthy { "Yes" } else { "No" };
            lines.push(clamp_line(format!("{indent}◇ if {} ?", atom.expr), width));
            lines.push(clamp_line(format!("{indent}  {branch}:"), width));
        }

        let indent = "  ".repeat(atoms.len());
        let tool = step.tool.as_deref().unwrap_or("none");
        let header = format!("{}┌─ [{}] {} [{}]", indent, step.id, step.title, tool);
        lines.push(clamp_line(header, width));

        if let Some(loop_var) = &step.loop_var {
            lines.push(clamp_line(
                format!(
                    "{}│ loop: {} in {}",
                    indent, loop_var.variable, loop_var.collection
                ),
                width,
            ));
        }

        if !step.prompt.is_empty() {
            let preview = step.prompt.lines().next().unwrap_or_default();
            lines.push(clamp_line(format!("{indent}│ {}", preview.trim()), width));
        }

        if !matches!(step.on_fail, FailAction::Abort) {
            lines.push(clamp_line(
                format!(
                    "{}│ on_fail - - > {}",
                    indent,
                    format_fail_action(&step.on_fail)
                ),
                width,
            ));
        }
        lines.push(clamp_line(format!("{indent}└─"), width));
        prev_atoms = atoms;
    }

    lines.join("\n")
}

fn render_variables_box(plan: &ExecutionPlan, width: usize) -> Vec<String> {
    let names = plan
        .variables
        .iter()
        .map(|v| v.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    render_box("Variables", &names, width)
}

fn render_box(title: &str, content: &str, width: usize) -> Vec<String> {
    let inner = width.saturating_sub(2).max(20);
    let title_text = format!(" {title} ");
    let title_width = title_text.chars().count();
    let dash_count = inner.saturating_sub(title_width);
    let top = format!("┌{title_text}{}┐", "─".repeat(dash_count));
    let content = truncate(content, inner);
    let middle = format!("│{content:<inner$}│");
    let bottom = format!("└{}┘", "─".repeat(inner));
    vec![
        clamp_line(top, width),
        clamp_line(middle, width),
        clamp_line(bottom, width),
    ]
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let prefix_len = max_chars - 3;
    let prefix: String = input.chars().take(prefix_len).collect();
    format!("{prefix}...")
}

fn clamp_line(line: String, width: usize) -> String {
    truncate(&line, width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::{LoopSpec, PlanStep, VariableDecl};

    fn sample_plan() -> ExecutionPlan {
        ExecutionPlan {
            name: "demo".to_string(),
            description: "visual layout demo".to_string(),
            variables: vec![
                VariableDecl {
                    name: "APP".to_string(),
                    default: None,
                },
                VariableDecl {
                    name: "ENV".to_string(),
                    default: None,
                },
            ],
            steps: vec![
                PlanStep {
                    id: 1,
                    title: "Build".to_string(),
                    tool: Some("codex".to_string()),
                    prompt: "Build binaries".to_string(),
                    tier: None,
                    depends_on: Vec::new(),
                    on_fail: FailAction::Abort,
                    condition: None,
                    loop_var: None,
                    session: None,
                },
                PlanStep {
                    id: 2,
                    title: "Run Tests".to_string(),
                    tool: Some("claude-code".to_string()),
                    prompt: "Execute full test matrix".to_string(),
                    tier: None,
                    depends_on: vec![1],
                    on_fail: FailAction::Retry(2),
                    condition: Some("has_tests".to_string()),
                    loop_var: None,
                    session: None,
                },
                PlanStep {
                    id: 3,
                    title: "Deploy".to_string(),
                    tool: Some("codex".to_string()),
                    prompt: "Deploy to target".to_string(),
                    tier: None,
                    depends_on: vec![2],
                    on_fail: FailAction::Skip,
                    condition: Some("!(has_tests)".to_string()),
                    loop_var: Some(LoopSpec {
                        variable: "region".to_string(),
                        collection: "${REGIONS}".to_string(),
                        max_iterations: 10,
                    }),
                    session: None,
                },
            ],
        }
    }

    #[test]
    fn test_render_ascii_snapshot_width_80() {
        let output = render_ascii_with_width(&sample_plan(), 80);
        let expected = r#"Plan: demo
visual layout demo
┌ Variables ───────────────────────────────────────────────────────────────────┐
│APP, ENV                                                                      │
└──────────────────────────────────────────────────────────────────────────────┘
┌─ [1] Build [codex]
│ Build binaries
└─
◇ if has_tests ?
  Yes:
  ┌─ [2] Run Tests [claude-code]
  │ Execute full test matrix
  │ on_fail - - > retry:2
  └─
◇ if has_tests ?
  No:
  ┌─ [3] Deploy [codex]
  │ loop: region in ${REGIONS}
  │ Deploy to target
  │ on_fail - - > skip
  └─"#;
        assert_eq!(output, expected);
    }

    #[test]
    fn test_render_ascii_snapshot_width_120() {
        let output = render_ascii_with_width(&sample_plan(), 120);
        let expected = r#"Plan: demo
visual layout demo
┌ Variables ───────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│APP, ENV                                                                                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
┌─ [1] Build [codex]
│ Build binaries
└─
◇ if has_tests ?
  Yes:
  ┌─ [2] Run Tests [claude-code]
  │ Execute full test matrix
  │ on_fail - - > retry:2
  └─
◇ if has_tests ?
  No:
  ┌─ [3] Deploy [codex]
  │ loop: region in ${REGIONS}
  │ Deploy to target
  │ on_fail - - > skip
  └─"#;
        assert_eq!(output, expected);
    }
}
