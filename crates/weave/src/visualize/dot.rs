#[cfg(feature = "visualize-png-dot")]
use std::path::Path;

#[cfg(feature = "visualize-png-dot")]
use anyhow::{Context, Result, bail};

use crate::compiler::ExecutionPlan;

use super::{VizEdgeKind, VizNodeKind, build_graph};

pub fn to_dot(plan: &ExecutionPlan) -> String {
    let graph = build_graph(plan);
    let mut out = String::from("digraph weave_plan {\n");
    out.push_str("  rankdir=TB;\n");
    out.push_str("  node [fontname=\"monospace\"];\n");

    for node in &graph.nodes {
        let (shape, label) = match &node.kind {
            VizNodeKind::VariableHeader { names } => {
                ("box", format!("Variables: {}", names.join(", ")))
            }
            VizNodeKind::Step {
                step_id,
                title,
                tool,
                loop_label,
            } => {
                let tool = tool.as_deref().unwrap_or("none");
                let mut label = format!("{step_id}. {title}\\n[{tool}]");
                if let Some(loop_label) = loop_label {
                    label.push_str(&format!("\\nloop: {loop_label}"));
                }
                ("box", label)
            }
            VizNodeKind::Decision {
                condition,
                depth: _,
            } => ("diamond", format!("{condition}?")),
            VizNodeKind::Join { depth: _, label } => (
                "circle",
                label.clone().unwrap_or_else(|| "join".to_string()),
            ),
        };
        out.push_str(&format!(
            "  {} [shape={}, label=\"{}\"];\n",
            node.id,
            shape,
            escape_dot_label(&label)
        ));
    }

    for edge in &graph.edges {
        let attrs = match edge.kind {
            VizEdgeKind::Normal => String::new(),
            VizEdgeKind::BranchYes => " [label=\"Yes\"]".to_string(),
            VizEdgeKind::BranchNo => " [label=\"No\"]".to_string(),
            VizEdgeKind::OnFail => {
                let label = edge
                    .label
                    .as_deref()
                    .map(escape_dot_label)
                    .unwrap_or_else(|| "unknown".to_string());
                format!(" [style=\"dashed\", label=\"on_fail: {label}\"]")
            }
        };
        out.push_str(&format!("  {} -> {}{};\n", edge.from, edge.to, attrs));
    }

    out.push_str("}\n");
    out
}

#[cfg(feature = "visualize-png-dot")]
pub fn render_png_with_dot(plan: &ExecutionPlan, output: &Path) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    which::which("dot").context("Graphviz 'dot' not found in PATH; install graphviz and retry")?;

    let dot_graph = to_dot(plan);
    let mut child = Command::new("dot")
        .arg("-Tpng")
        .arg("-o")
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch graphviz dot process")?;

    let mut stdin = child
        .stdin
        .take()
        .context("failed to open dot stdin pipe")?;
    stdin
        .write_all(dot_graph.as_bytes())
        .context("failed to send DOT graph to dot stdin")?;
    drop(stdin);

    let output_data = child
        .wait_with_output()
        .context("failed while waiting for dot process")?;
    if !output_data.status.success() {
        let stderr = String::from_utf8_lossy(&output_data.stderr);
        bail!("dot failed to render PNG: {stderr}");
    }

    let meta = std::fs::metadata(output)
        .with_context(|| format!("expected png output missing: {}", output.display()))?;
    if meta.len() == 0 {
        bail!("dot created an empty png file: {}", output.display());
    }

    Ok(())
}

fn escape_dot_label(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::{ExecutionPlan, FailAction, PlanStep};

    #[test]
    fn test_to_dot_contains_expected_nodes_and_edges() {
        let plan = ExecutionPlan {
            name: "dot-demo".to_string(),
            description: String::new(),
            variables: Vec::new(),
            steps: vec![
                PlanStep {
                    id: 1,
                    title: "Build".to_string(),
                    tool: Some("codex".to_string()),
                    prompt: "Build".to_string(),
                    tier: None,
                    depends_on: Vec::new(),
                    on_fail: FailAction::Abort,
                    condition: None,
                    loop_var: None,
                },
                PlanStep {
                    id: 2,
                    title: "Test".to_string(),
                    tool: None,
                    prompt: "Test".to_string(),
                    tier: None,
                    depends_on: vec![1],
                    on_fail: FailAction::Retry(2),
                    condition: Some("has_tests".to_string()),
                    loop_var: None,
                },
            ],
        };

        let dot = to_dot(&plan);
        assert!(dot.starts_with("digraph weave_plan"));
        assert!(dot.contains("S1 [shape=box, label=\"1. Build\\\\n[codex]\"]"));
        assert!(dot.contains("D1 [shape=diamond, label=\"has_tests?\"]"));
        assert!(dot.contains("D1 -> S2 [label=\"Yes\"]"));
        assert!(dot.contains("S2 -> F1 [style=\"dashed\", label=\"on_fail: retry:2\"]"));
    }
}
