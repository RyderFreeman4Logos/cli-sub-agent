// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
#![allow(dead_code)]
//! CLI subcommand for token estimation via tokuin.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use std::path::PathBuf;
use tree_sitter::{Node, Parser};

const DEFAULT_MODEL: &str = "gpt-4o";
const DEFAULT_AST_BUDGET: usize = 8_000;
const RUST_AST_NODE_KINDS: &[&str] = &[
    "function_item",
    "impl_item",
    "struct_item",
    "enum_item",
    "mod_item",
];

#[derive(Subcommand)]
pub enum TokuinCommands {
    /// Estimate token count for one or more files
    Estimate {
        /// Paths to files to estimate
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Model for OpenAI BPE tokenizer (conservative: returns max of BPE + chars/3)
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Token budget threshold — exit non-zero if any file exceeds this
        #[arg(long)]
        budget: Option<usize>,

        /// Enable Rust tree-sitter AST-aware token breakdown
        #[arg(long)]
        ast: bool,
    },
    /// List supported model names for the OpenAI BPE tokenizer
    Models,
}

pub fn handle_tokuin(cmd: TokuinCommands) -> Result<()> {
    match cmd {
        TokuinCommands::Estimate {
            files,
            model,
            json,
            budget,
            ast,
        } => handle_estimate(files, model, json, budget, ast),
        TokuinCommands::Models => handle_models(),
    }
}

#[derive(Debug, Serialize)]
struct EstimateFileOutput {
    file: String,
    tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    ast_breakdown: Option<AstBreakdown>,
}

#[derive(Debug, Serialize)]
struct AstBreakdown {
    language: Option<&'static str>,
    total_tokens: usize,
    budget: usize,
    nodes: Vec<AstNodeBreakdown>,
    split_suggestions: Vec<AstSplitSuggestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

#[derive(Debug, Serialize)]
struct AstNodeBreakdown {
    kind: String,
    name: String,
    tokens: usize,
    start_line: usize,
    end_line: usize,
    start_byte: usize,
    end_byte: usize,
    children: Vec<AstNodeBreakdown>,
}

#[derive(Debug, Serialize)]
struct AstSplitSuggestion {
    after_node: String,
    line: usize,
    accumulated_tokens: usize,
    reason: String,
}

fn resolve_model(model: &str) -> &str {
    match model {
        "claude" | "opus" | "sonnet" | "haiku" | "claude-4" => "gpt-4o",
        "codex" | "gpt-5" | "gpt-5.5" => "gpt-4o",
        other => other,
    }
}

fn handle_estimate(
    files: Vec<PathBuf>,
    model: String,
    json: bool,
    budget: Option<usize>,
    ast: bool,
) -> Result<()> {
    use tokuin::tokenizers::{ConservativeTokenizer, Tokenizer};

    let resolved = resolve_model(&model);
    let tokenizer = ConservativeTokenizer::new(resolved).map_err(|e| {
        anyhow::anyhow!(
            "Failed to create tokenizer for model '{model}' (resolved: '{resolved}'): {e}"
        )
    })?;

    let mut results = Vec::new();
    let mut exceeded = false;
    let ast_budget = budget.unwrap_or(DEFAULT_AST_BUDGET);

    for file in &files {
        let content = std::fs::read_to_string(file)
            .with_context(|| format!("Failed to read file: {}", file.display()))?;

        let tokens = tokenizer
            .count_tokens(&content)
            .map_err(|e| anyhow::anyhow!("Failed to count tokens for {}: {e}", file.display()))?;

        if let Some(limit) = budget
            && tokens > limit
        {
            exceeded = true;
        }

        let ast_breakdown = ast
            .then(|| build_ast_breakdown(file, &content, tokens, ast_budget, &tokenizer))
            .transpose()?;

        if let Some(AstBreakdown {
            warning: Some(warning),
            ..
        }) = &ast_breakdown
        {
            eprintln!("{}: {warning}", file.display());
        }

        results.push(EstimateFileOutput {
            file: file.display().to_string(),
            tokens,
            ast_breakdown,
        });
    }

    if json {
        if results.len() == 1 {
            let result = &results[0];
            let mut obj = serde_json::json!({
                "file": result.file,
                "model": resolved,
                "tokenizer": "conservative",
                "tokens": result.tokens,
            });
            if let Some(ast_breakdown) = &result.ast_breakdown {
                obj["ast_breakdown"] = serde_json::to_value(ast_breakdown)?;
            }
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            let arr: Vec<_> = results
                .iter()
                .map(|result| {
                    let mut obj = serde_json::json!({
                        "file": result.file,
                        "tokens": result.tokens,
                    });
                    if let Some(ast_breakdown) = &result.ast_breakdown {
                        obj["ast_breakdown"] = serde_json::to_value(ast_breakdown)?;
                    }
                    Ok::<_, serde_json::Error>(obj)
                })
                .collect::<std::result::Result<_, _>>()?;
            let obj = serde_json::json!({
                "model": resolved,
                "tokenizer": "conservative",
                "files": arr,
                "total": results.iter().map(|result| result.tokens).sum::<usize>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
    } else {
        for result in &results {
            if results.len() == 1 {
                println!("{}", result.tokens);
            } else {
                let marker = budget
                    .filter(|&limit| result.tokens > limit)
                    .map(|_| " OVER")
                    .unwrap_or("");
                println!("{}\t{}{}", result.tokens, result.file, marker);
            }

            if let Some(breakdown) = &result.ast_breakdown {
                print_ast_breakdown(breakdown);
            }
        }
    }

    if exceeded {
        std::process::exit(1);
    }

    Ok(())
}

fn build_ast_breakdown<T: tokuin::tokenizers::Tokenizer>(
    file: &std::path::Path,
    content: &str,
    total_tokens: usize,
    budget: usize,
    tokenizer: &T,
) -> Result<AstBreakdown> {
    if file.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return Ok(AstBreakdown {
            language: None,
            total_tokens,
            budget,
            nodes: Vec::new(),
            split_suggestions: Vec::new(),
            warning: Some(
                "AST mode currently supports only Rust files; used flat estimation".into(),
            ),
        });
    }

    let mut parser = Parser::new();
    let language = tree_sitter_rust::LANGUAGE;
    parser
        .set_language(&language.into())
        .context("Failed to load tree-sitter Rust grammar")?;

    let tree = parser
        .parse(content, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Rust source"))?;
    let root = tree.root_node();
    let nodes = collect_ast_children(root, content, tokenizer)?;
    let split_suggestions = suggest_split_points(root, content, total_tokens, budget, tokenizer)?;

    Ok(AstBreakdown {
        language: Some("rust"),
        total_tokens,
        budget,
        nodes,
        split_suggestions,
        warning: root.has_error().then(|| {
            "tree-sitter parsed Rust source with syntax errors; AST ranges may be approximate"
                .into()
        }),
    })
}

fn collect_ast_children<T: tokuin::tokenizers::Tokenizer>(
    parent: Node<'_>,
    content: &str,
    tokenizer: &T,
) -> Result<Vec<AstNodeBreakdown>> {
    let mut cursor = parent.walk();
    let mut children = Vec::new();
    for child in parent.children(&mut cursor) {
        if is_ast_breakdown_node(child) {
            children.push(build_ast_node(child, content, tokenizer)?);
        } else {
            children.extend(collect_ast_children(child, content, tokenizer)?);
        }
    }
    children.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    Ok(children)
}

fn build_ast_node<T: tokuin::tokenizers::Tokenizer>(
    node: Node<'_>,
    content: &str,
    tokenizer: &T,
) -> Result<AstNodeBreakdown> {
    let snippet = &content[node.start_byte()..node.end_byte()];
    let tokens = tokenizer
        .count_tokens(snippet)
        .map_err(|e| anyhow::anyhow!("Failed to count tokens for AST node: {e}"))?;
    let children = collect_ast_children(node, content, tokenizer)?;

    Ok(AstNodeBreakdown {
        kind: node.kind().to_string(),
        name: node_name(node, content),
        tokens,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        children,
    })
}

fn suggest_split_points<T: tokuin::tokenizers::Tokenizer>(
    root: Node<'_>,
    content: &str,
    total_tokens: usize,
    budget: usize,
    tokenizer: &T,
) -> Result<Vec<AstSplitSuggestion>> {
    if total_tokens <= budget {
        return Ok(Vec::new());
    }

    let mut top_level_nodes = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if is_ast_breakdown_node(child) {
            top_level_nodes.push(child);
        }
    }

    let mut suggestions = Vec::new();
    let mut accumulated = 0usize;
    for node in top_level_nodes {
        let snippet = &content[node.start_byte()..node.end_byte()];
        let tokens = tokenizer
            .count_tokens(snippet)
            .map_err(|e| anyhow::anyhow!("Failed to count tokens for split suggestion: {e}"))?;
        accumulated = accumulated.saturating_add(tokens);
        if accumulated >= budget {
            suggestions.push(AstSplitSuggestion {
                after_node: format!("{} {}", node.kind(), node_name(node, content)),
                line: node.end_position().row + 1,
                accumulated_tokens: accumulated,
                reason: format!(
                    "Split after this top-level item to keep chunks near the {budget}-token budget"
                ),
            });
            accumulated = 0;
        }
    }

    Ok(suggestions)
}

fn is_ast_breakdown_node(node: Node<'_>) -> bool {
    RUST_AST_NODE_KINDS.contains(&node.kind())
}

fn node_name(node: Node<'_>, content: &str) -> String {
    match node.kind() {
        "impl_item" => impl_label(node, content),
        _ => node
            .child_by_field_name("name")
            .and_then(|name| name.utf8_text(content.as_bytes()).ok())
            .map(str::to_string)
            .unwrap_or_else(|| node.kind().to_string()),
    }
}

fn impl_label(node: Node<'_>, content: &str) -> String {
    let snippet = &content[node.start_byte()..node.end_byte()];
    snippet
        .lines()
        .next()
        .unwrap_or("impl")
        .split('{')
        .next()
        .unwrap_or("impl")
        .trim()
        .chars()
        .take(96)
        .collect()
}

fn print_ast_breakdown(breakdown: &AstBreakdown) {
    if let Some(language) = breakdown.language {
        println!(
            "AST ({language}): {} tokens, budget {}",
            breakdown.total_tokens, breakdown.budget
        );
        for node in &breakdown.nodes {
            print_ast_node(node, 1);
        }
        if !breakdown.split_suggestions.is_empty() {
            println!("Split suggestions:");
            for suggestion in &breakdown.split_suggestions {
                println!(
                    "  after {} at line {} ({} tokens): {}",
                    suggestion.after_node,
                    suggestion.line,
                    suggestion.accumulated_tokens,
                    suggestion.reason
                );
            }
        }
    }
}

fn print_ast_node(node: &AstNodeBreakdown, depth: usize) {
    let indent = "  ".repeat(depth);
    println!(
        "{indent}{} {}: {} tokens (lines {}-{})",
        node.kind, node.name, node.tokens, node.start_line, node.end_line
    );
    for child in &node.children {
        print_ast_node(child, depth + 1);
    }
}

fn handle_models() -> Result<()> {
    let models = [
        ("gpt-4o", "o200k_base", "GPT-4o / GPT-5 / GPT-5.5"),
        ("gpt-4", "cl100k_base", "GPT-4 / GPT-4 Turbo"),
        ("gpt-3.5-turbo", "cl100k_base", "GPT-3.5 Turbo"),
    ];
    let aliases = [
        (
            "claude / opus / sonnet / haiku",
            "→ gpt-4o (conservative: max of BPE + chars/3)",
        ),
        ("codex / gpt-5 / gpt-5.5", "→ gpt-4o (o200k_base)"),
    ];

    println!("Supported models (OpenAI BPE tokenizer):");
    println!();
    for (name, bpe, desc) in &models {
        println!("  {name:<20} {bpe:<16} {desc}");
    }
    println!();
    println!("Aliases (resolved to closest BPE tokenizer):");
    println!();
    for (alias, target) in &aliases {
        println!("  {alias:<40} {target}");
    }
    println!();
    println!("Note: ConservativeTokenizer returns max(BPE count, chars/3).");
    println!("      Claude models have ~65K vocab vs OpenAI's 200K — chars/3");
    println!("      approximates Claude counts conservatively.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokuin::tokenizers::{ConservativeTokenizer, Tokenizer};

    fn tokenizer() -> ConservativeTokenizer {
        ConservativeTokenizer::new("gpt-4o").expect("test tokenizer should initialize")
    }

    #[test]
    fn ast_breakdown_collects_rust_items_and_impl_children() {
        let source = r#"
mod inner {
    pub fn nested() {}
}

pub struct Widget {
    id: u64,
}

impl Widget {
    pub fn new(id: u64) -> Self {
        Self { id }
    }
}

enum Mode {
    Fast,
    Slow,
}
"#;
        let tokenizer = tokenizer();
        let breakdown = build_ast_breakdown(
            std::path::Path::new("sample.rs"),
            source,
            tokenizer.count_tokens(source).unwrap(),
            1,
            &tokenizer,
        )
        .unwrap();

        assert_eq!(breakdown.language, Some("rust"));
        assert!(breakdown.warning.is_none());
        assert!(breakdown.split_suggestions.len() > 1);

        let names: Vec<_> = breakdown
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert!(names.contains(&"inner"));
        assert!(names.contains(&"Widget"));
        assert!(names.contains(&"Mode"));

        let widget_impl = breakdown
            .nodes
            .iter()
            .find(|node| node.kind == "impl_item")
            .expect("impl item should be present");
        assert!(widget_impl.children.iter().any(|node| node.name == "new"));
        assert!(
            breakdown
                .nodes
                .windows(2)
                .all(|pair| pair[0].tokens >= pair[1].tokens)
        );
    }

    #[test]
    fn ast_breakdown_falls_back_for_non_rust_files() {
        let tokenizer = tokenizer();
        let breakdown = build_ast_breakdown(
            std::path::Path::new("notes.md"),
            "# Notes",
            3,
            DEFAULT_AST_BUDGET,
            &tokenizer,
        )
        .unwrap();

        assert_eq!(breakdown.language, None);
        assert!(breakdown.nodes.is_empty());
        assert!(breakdown.split_suggestions.is_empty());
        assert!(breakdown.warning.unwrap().contains("supports only Rust"));
    }
}
