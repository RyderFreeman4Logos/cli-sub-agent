#![allow(dead_code)]
//! CLI subcommand for token estimation via tokuin.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum TokuinCommands {
    /// Estimate token count for a file
    Estimate {
        /// Path to the file to estimate
        file: PathBuf,

        /// Model to use for tokenization (default: gpt-4)
        #[arg(long, default_value = "gpt-4")]
        model: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn handle_tokuin(cmd: TokuinCommands) -> Result<()> {
    match cmd {
        TokuinCommands::Estimate { file, model, json } => handle_estimate(file, model, json),
    }
}

fn handle_estimate(file: PathBuf, model: String, json: bool) -> Result<()> {
    use tokuin::tokenizers::{OpenAITokenizer, Tokenizer};

    let content = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let tokenizer = OpenAITokenizer::new(&model)
        .map_err(|e| anyhow::anyhow!("Failed to create tokenizer for model '{model}': {e}"))?;

    let tokens = tokenizer
        .count_tokens(&content)
        .map_err(|e| anyhow::anyhow!("Failed to count tokens: {e}"))?;

    if json {
        let result = serde_json::json!({
            "file": file.display().to_string(),
            "model": model,
            "tokens": tokens,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{tokens}");
    }

    Ok(())
}
