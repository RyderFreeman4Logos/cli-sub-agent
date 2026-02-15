use std::path::Path;

use anyhow::{Result, bail};

use crate::compiler::ExecutionPlan;

/// Render an execution plan as a minimal ASCII representation.
pub fn render_ascii(plan: &ExecutionPlan) -> String {
    let mut lines = vec![format!("Plan: {}", plan.name)];
    for step in &plan.steps {
        let tool = step.tool.as_deref().unwrap_or("none");
        lines.push(format!("- {}. {} [{tool}]", step.id, step.title));
    }
    lines.join("\n")
}

/// Render an execution plan as Mermaid flowchart text.
pub fn render_mermaid(plan: &ExecutionPlan) -> String {
    let mut out = String::from("flowchart TD\n");
    for step in &plan.steps {
        let title = step.title.replace('"', "\\\"");
        out.push_str(&format!("  S{}[\"{}. {}\"]\n", step.id, step.id, title));
    }
    for pair in plan.steps.windows(2) {
        out.push_str(&format!("  S{} --> S{}\n", pair[0].id, pair[1].id));
    }
    out
}

/// Render an execution plan as PNG.
pub fn render_png(_plan: &ExecutionPlan, _output: &Path) -> Result<()> {
    bail!("PNG rendering is not available yet")
}
