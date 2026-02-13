use anyhow::{Result, anyhow, bail};
use std::collections::{BTreeSet, HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyNode {
    pub title: String,
    pub is_done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DependencyGraph {
    nodes: Vec<DependencyNode>,
    edges: Vec<Vec<usize>>,
    incoming: Vec<Vec<usize>>,
}

impl DependencyGraph {
    /// Build a dependency graph from TODO markdown content.
    ///
    /// Supported dependency formats:
    /// - Inline annotation: `- [ ] Task B (depends: Task A, Task C)`
    /// - `## Dependencies` section with `Task A -> Task B` lines
    pub fn from_markdown(markdown: &str) -> Result<Self> {
        let mut parsed_nodes: Vec<ParsedNode> = Vec::new();
        let mut section_dependencies: Vec<(String, String)> = Vec::new();
        let mut in_dependencies_section = false;

        for raw_line in markdown.lines() {
            let line = raw_line.trim();

            if let Some(heading) = parse_heading(line) {
                in_dependencies_section = heading.eq_ignore_ascii_case("dependencies");
                continue;
            }

            if in_dependencies_section {
                if let Some((from, to)) = parse_dependency_relation(line) {
                    section_dependencies.push((from, to));
                }
                continue;
            }

            if let Some((is_done, raw_title)) = parse_checkbox_item(line) {
                let (title, inline_dependencies) = split_inline_dependencies(&raw_title);
                if title.is_empty() {
                    continue;
                }

                parsed_nodes.push(ParsedNode {
                    title,
                    is_done,
                    inline_dependencies,
                });
            }
        }

        let nodes: Vec<DependencyNode> = parsed_nodes
            .iter()
            .map(|n| DependencyNode {
                title: n.title.clone(),
                is_done: n.is_done,
            })
            .collect();

        let mut title_to_indices: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            title_to_indices
                .entry(normalize_reference(&node.title))
                .or_default()
                .push(index);
        }

        let mut edge_set: BTreeSet<(usize, usize)> = BTreeSet::new();

        for (to_index, parsed) in parsed_nodes.iter().enumerate() {
            for dependency in &parsed.inline_dependencies {
                let from_index = resolve_reference(dependency, &title_to_indices)?;
                edge_set.insert((from_index, to_index));
            }
        }

        for (from_ref, to_ref) in section_dependencies {
            let from_index = resolve_reference(&from_ref, &title_to_indices)?;
            let to_index = resolve_reference(&to_ref, &title_to_indices)?;
            edge_set.insert((from_index, to_index));
        }

        let mut edges = vec![Vec::new(); nodes.len()];
        let mut incoming = vec![Vec::new(); nodes.len()];
        for (from, to) in edge_set {
            edges[from].push(to);
            incoming[to].push(from);
        }

        Ok(Self {
            nodes,
            edges,
            incoming,
        })
    }

    pub fn nodes(&self) -> &[DependencyNode] {
        &self.nodes
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.iter().map(std::vec::Vec::len).sum()
    }

    /// Detect a cycle using a BFS-style topological reduction (Kahn's algorithm).
    /// Returns the titles still carrying in-degree after traversal.
    pub fn cycle_nodes_bfs(&self) -> Option<Vec<String>> {
        let mut indegree: Vec<usize> = self.incoming.iter().map(std::vec::Vec::len).collect();
        let mut queue: VecDeque<usize> = indegree
            .iter()
            .enumerate()
            .filter_map(|(index, degree)| (*degree == 0).then_some(index))
            .collect();
        let mut visited = 0usize;

        while let Some(node) = queue.pop_front() {
            visited += 1;
            for &next in &self.edges[node] {
                indegree[next] = indegree[next].saturating_sub(1);
                if indegree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }

        if visited == self.nodes.len() {
            None
        } else {
            Some(
                indegree
                    .iter()
                    .enumerate()
                    .filter_map(|(index, degree)| {
                        (*degree > 0).then_some(self.nodes[index].title.clone())
                    })
                    .collect(),
            )
        }
    }

    pub fn has_cycle_bfs(&self) -> bool {
        self.cycle_nodes_bfs().is_some()
    }

    /// Return a topological execution order. Errors if the graph has cycles.
    pub fn topological_sort(&self) -> Result<Vec<usize>> {
        let mut indegree: Vec<usize> = self.incoming.iter().map(std::vec::Vec::len).collect();
        let mut queue: VecDeque<usize> = indegree
            .iter()
            .enumerate()
            .filter_map(|(index, degree)| (*degree == 0).then_some(index))
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());

        while let Some(node) = queue.pop_front() {
            order.push(node);
            for &next in &self.edges[node] {
                indegree[next] = indegree[next].saturating_sub(1);
                if indegree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }

        if order.len() == self.nodes.len() {
            Ok(order)
        } else {
            let cycle = self
                .cycle_nodes_bfs()
                .unwrap_or_else(|| vec!["unknown".to_string()]);
            bail!("Dependency cycle detected: {}", cycle.join(" -> "));
        }
    }

    pub fn to_mermaid(&self) -> String {
        let mut output = String::from("graph TD\n");

        for (index, node) in self.nodes.iter().enumerate() {
            output.push_str(&format!(
                "  N{index}[\"{}\"]\n",
                escape_mermaid_label(&node.title)
            ));
        }

        for (from, children) in self.edges.iter().enumerate() {
            for to in children {
                output.push_str(&format!("  N{from} --> N{to}\n"));
            }
        }

        output
    }

    pub fn to_dot(&self) -> String {
        let mut output = String::from("digraph TODO {\n  rankdir=LR;\n");

        for (index, node) in self.nodes.iter().enumerate() {
            output.push_str(&format!(
                "  n{index} [label=\"{}\"];\n",
                escape_dot_label(&node.title)
            ));
        }

        for (from, children) in self.edges.iter().enumerate() {
            for to in children {
                output.push_str(&format!("  n{from} -> n{to};\n"));
            }
        }

        output.push_str("}\n");
        output
    }

    /// Render dependency trees from all roots (in-degree 0) to leaf tasks.
    pub fn to_terminal(&self) -> String {
        if self.nodes.is_empty() {
            return String::new();
        }

        let mut roots: Vec<usize> = self
            .incoming
            .iter()
            .enumerate()
            .filter_map(|(index, incoming)| incoming.is_empty().then_some(index))
            .collect();

        if roots.is_empty() {
            roots = (0..self.nodes.len()).collect();
        }

        let mut lines = Vec::new();
        for root in roots {
            self.render_terminal_node(root, "", true, &mut Vec::new(), &mut lines);
        }
        lines.join("\n")
    }

    fn render_terminal_node(
        &self,
        node_index: usize,
        prefix: &str,
        is_last: bool,
        path: &mut Vec<usize>,
        lines: &mut Vec<String>,
    ) {
        let done_suffix = if self.nodes[node_index].is_done {
            " \u{2713}"
        } else {
            ""
        };

        if path.is_empty() {
            lines.push(format!("{}{}", self.nodes[node_index].title, done_suffix));
        } else {
            let branch = if is_last { "└── " } else { "├── " };
            lines.push(format!(
                "{prefix}{branch}{}{}",
                self.nodes[node_index].title, done_suffix
            ));
        }

        if path.contains(&node_index) {
            return;
        }

        path.push(node_index);
        let children = &self.edges[node_index];
        for (index, child) in children.iter().enumerate() {
            let next_prefix = if path.len() == 1 {
                String::new()
            } else {
                let suffix = if is_last { "    " } else { "│   " };
                format!("{prefix}{suffix}")
            };
            self.render_terminal_node(
                *child,
                &next_prefix,
                index + 1 == children.len(),
                path,
                lines,
            );
        }
        let _ = path.pop();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedNode {
    title: String,
    is_done: bool,
    inline_dependencies: Vec<String>,
}

fn parse_heading(line: &str) -> Option<&str> {
    let hash_count = line.chars().take_while(|c| *c == '#').count();
    if hash_count == 0 {
        return None;
    }

    let rest = line.get(hash_count..)?.trim();
    if rest.is_empty() { None } else { Some(rest) }
}

fn parse_checkbox_item(line: &str) -> Option<(bool, String)> {
    let mut rest = line.trim_start();
    rest = rest
        .strip_prefix('-')
        .or_else(|| rest.strip_prefix('*'))
        .map(str::trim_start)?;
    rest = rest.strip_prefix('[')?;

    let marker = rest.chars().next()?;
    let is_done = match marker {
        'x' | 'X' => true,
        ' ' => false,
        _ => return None,
    };

    rest = rest.get(marker.len_utf8()..)?;
    rest = rest.strip_prefix(']')?;

    let title = rest.trim_start();
    if title.is_empty() {
        return None;
    }

    Some((is_done, title.to_string()))
}

fn split_inline_dependencies(title: &str) -> (String, Vec<String>) {
    let lower = title.to_ascii_lowercase();
    let Some(start) = lower.rfind("(depends:") else {
        return (title.trim().to_string(), Vec::new());
    };

    if !title.ends_with(')') {
        return (title.trim().to_string(), Vec::new());
    }

    let dep_start = start + "(depends:".len();
    if dep_start >= title.len() - 1 {
        return (title.trim().to_string(), Vec::new());
    }

    let dependencies_raw = &title[dep_start..title.len() - 1];
    let dependencies: Vec<String> = dependencies_raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if dependencies.is_empty() {
        return (title.trim().to_string(), Vec::new());
    }

    let clean_title = title[..start].trim_end().to_string();
    if clean_title.is_empty() {
        return (title.trim().to_string(), Vec::new());
    }

    (clean_title, dependencies)
}

fn parse_dependency_relation(line: &str) -> Option<(String, String)> {
    if line.is_empty() {
        return None;
    }

    let relation = line
        .strip_prefix('-')
        .or_else(|| line.strip_prefix('*'))
        .map(str::trim_start)
        .unwrap_or(line);
    let (from, to) = relation.split_once("->")?;
    let from = from.trim();
    let to = to.trim();
    if from.is_empty() || to.is_empty() {
        return None;
    }
    Some((from.to_string(), to.to_string()))
}

fn normalize_reference(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn resolve_reference(
    reference: &str,
    title_to_indices: &HashMap<String, Vec<usize>>,
) -> Result<usize> {
    let normalized = normalize_reference(reference);
    let indices = title_to_indices
        .get(&normalized)
        .ok_or_else(|| anyhow!("Unknown dependency reference: '{reference}'"))?;

    match indices.as_slice() {
        [index] => Ok(*index),
        _ => bail!("Ambiguous dependency reference: '{reference}'"),
    }
}

fn escape_mermaid_label(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_dot_label(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_inline_dependencies_and_topological_order() {
        let markdown = r#"
# TODO

- [x] Migrate to edition 2024
- [x] Fix UTF-8 truncation (depends: Migrate to edition 2024)
- [x] Fix session resume (depends: Migrate to edition 2024)
- [ ] Integrate agent-teams (depends: Fix UTF-8 truncation, Fix session resume)
"#;

        let graph = DependencyGraph::from_markdown(markdown).unwrap();
        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 4);
        assert!(!graph.has_cycle_bfs());

        let order = graph.topological_sort().unwrap();
        let mut position = HashMap::new();
        for (rank, node_index) in order.iter().enumerate() {
            position.insert(graph.nodes()[*node_index].title.clone(), rank);
        }

        assert!(
            position["Migrate to edition 2024"] < position["Fix UTF-8 truncation"],
            "migrate should run before utf-8 fix"
        );
        assert!(
            position["Migrate to edition 2024"] < position["Fix session resume"],
            "migrate should run before session fix"
        );
        assert!(
            position["Fix UTF-8 truncation"] < position["Integrate agent-teams"],
            "utf-8 fix should run before integration"
        );
        assert!(
            position["Fix session resume"] < position["Integrate agent-teams"],
            "session fix should run before integration"
        );
    }

    #[test]
    fn parse_dependencies_section_edges() {
        let markdown = r#"
# TODO

- [ ] Task A
- [ ] Task B
- [ ] Task C

## Dependencies
- Task A -> Task B
- Task B -> Task C
"#;

        let graph = DependencyGraph::from_markdown(markdown).unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);

        let mermaid = graph.to_mermaid();
        assert!(mermaid.contains("graph TD"));
        assert!(mermaid.contains("N0 --> N1"));
        assert!(mermaid.contains("N1 --> N2"));
    }

    #[test]
    fn detect_cycle_with_bfs() {
        let markdown = r#"
- [ ] Task A (depends: Task B)
- [ ] Task B (depends: Task A)
"#;

        let graph = DependencyGraph::from_markdown(markdown).unwrap();
        assert!(graph.has_cycle_bfs());

        let cycle_nodes = graph.cycle_nodes_bfs().unwrap();
        assert_eq!(cycle_nodes.len(), 2);
        assert!(cycle_nodes.iter().any(|n| n == "Task A"));
        assert!(cycle_nodes.iter().any(|n| n == "Task B"));
        assert!(graph.topological_sort().is_err());
    }

    #[test]
    fn terminal_output_includes_tree_and_status() {
        let markdown = r#"
- [x] [P0] Migrate to edition 2024
- [x] [P0] Fix UTF-8 truncation (depends: [P0] Migrate to edition 2024)
- [x] [P0] Fix session resume (depends: [P0] Migrate to edition 2024)
- [ ] [1] Integrate agent-teams (depends: [P0] Fix UTF-8 truncation, [P0] Fix session resume)
"#;

        let graph = DependencyGraph::from_markdown(markdown).unwrap();
        let terminal = graph.to_terminal();

        assert!(terminal.contains("[P0] Migrate to edition 2024 ✓"));
        assert!(terminal.contains("├── [P0] Fix UTF-8 truncation ✓"));
        assert!(terminal.contains("└── [P0] Fix session resume ✓"));
        assert!(terminal.contains("[1] Integrate agent-teams"));
    }

    #[test]
    fn dot_output_contains_nodes_and_edges() {
        let markdown = r#"
- [ ] Task A
- [ ] Task B (depends: Task A)
"#;

        let graph = DependencyGraph::from_markdown(markdown).unwrap();
        let dot = graph.to_dot();

        assert!(dot.starts_with("digraph TODO"));
        assert!(dot.contains("n0 [label=\"Task A\"];"));
        assert!(dot.contains("n0 -> n1;"));
    }

    #[test]
    fn unknown_dependency_reference_returns_error() {
        let markdown = r#"
- [ ] Task B (depends: Task A)
"#;

        let err = DependencyGraph::from_markdown(markdown).unwrap_err();
        assert!(err.to_string().contains("Unknown dependency reference"));
    }
}
