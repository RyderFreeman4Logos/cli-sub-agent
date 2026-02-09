//! Helper functions for `csa run`: tool resolution, executor building, token parsing.

use anyhow::Result;
use std::io::Read;
use std::path::Path;
use tracing::warn;

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::{Executor, ModelSpec};
use csa_session::TokenUsage;

/// Resolve tool and model from CLI args and config.
///
/// Returns (tool, model_spec, model) where:
/// - tool: the selected tool (from CLI or tier-based selection)
/// - model_spec: optional model spec string (from CLI or tier)
/// - model: optional model string (from CLI, with alias resolution applied)
pub(crate) fn resolve_tool_and_model(
    tool: Option<ToolName>,
    model_spec: Option<&str>,
    model: Option<&str>,
    config: Option<&ProjectConfig>,
    project_root: &Path,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    // Case 1: model_spec provided → parse it to get tool
    if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        let tool_name = parse_tool_name(&parsed.tool)?;
        return Ok((tool_name, Some(spec.to_string()), None));
    }

    // Case 2: tool provided → use it with optional model (apply alias resolution)
    if let Some(tool_name) = tool {
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        return Ok((tool_name, None, resolved_model));
    }

    // Case 3: neither tool nor model_spec → use round-robin tier-based selection
    if let Some(cfg) = config {
        // Try round-robin rotation first (needs project root to persist state)
        if let Ok(Some((tool_name_str, tier_model_spec))) =
            csa_scheduler::resolve_tier_tool_rotated(cfg, "default", project_root, false)
        {
            let tool_name = parse_tool_name(&tool_name_str)?;
            return Ok((tool_name, Some(tier_model_spec), None));
        }
        // Fallback: original non-rotating selection
        if let Some((tool_name_str, tier_model_spec)) = cfg.resolve_tier_tool("default") {
            let tool_name = parse_tool_name(&tool_name_str)?;
            return Ok((tool_name, Some(tier_model_spec), None));
        }
    }

    // Case 4: no config or no tier mapping → error
    anyhow::bail!(
        "No tool specified and no tier-based selection available. \
         Use --tool or run 'csa init' to configure tiers."
    )
}

/// Build an executor from tool, model_spec, model, and thinking parameters.
///
/// If `config` is provided, automatically injects config-driven settings
/// (e.g., `suppress_notify` for Codex) so all call sites benefit consistently.
pub(crate) fn build_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    config: Option<&ProjectConfig>,
) -> Result<Executor> {
    let mut executor = if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        let tool_name = parse_tool_name(&parsed.tool)?;
        Executor::from_tool_name(&tool_name, Some(parsed.model))
    } else {
        let mut final_model = model.map(|s| s.to_string());

        // Apply thinking budget if specified (tool-specific logic)
        if let Some(thinking_level) = thinking {
            if final_model.is_none() {
                // Generate model string with thinking budget
                final_model = Some(format!("thinking:{}", thinking_level));
            } else {
                warn!("Both --model and --thinking specified; --thinking ignored");
            }
        }

        Executor::from_tool_name(tool, final_model)
    };

    // Inject config-driven settings
    if let Some(cfg) = config {
        executor.set_suppress_notify(cfg.should_suppress_codex_notify());
    }

    Ok(executor)
}

/// Check if a prompt is a context compress/compact command.
pub(crate) fn is_compress_command(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    trimmed == "/compress" || trimmed == "/compact" || trimmed.starts_with("/compact ")
}

/// Parse a tool name string to ToolName enum.
pub(crate) fn parse_tool_name(name: &str) -> Result<ToolName> {
    match name {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        _ => anyhow::bail!("Unknown tool: {}", name),
    }
}

/// Truncate a string to max_len characters, adding "..." if truncated.
pub(crate) fn truncate_prompt(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a good break point (preferably a space)
        let truncate_at = max_len.saturating_sub(3);
        let substring = &s[..truncate_at.min(s.len())];

        // Try to break at last space if possible
        if let Some(last_space) = substring.rfind(' ') {
            if last_space > truncate_at / 2 {
                return format!("{}...", &substring[..last_space]);
            }
        }

        format!("{}...", substring)
    }
}

/// Parse token usage from tool output (best-effort, returns None on failure).
///
/// Looks for common patterns in stdout/stderr:
/// - "tokens: N" or "Tokens: N" or "total_tokens: N"
/// - "input_tokens: N" / "output_tokens: N"
/// - "cost: $N.NN" or "estimated_cost: $N.NN"
pub(crate) fn parse_token_usage(output: &str) -> Option<TokenUsage> {
    let mut usage = TokenUsage::default();
    let mut found_any = false;

    // Simple pattern matching without regex
    for line in output.lines() {
        let line_lower = line.to_lowercase();

        // Parse input_tokens
        if let Some(pos) = line_lower.find("input_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.input_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse output_tokens
        if let Some(pos) = line_lower.find("output_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.output_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse total_tokens
        if let Some(pos) = line_lower.find("total_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        } else if let Some(pos) = line_lower.find("tokens:") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse cost (look for "$N.NN" pattern)
        if line_lower.contains("cost") {
            if let Some(val) = extract_cost(line) {
                usage.estimated_cost_usd = Some(val);
                found_any = true;
            }
        }
    }

    // Calculate total_tokens if not found but input/output are available
    if usage.total_tokens.is_none() {
        if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
            usage.total_tokens = Some(input + output);
            found_any = true;
        }
    }

    if found_any {
        Some(usage)
    } else {
        None
    }
}

/// Extract a number after colon or equals sign.
fn extract_number(text: &str) -> Option<u64> {
    // Find colon or equals
    let start = text.find(':')?;
    let after_colon = &text[start + 1..];

    // Take first word after colon
    let num_str: String = after_colon
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();

    num_str.parse().ok()
}

/// Extract cost value after $ sign.
fn extract_cost(text: &str) -> Option<f64> {
    let start = text.find('$')?;
    let after_dollar = &text[start + 1..];

    // Take digits and decimal point
    let num_str: String = after_dollar
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    num_str.parse().ok()
}

/// Read prompt from CLI argument or stdin.
pub(crate) fn read_prompt(prompt: Option<String>) -> Result<String> {
    if let Some(p) = prompt {
        if p.trim().is_empty() {
            anyhow::bail!(
                "Empty prompt provided. Usage:\n  csa run --tool <tool> \"your prompt here\"\n  echo \"prompt\" | csa run --tool <tool>"
            );
        }
        Ok(p)
    } else {
        // No prompt argument: read from stdin
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            anyhow::bail!(
                "No prompt provided and stdin is a terminal.\n\n\
                 Usage:\n  \
                 csa run --tool <tool> \"your prompt here\"\n  \
                 echo \"prompt\" | csa run --tool <tool>"
            );
        }
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        if buffer.trim().is_empty() {
            anyhow::bail!("Empty prompt from stdin. Provide a non-empty prompt.");
        }
        Ok(buffer)
    }
}

/// Check if a tool's binary is available on PATH (synchronous).
pub(crate) fn is_tool_binary_available(tool_name: &str) -> bool {
    let binary = match tool_name {
        "gemini-cli" => "gemini",
        "opencode" => "opencode",
        "codex" => "codex",
        "claude-code" => "claude",
        _ => return false,
    };
    std::process::Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Infer whether a prompt requires editing existing files.
///
/// Returns:
/// - `Some(true)` when the prompt clearly asks for implementation/editing.
/// - `Some(false)` when the prompt explicitly requests read-only execution.
/// - `None` when intent is ambiguous.
pub(crate) fn infer_task_edit_requirement(prompt: &str) -> Option<bool> {
    let prompt_lower = prompt.to_lowercase();

    // Phase 0: Check explicit read-only markers (substring-safe phrases).
    let explicit_read_only = [
        "read-only",
        "readonly",
        "do not edit",
        "don't edit",
        "must not edit",
        "without editing",
    ];
    if explicit_read_only
        .iter()
        .any(|marker| prompt_lower.contains(marker))
    {
        return Some(false);
    }

    // Tokenize once: lowercase words with trailing/leading punctuation stripped.
    // Filter empty tokens to handle prompts starting with emoji/bullets (e.g. "✅ Fix").
    let tokens: Vec<&str> = prompt_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();

    // Phase 1: Unambiguous multi-word token sequences.
    let seq_pairs: &[(&str, &str)] = &[("write", "code"), ("create", "file")];
    for window in tokens.windows(2) {
        for (a, b) in seq_pairs {
            if window[0] == *a && window[1] == *b {
                return Some(true);
            }
        }
    }

    // Phase 2: "fix" as an independent word followed by a determiner.
    // Prevents "prefix the" from matching by requiring "fix" as a whole token.
    let fix_determiners = ["the", "a", "this", "that", "my", "our"];
    for window in tokens.windows(2) {
        if window[0] == "fix" && fix_determiners.contains(&window[1]) {
            return Some(true);
        }
    }

    // Phase 3: Single-word markers that are unambiguous as standalone words.
    let word_markers = ["refactor", "rename", "modify", "patch"];
    if tokens.iter().any(|t| word_markers.contains(t)) {
        return Some(true);
    }

    // Phase 4: Ambiguous verbs — only match as the first meaningful word.
    // Skips polite prefixes and filler adverbs before the verb.
    let skip_prefixes: &[&str] = &[
        "please", "can", "could", "would", "should", "shall", "you", // modals & polite
        "we", "i", "lets", "let", "us", "need", "to", "go", // pronouns & lead-ins
        "also", "just", "now", "then", "quickly", // filler adverbs
    ];
    let first_verb = tokens.iter().find(|t| !skip_prefixes.contains(t)).copied();
    let verb_markers = ["edit", "update", "implement", "fix"];
    if let Some(verb) = first_verb {
        if verb_markers.contains(&verb) {
            return Some(true);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::infer_task_edit_requirement;

    #[test]
    fn infer_edit_requirement_detects_explicit_read_only() {
        let result = infer_task_edit_requirement("Analyze auth flow in read-only mode");
        assert_eq!(result, Some(false));
    }

    #[test]
    fn infer_edit_requirement_detects_implementation_intent() {
        let result = infer_task_edit_requirement("Fix the login bug and update tests");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_requirement_read_only_overrides_edit_words() {
        let result = infer_task_edit_requirement("Do not edit files, only review this patch");
        assert_eq!(result, Some(false));
    }

    #[test]
    fn infer_edit_requirement_returns_none_for_ambiguous_prompt() {
        let result = infer_task_edit_requirement("Continue work from previous session");
        assert_eq!(result, None);
    }

    #[test]
    fn infer_edit_requirement_keeps_analysis_only_prompt_ambiguous() {
        let result = infer_task_edit_requirement("Review auth flow and report issues");
        assert_eq!(result, None);
    }

    #[test]
    fn infer_edit_no_false_positive_on_explain_implementation() {
        let result = infer_task_edit_requirement("Explain the implementation details");
        assert_eq!(result, None);
    }

    #[test]
    fn infer_edit_no_false_positive_on_check_updates() {
        let result = infer_task_edit_requirement("Check for updates in the dependency list");
        assert_eq!(result, None);
    }

    #[test]
    fn infer_edit_detects_leading_verb_implement() {
        let result = infer_task_edit_requirement("Implement JWT validation in auth module");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_detects_refactor_as_word() {
        let result = infer_task_edit_requirement("Please refactor the auth module");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_fix_this_triggers() {
        let result = infer_task_edit_requirement("Fix this compilation error");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_review_and_fix_is_ambiguous() {
        // "review and fix" — "fix" is not the first verb, so stays ambiguous.
        let result = infer_task_edit_requirement("Review and fix issues");
        assert_eq!(result, None);
    }

    // Regression tests from codex review: substring boundary issues
    #[test]
    fn infer_edit_no_false_positive_on_prefix_the() {
        let result = infer_task_edit_requirement("Prefix the string with 0x");
        assert_eq!(result, None);
    }

    #[test]
    fn infer_edit_no_false_positive_on_suffix_a() {
        let result = infer_task_edit_requirement("Suffix a newline to the output");
        assert_eq!(result, None);
    }

    // Polite prefixes should be skipped
    #[test]
    fn infer_edit_please_implement_triggers() {
        let result = infer_task_edit_requirement("Please implement the auth module");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_could_you_update_triggers() {
        let result = infer_task_edit_requirement("Could you update the config parser?");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_fix_as_first_verb_triggers() {
        let result = infer_task_edit_requirement("Fix the broken CI pipeline");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_bare_fix_as_first_verb() {
        let result = infer_task_edit_requirement("Fix issues in the parser");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_emoji_prefix_does_not_break_verb_detection() {
        let result = infer_task_edit_requirement("✅ Fix the bug in auth");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_bullet_prefix_does_not_break_verb_detection() {
        let result = infer_task_edit_requirement("- Implement the new parser");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_filler_also_update_triggers() {
        let result = infer_task_edit_requirement("Please also update the config");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_filler_just_implement_triggers() {
        let result = infer_task_edit_requirement("Please just implement X");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_we_need_to_update_triggers() {
        let result = infer_task_edit_requirement("We need to update the config");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_should_we_implement_triggers() {
        let result = infer_task_edit_requirement("Should we implement X?");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_lets_fix_triggers() {
        let result = infer_task_edit_requirement("Let's fix the broken tests");
        assert_eq!(result, Some(true));
    }

    #[test]
    fn infer_edit_i_need_to_edit_triggers() {
        let result = infer_task_edit_requirement("I need to edit the config file");
        assert_eq!(result, Some(true));
    }
}
