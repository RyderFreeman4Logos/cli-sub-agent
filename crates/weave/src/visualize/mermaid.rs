use crate::compiler::ExecutionPlan;

use super::{VizEdgeKind, VizNodeKind, build_graph};

pub fn render_mermaid(plan: &ExecutionPlan) -> String {
    let graph = build_graph(plan);
    let mut lines = vec!["flowchart TD".to_string()];

    for node in &graph.nodes {
        match &node.kind {
            VizNodeKind::VariableHeader { names } => {
                let label = escape_label(&format!("Variables: {}", names.join(", ")));
                lines.push(format!("  {}[\"{}\"]", node.id, label));
            }
            VizNodeKind::Step {
                step_id,
                title,
                tool,
                loop_label: _,
            } => {
                let tool = tool.as_deref().unwrap_or("none");
                let label = escape_label(&format!("{step_id}. {title}\\n[{tool}]"));
                lines.push(format!("  {}[\"{}\"]", node.id, label));
            }
            VizNodeKind::Decision {
                condition,
                depth: _,
            } => {
                let label = escape_label(&format!("{condition}?"));
                lines.push(format!("  {}{{\"{}\"}}", node.id, label));
            }
            VizNodeKind::Join { depth: _, label } => {
                if node.id == "ROOT" {
                    continue;
                }
                let title = label.as_deref().unwrap_or("join");
                let escaped = escape_label(title);
                lines.push(format!("  {}((\"{}\"))", node.id, escaped));
            }
        }
    }

    for edge in &graph.edges {
        if edge.from == "ROOT" || edge.to == "ROOT" {
            continue;
        }

        let edge_text = match edge.kind {
            VizEdgeKind::Normal => format!("  {} --> {}", edge.from, edge.to),
            VizEdgeKind::BranchYes => format!("  {} -->|Yes| {}", edge.from, edge.to),
            VizEdgeKind::BranchNo => format!("  {} -->|No| {}", edge.from, edge.to),
            VizEdgeKind::OnFail => {
                let label = edge
                    .label
                    .as_deref()
                    .map(escape_label)
                    .unwrap_or_else(|| "unknown".to_string());
                format!("  {} -->|on_fail: {}| {}", edge.from, label, edge.to)
            }
        };
        lines.push(edge_text);
    }

    lines.join("\n")
}

fn escape_label(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('|', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::compile;
    use crate::parser::parse_skill;

    fn compile_doc(markdown: &str) -> ExecutionPlan {
        let doc = parse_skill(markdown).expect("parse should succeed");
        compile(&doc).expect("compile should succeed")
    }

    #[test]
    fn test_render_mermaid_contains_expected_nodes_and_edges() {
        let plan = compile_doc(
            r#"---
name = "mermaid-demo"
---
## Build
Tool: codex
Build project.

## IF has_tests
## Test
Tool: claude-code
OnFail: retry 2
Run tests.
## ELSE
## Skip Tests
OnFail: skip
No tests available.
## ENDIF
"#,
        );

        let output = render_mermaid(&plan);
        assert!(output.starts_with("flowchart TD"));
        assert!(output.contains("S1[\"1. Build\\\\n[codex]\"]"));
        assert!(output.contains("S2[\"2. Test\\\\n[claude-code]\"]"));
        assert!(output.contains("D1{\"has_tests?\"}"));
        assert!(output.contains("D1 -->|Yes| S2"));
        assert!(output.contains("D1 -->|No| S3"));
        assert!(output.contains("S2 -->|on_fail: retry:2|"));
    }

    #[test]
    fn test_render_mermaid_escapes_unsafe_label_chars() {
        let plan = ExecutionPlan {
            name: "escape".to_string(),
            description: String::new(),
            variables: Vec::new(),
            steps: vec![crate::compiler::PlanStep {
                id: 1,
                title: "Say \"hi\" | greet".to_string(),
                tool: Some("co\\dex".to_string()),
                prompt: String::new(),
                tier: None,
                depends_on: Vec::new(),
                on_fail: crate::compiler::FailAction::Abort,
                condition: None,
                loop_var: None,
            }],
        };

        let output = render_mermaid(&plan);
        assert!(output.contains("Say \\\"hi\\\" / greet"));
        assert!(output.contains("[co\\\\dex]"));
    }
}
