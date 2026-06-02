//! Debate question assembly for `csa debate` (#1741 monolith-gate split).
//!
//! Extracted verbatim from `debate_cmd::handle_debate` to keep that module
//! under the monolith gate. Resolves the debate question from
//! `--prompt-file` / positional / `--topic` / stdin, strips difficulty
//! frontmatter, and prepends `--context` and repeated `--file` content.

use anyhow::{Context, Result};

use crate::cli::DebateArgs;
use crate::run_helpers::resolve_prompt_with_file;

/// Maximum size of a `--file` attachment for debate (5 MB).
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Build the effective debate question and extract any difficulty frontmatter.
///
/// Consumes `args.question` / `args.topic` (via `take`); reads `args.prompt_file`,
/// `args.context`, and `args.file` by reference. Returns the assembled question
/// plus the parsed frontmatter difficulty (if present).
pub(super) fn build_debate_question(args: &mut DebateArgs) -> Result<(String, Option<String>)> {
    let effective_question =
        crate::run_helpers::resolve_positional_stdin_sentinel(args.question.take())?
            .or_else(|| args.topic.take());
    let mut question = resolve_prompt_with_file(effective_question, args.prompt_file.as_deref())?;
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
