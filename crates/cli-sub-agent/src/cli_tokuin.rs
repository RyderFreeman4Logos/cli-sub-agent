#![allow(dead_code)]
//! CLI subcommand for token estimation via tokuin.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

const DEFAULT_MODEL: &str = "gpt-4o";

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
        } => handle_estimate(files, model, json, budget),
        TokuinCommands::Models => handle_models(),
    }
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

        results.push((file.display().to_string(), tokens));
    }

    if json {
        if results.len() == 1 {
            let (ref path, tokens) = results[0];
            let obj = serde_json::json!({
                "file": path,
                "model": resolved,
                "tokenizer": "conservative",
                "tokens": tokens,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            let arr: Vec<_> = results
                .iter()
                .map(|(path, tokens)| {
                    serde_json::json!({
                        "file": path,
                        "tokens": tokens,
                    })
                })
                .collect();
            let obj = serde_json::json!({
                "model": resolved,
                "tokenizer": "conservative",
                "files": arr,
                "total": results.iter().map(|(_, t)| t).sum::<usize>(),
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
    } else {
        for (path, tokens) in &results {
            if results.len() == 1 {
                println!("{tokens}");
            } else {
                let marker = budget
                    .filter(|&limit| *tokens > limit)
                    .map(|_| " OVER")
                    .unwrap_or("");
                println!("{tokens}\t{path}{marker}");
            }
        }
    }

    if exceeded {
        std::process::exit(1);
    }

    Ok(())
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
