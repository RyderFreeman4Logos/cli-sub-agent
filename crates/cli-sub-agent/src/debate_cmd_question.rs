//! Debate question assembly for `csa debate` (#1741 monolith-gate split).
//!
//! Extracted verbatim from `debate_cmd::handle_debate` to keep that module
//! under the monolith gate. Resolves the debate question from
//! positional / `--topic` / `--question-file` / stdin, strips difficulty
//! frontmatter, and prepends `--context` and repeated `--file` content.

use anyhow::{Context, Result};
use std::io::{IsTerminal, Read};
use std::path::Path;

use crate::cli::DebateArgs;
use crate::debate_errors::EMPTY_DEBATE_QUESTION_ERROR;
use crate::run_helpers::is_prompt_file_stdin_sentinel;

/// Maximum size of a `--file` attachment for debate (5 MB).
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Build the effective debate question and extract any difficulty frontmatter.
///
/// Consumes `args.question` / `args.topic` (via `take`); reads `args.question_file`,
/// `args.context`, and `args.file` by reference. Returns the assembled question
/// plus the parsed frontmatter difficulty (if present).
pub(super) fn build_debate_question(args: &mut DebateArgs) -> Result<(String, Option<String>)> {
    let mut stdin = std::io::stdin();
    build_debate_question_from_reader(args, stdin.is_terminal(), &mut stdin)
}

#[cfg(test)]
pub(super) fn build_debate_question_from_reader<R: Read>(
    args: &mut DebateArgs,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<(String, Option<String>)> {
    build_debate_question_inner(args, stdin_is_terminal, reader)
}

#[cfg(not(test))]
fn build_debate_question_from_reader<R: Read>(
    args: &mut DebateArgs,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<(String, Option<String>)> {
    build_debate_question_inner(args, stdin_is_terminal, reader)
}

fn build_debate_question_inner<R: Read>(
    args: &mut DebateArgs,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<(String, Option<String>)> {
    let mut question = resolve_question_from_reader(
        args.question.take(),
        args.topic.take(),
        args.question_file.as_deref(),
        stdin_is_terminal,
        reader,
    )?;
    let parsed_question = crate::difficulty_routing::strip_difficulty_frontmatter(question)?;
    let frontmatter_difficulty = parsed_question.difficulty;
    question = parsed_question.prompt;
    if let Some(ctx) = &args.context {
        question = format!("<debate-context>\n{ctx}\n</debate-context>\n\n{question}");
    }
    let mut attached_files = String::new();
    for file_path in &args.file {
        let file_display = file_path.display();
        let metadata = std::fs::metadata(file_path)
            .with_context(|| format!("Failed to stat --file: {file_display}"))?;
        if metadata.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "--file '{}' is too large ({} bytes, max {} bytes)",
                file_display,
                metadata.len(),
                MAX_FILE_SIZE
            );
        }
        let file_content = std::fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read --file: {file_display}"))?;
        attached_files.push_str(&format!(
            "<attached-file path=\"{file_display}\">\n{file_content}\n</attached-file>\n\n"
        ));
    }
    if !attached_files.is_empty() {
        question = format!("{attached_files}{question}");
    }
    Ok((question, frontmatter_difficulty))
}

fn resolve_question_from_reader<R: Read>(
    question: Option<String>,
    topic: Option<String>,
    question_file: Option<&Path>,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<String> {
    if let Some(question) = question {
        if question == "-" {
            return read_question_from_stdin(stdin_is_terminal, reader);
        }
        return validate_inline_question(question);
    }

    if let Some(topic) = topic {
        return validate_inline_question(topic);
    }

    if let Some(path) = question_file {
        if is_prompt_file_stdin_sentinel(path) {
            return read_question_from_stdin(stdin_is_terminal, reader);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("--question-file: failed to read '{}'", path.display()))?;
        if content.trim().is_empty() {
            anyhow::bail!("--question-file '{}' is empty", path.display());
        }
        return Ok(content);
    }

    read_question_from_stdin(stdin_is_terminal, reader)
}

fn validate_inline_question(question: String) -> Result<String> {
    if question.trim().is_empty() {
        anyhow::bail!(EMPTY_DEBATE_QUESTION_ERROR);
    }
    Ok(question)
}

fn read_question_from_stdin<R: Read>(stdin_is_terminal: bool, reader: &mut R) -> Result<String> {
    if stdin_is_terminal {
        anyhow::bail!(EMPTY_DEBATE_QUESTION_ERROR);
    }

    let mut buffer = String::new();
    reader
        .read_to_string(&mut buffer)
        .context("failed to read debate question from stdin")?;
    if buffer.trim().is_empty() {
        anyhow::bail!(EMPTY_DEBATE_QUESTION_ERROR);
    }
    Ok(buffer)
}
