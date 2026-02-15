use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::compiler::{ExecutionPlan, FailAction, PlanStep, plan_from_toml};

pub mod ascii;
pub mod dot;
pub mod mermaid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisualizeTarget {
    Ascii,
    Mermaid,
    Png(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisualizeResult {
    Stdout(String),
    FileWritten(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VizGraph {
    pub nodes: Vec<VizNode>,
    pub edges: Vec<VizEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VizNode {
    pub id: String,
    pub kind: VizNodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VizNodeKind {
    VariableHeader {
        names: Vec<String>,
    },
    Step {
        step_id: usize,
        title: String,
        tool: Option<String>,
        loop_label: Option<String>,
    },
    Decision {
        condition: String,
        depth: usize,
    },
    Join {
        depth: usize,
        label: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VizEdge {
    pub from: String,
    pub to: String,
    pub kind: VizEdgeKind,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VizEdgeKind {
    Normal,
    BranchYes,
    BranchNo,
    OnFail,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConditionAtom {
    pub expr: String,
    pub truthy: bool,
}

/// Build a neutral graph representation from the compiled execution plan.
pub fn build_graph(plan: &ExecutionPlan) -> VizGraph {
    let mut graph = VizGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
    };

    graph.nodes.push(VizNode {
        id: "ROOT".to_string(),
        kind: VizNodeKind::Join {
            depth: 0,
            label: Some("root".to_string()),
        },
    });

    let mut entry_id = "ROOT".to_string();
    if !plan.variables.is_empty() {
        graph.nodes.push(VizNode {
            id: "V".to_string(),
            kind: VizNodeKind::VariableHeader {
                names: plan.variables.iter().map(|v| v.name.clone()).collect(),
            },
        });
        graph.edges.push(VizEdge {
            from: "ROOT".to_string(),
            to: "V".to_string(),
            kind: VizEdgeKind::Normal,
            label: None,
        });
        entry_id = "V".to_string();
    }

    for step in &plan.steps {
        graph.nodes.push(VizNode {
            id: step_node_id(step.id),
            kind: VizNodeKind::Step {
                step_id: step.id,
                title: step.title.clone(),
                tool: step.tool.clone(),
                loop_label: step
                    .loop_var
                    .as_ref()
                    .map(|lv| format!("{} in {}", lv.variable, lv.collection)),
            },
        });
    }

    if let Some(first) = plan.steps.first() {
        graph.edges.push(VizEdge {
            from: entry_id,
            to: step_node_id(first.id),
            kind: VizEdgeKind::Normal,
            label: None,
        });
    }
    for pair in plan.steps.windows(2) {
        graph.edges.push(VizEdge {
            from: step_node_id(pair[0].id),
            to: step_node_id(pair[1].id),
            kind: VizEdgeKind::Normal,
            label: None,
        });
    }

    add_decision_edges(&mut graph, plan);
    add_join_nodes(&mut graph, plan);
    add_on_fail_edges(&mut graph, plan);

    dedupe_edges(&mut graph);
    graph
}

fn add_decision_edges(graph: &mut VizGraph, plan: &ExecutionPlan) {
    let mut decision_map: HashMap<String, String> = HashMap::new();
    let mut decision_count = 0usize;

    for step in &plan.steps {
        let atoms = step_condition_atoms(step);
        if atoms.is_empty() {
            continue;
        }

        let mut prefix: Vec<ConditionAtom> = Vec::new();
        let mut decision_chain: Vec<String> = Vec::new();
        for atom in &atoms {
            let key = decision_key(&prefix, &atom.expr);
            let decision_id = if let Some(existing) = decision_map.get(&key) {
                existing.clone()
            } else {
                decision_count += 1;
                let id = format!("D{decision_count}");
                graph.nodes.push(VizNode {
                    id: id.clone(),
                    kind: VizNodeKind::Decision {
                        condition: atom.expr.clone(),
                        depth: prefix.len(),
                    },
                });
                decision_map.insert(key, id.clone());
                id
            };
            decision_chain.push(decision_id);
            prefix.push(atom.clone());
        }

        if let Some(first) = decision_chain.first() {
            graph.edges.push(VizEdge {
                from: "ROOT".to_string(),
                to: first.clone(),
                kind: VizEdgeKind::Normal,
                label: None,
            });
        }

        for idx in 1..decision_chain.len() {
            let prev = &decision_chain[idx - 1];
            let next = &decision_chain[idx];
            let prev_atom = &atoms[idx - 1];
            graph.edges.push(VizEdge {
                from: prev.clone(),
                to: next.clone(),
                kind: branch_kind(prev_atom.truthy),
                label: None,
            });
        }

        if let (Some(last_decision), Some(last_atom)) = (decision_chain.last(), atoms.last()) {
            graph.edges.push(VizEdge {
                from: last_decision.clone(),
                to: step_node_id(step.id),
                kind: branch_kind(last_atom.truthy),
                label: None,
            });
        }
    }
}

fn add_join_nodes(graph: &mut VizGraph, plan: &ExecutionPlan) {
    if plan.steps.len() < 2 {
        return;
    }

    let mut join_index = 0usize;
    let mut prev_atoms = step_condition_atoms(&plan.steps[0]);
    for pair in plan.steps.windows(2) {
        let current_atoms = step_condition_atoms(&pair[1]);
        let common = common_prefix_len(&prev_atoms, &current_atoms);
        if !prev_atoms.is_empty() && current_atoms.len() < prev_atoms.len() {
            join_index += 1;
            let join_id = format!("J{join_index}");
            graph.nodes.push(VizNode {
                id: join_id.clone(),
                kind: VizNodeKind::Join {
                    depth: common,
                    label: Some("join".to_string()),
                },
            });
            graph.edges.push(VizEdge {
                from: step_node_id(pair[0].id),
                to: join_id.clone(),
                kind: VizEdgeKind::Normal,
                label: None,
            });
            graph.edges.push(VizEdge {
                from: join_id,
                to: step_node_id(pair[1].id),
                kind: VizEdgeKind::Normal,
                label: None,
            });
        }
        prev_atoms = current_atoms;
    }
}

fn add_on_fail_edges(graph: &mut VizGraph, plan: &ExecutionPlan) {
    let mut fail_join_counter = 0usize;
    for step in &plan.steps {
        if matches!(step.on_fail, FailAction::Abort) {
            continue;
        }

        fail_join_counter += 1;
        let fail_node_id = format!("F{fail_join_counter}");
        graph.nodes.push(VizNode {
            id: fail_node_id.clone(),
            kind: VizNodeKind::Join {
                depth: 0,
                label: Some("on_fail".to_string()),
            },
        });
        graph.edges.push(VizEdge {
            from: step_node_id(step.id),
            to: fail_node_id,
            kind: VizEdgeKind::OnFail,
            label: Some(format_fail_action(&step.on_fail)),
        });
    }
}

fn dedupe_edges(graph: &mut VizGraph) {
    let mut deduped: Vec<VizEdge> = Vec::with_capacity(graph.edges.len());
    for edge in &graph.edges {
        if !deduped.iter().any(|existing| existing == edge) {
            deduped.push(edge.clone());
        }
    }
    graph.edges = deduped;
}

fn step_node_id(step_id: usize) -> String {
    format!("S{step_id}")
}

fn branch_kind(truthy: bool) -> VizEdgeKind {
    if truthy {
        VizEdgeKind::BranchYes
    } else {
        VizEdgeKind::BranchNo
    }
}

fn decision_key(prefix: &[ConditionAtom], expr: &str) -> String {
    if prefix.is_empty() {
        return format!("root::{expr}");
    }
    let prefix_key = prefix
        .iter()
        .map(|atom| {
            if atom.truthy {
                format!("+{}", atom.expr)
            } else {
                format!("-{}", atom.expr)
            }
        })
        .collect::<Vec<_>>()
        .join("&&");
    format!("{prefix_key}::{expr}")
}

pub fn split_top_level_and(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut depth = 0i32;

    while let Some(ch) = chars.next() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(ch);
            }
            '&' => {
                if depth == 0 && chars.peek() == Some(&'&') {
                    chars.next();
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                    current.clear();
                } else {
                    current.push(ch);
                }
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

fn strip_wrapping_parens(input: &str) -> &str {
    let mut s = input.trim();
    loop {
        if !s.starts_with('(') || !s.ends_with(')') {
            return s;
        }
        let mut depth = 0i32;
        let mut wraps_entire = true;
        for (idx, ch) in s.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && idx != s.len() - 1 {
                        wraps_entire = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if wraps_entire && depth == 0 {
            s = &s[1..s.len() - 1];
            s = s.trim();
            continue;
        }
        return s;
    }
}

fn parse_condition_atoms(condition: &str) -> Vec<ConditionAtom> {
    split_top_level_and(condition)
        .into_iter()
        .map(|raw| {
            let trimmed = strip_wrapping_parens(&raw);
            if let Some(rest) = trimmed.strip_prefix('!') {
                ConditionAtom {
                    expr: strip_wrapping_parens(rest).to_string(),
                    truthy: false,
                }
            } else {
                ConditionAtom {
                    expr: trimmed.to_string(),
                    truthy: true,
                }
            }
        })
        .filter(|atom| !atom.expr.is_empty())
        .collect()
}

pub(crate) fn step_condition_atoms(step: &PlanStep) -> Vec<ConditionAtom> {
    step.condition
        .as_deref()
        .map(parse_condition_atoms)
        .unwrap_or_default()
}

pub(crate) fn common_prefix_len(left: &[ConditionAtom], right: &[ConditionAtom]) -> usize {
    left.iter()
        .zip(right.iter())
        .take_while(|(l, r)| l == r)
        .count()
}

pub(crate) fn format_fail_action(action: &FailAction) -> String {
    match action {
        FailAction::Abort => "abort".to_string(),
        FailAction::Retry(n) => format!("retry:{n}"),
        FailAction::Skip => "skip".to_string(),
        FailAction::Delegate(target) => format!("delegate:{target}"),
    }
}

/// Render an execution plan as a minimal ASCII representation.
pub fn render_ascii(plan: &ExecutionPlan) -> String {
    ascii::render_ascii(plan)
}

/// Render an execution plan as Mermaid flowchart text.
pub fn render_mermaid(plan: &ExecutionPlan) -> String {
    mermaid::render_mermaid(plan)
}

/// Render an execution plan as PNG.
pub fn render_png(plan: &ExecutionPlan, output: &Path) -> Result<()> {
    #[cfg(feature = "visualize-png-dot")]
    {
        dot::render_png_with_dot(plan, output)
    }

    #[cfg(not(feature = "visualize-png-dot"))]
    {
        let _ = (plan, output);
        anyhow::bail!(
            "PNG output requires the `visualize-png-dot` feature and Graphviz `dot` in PATH"
        )
    }
}

/// Load a plan TOML file and render it to the requested target.
pub fn visualize_plan_file(plan_path: &Path, target: VisualizeTarget) -> Result<VisualizeResult> {
    let content = std::fs::read_to_string(plan_path)
        .with_context(|| format!("failed to read {}", plan_path.display()))?;
    let plan = plan_from_toml(&content)
        .with_context(|| format!("failed to parse {}", plan_path.display()))?;

    match target {
        VisualizeTarget::Ascii => Ok(VisualizeResult::Stdout(render_ascii(&plan))),
        VisualizeTarget::Mermaid => Ok(VisualizeResult::Stdout(render_mermaid(&plan))),
        VisualizeTarget::Png(output) => {
            render_png(&plan, &output)?;
            Ok(VisualizeResult::FileWritten(output))
        }
    }
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

    fn find_decision_node<'a>(graph: &'a VizGraph, condition: &str) -> Option<&'a VizNode> {
        graph.nodes.iter().find(|node| {
            matches!(
                &node.kind,
                VizNodeKind::Decision {
                    condition: c,
                    depth: _
                } if c == condition
            )
        })
    }

    #[test]
    fn test_build_graph_linear_plan() {
        let plan = compile_doc(
            r#"---
name = "linear"
---
## Build
Tool: codex
Build project.

## Test
Tool: claude-code
Run tests.
"#,
        );
        let graph = build_graph(&plan);

        assert!(
            graph
                .nodes
                .iter()
                .all(|n| !matches!(n.kind, VizNodeKind::Decision { .. }))
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.from == "S1" && e.to == "S2" && e.kind == VizEdgeKind::Normal)
        );
    }

    #[test]
    fn test_build_graph_single_if_else() {
        let plan = compile_doc(
            r#"---
name = "if-else"
---
## IF has_tests
## Run Tests
Tool: codex
Run unit tests.
## ELSE
## Skip Tests
No tests available.
## ENDIF
"#,
        );
        let graph = build_graph(&plan);

        let decision = find_decision_node(&graph, "has_tests").expect("missing decision");
        assert!(graph.edges.iter().any(|e| {
            e.from == decision.id && e.to == "S1" && e.kind == VizEdgeKind::BranchYes
        }));
        assert!(
            graph.edges.iter().any(|e| {
                e.from == decision.id && e.to == "S2" && e.kind == VizEdgeKind::BranchNo
            })
        );
    }

    #[test]
    fn test_build_graph_nested_if_in_else() {
        let plan = compile_doc(
            r#"---
name = "nested"
---
## IF ${USER_APPROVES}
## Apply Plan
Proceed.
## ELSE
## IF ${USER_MODIFIES}
## Resume
Resume with edits.
## ELSE
## Abandon
Stop.
## ENDIF
## ENDIF
"#,
        );
        let graph = build_graph(&plan);

        let outer = find_decision_node(&graph, "${USER_APPROVES}").expect("missing outer");
        let inner = find_decision_node(&graph, "${USER_MODIFIES}").expect("missing inner");

        assert!(graph.edges.iter().any(|e| {
            e.from == outer.id && e.to == inner.id && e.kind == VizEdgeKind::BranchNo
        }));
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.from == inner.id && e.to == "S2" && e.kind == VizEdgeKind::BranchYes)
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.from == inner.id && e.to == "S3" && e.kind == VizEdgeKind::BranchNo)
        );
    }

    #[test]
    fn test_build_graph_if_inside_for_preserves_loop_labels() {
        let plan = compile_doc(
            r#"---
name = "if-for"
---
## FOR item IN ${ITEMS}
## IF ${IS_VALID}
## Process Item
Tool: codex
OnFail: retry 2
Process ${item}.
## ELSE
## Skip Item
OnFail: skip
Skip ${item}.
## ENDIF
## ENDFOR
"#,
        );

        let graph = build_graph(&plan);
        let process = graph
            .nodes
            .iter()
            .find(|node| {
                matches!(
                    node.kind,
                    VizNodeKind::Step {
                        step_id: 1,
                        title: _,
                        tool: _,
                        loop_label: _
                    }
                )
            })
            .expect("missing step");

        match &process.kind {
            VizNodeKind::Step { loop_label, .. } => {
                assert_eq!(loop_label.as_deref(), Some("item in ${ITEMS}"));
            }
            _ => unreachable!("unexpected node kind"),
        }

        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == VizEdgeKind::OnFail && e.from == "S1")
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == VizEdgeKind::OnFail && e.from == "S2")
        );
    }
}
