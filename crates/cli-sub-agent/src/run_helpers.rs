//! Helper functions for `csa run`: tool resolution, executor building, token parsing.

use anyhow::Result;
use std::io::Read;
use std::path::Path;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{Executor, ModelSpec, ThinkingBudget};
use csa_session::TokenUsage;

/// Resolve tool and model from CLI args and config.
///
/// Returns (tool, model_spec, model) where:
/// - tool: the selected tool (from CLI or tier-based selection)
/// - model_spec: optional model spec string (from CLI or tier)
/// - model: optional model string (from CLI, with alias resolution applied)
///
/// When tool is None, uses tier-based round-robin selection.
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
        // Enforce tier whitelist: model-spec must appear in tiers
        if let Some(cfg) = config {
            cfg.enforce_tier_whitelist(tool_name.as_str(), Some(spec))?;
        }
        return Ok((tool_name, Some(spec.to_string()), None));
    }

    // Case 2: tool provided → use it with optional model (apply alias resolution)
    if let Some(tool_name) = tool {
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        // Enforce tier whitelist: tool must be in tiers; model name must match if provided
        if let Some(cfg) = config {
            cfg.enforce_tier_whitelist(tool_name.as_str(), None)?;
            cfg.enforce_tier_model_name(tool_name.as_str(), resolved_model.as_deref())?;
        }
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
/// Keeps a `config` parameter for call-site API stability.
pub(crate) fn build_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking: Option<&str>,
    _config: Option<&ProjectConfig>,
) -> Result<Executor> {
    let executor = if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        Executor::from_spec(&parsed)?
    } else {
        let final_model = model.map(|s| s.to_string());
        let budget = thinking.map(ThinkingBudget::parse).transpose()?;

        Executor::from_tool_name(tool, final_model, budget)
    };

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
///
/// Uses character (not byte) counting to safely handle multi-byte UTF-8.
pub(crate) fn truncate_prompt(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        // Find byte offset for the character at position (max_len - 3)
        let truncate_at_chars = max_len.saturating_sub(3);
        let byte_offset = s
            .char_indices()
            .nth(truncate_at_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let substring = &s[..byte_offset];

        // Try to break at last space if possible
        if let Some(last_space) = substring.rfind(' ') {
            if last_space > byte_offset / 2 {
                return format!("{}...", &s[..last_space]);
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
            // Only match standalone "tokens:" — skip if preceded by a letter or
            // underscore (e.g. "input_tokens:" or "output_tokens:" already
            // handled above).
            let prev = line_lower.as_bytes().get(pos.wrapping_sub(1)).copied();
            let is_standalone = pos == 0 || !matches!(prev, Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'));
            if is_standalone {
                if let Some(val) = extract_number(&line[pos..]) {
                    usage.total_tokens = Some(val);
                    found_any = true;
                }
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

    if found_any { Some(usage) } else { None }
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
///
/// For ACP-routed tools (codex, claude-code), checks for the ACP adapter
/// binary (`codex-acp`, `claude-code-acp`). For legacy tools, checks the
/// native CLI binary.
pub(crate) fn is_tool_binary_available(tool_name: &str) -> bool {
    let binary = match tool_name {
        "gemini-cli" => "gemini",
        "opencode" => "opencode",
        // ACP adapter binaries (npm: @zed-industries/codex-acp, @zed-industries/claude-code-acp)
        "codex" => "codex-acp",
        "claude-code" => "claude-code-acp",
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

/// Detect the parent tool context.
///
/// Resolution order:
/// 1. `CSA_TOOL` environment variable (set by CSA when spawning children)
/// 2. `CSA_PARENT_TOOL` environment variable (set for grandchild processes)
/// 3. Process tree walking via `/proc` (Linux-only fallback)
pub(crate) fn detect_parent_tool() -> Option<String> {
    std::env::var("CSA_TOOL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CSA_PARENT_TOOL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(crate::process_tree::detect_ancestor_tool)
}

/// Resolve parent tool context using detection result with global-config fallback.
///
/// Resolution order:
/// 1. Detected parent tool from runtime context
/// 2. `~/.config/cli-sub-agent/config.toml` `[defaults].tool`
/// 3. None
pub(crate) fn resolve_tool(detected: Option<String>, config: &GlobalConfig) -> Option<String> {
    detected.or_else(|| config.defaults.tool.clone())
}

/// Infer whether a prompt requires editing existing files.
///
/// Returns:
/// - `Some(true)` when the prompt clearly asks for implementation/editing.
/// - `Some(false)` when the prompt explicitly requests read-only execution.
/// - `None` when intent is ambiguous.
pub(crate) fn infer_task_edit_requirement(prompt: &str) -> Option<bool> {
    let prompt_lower = prompt.to_lowercase();

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

    let edit_markers = [
        "fix ",
        "implement",
        "refactor",
        "edit ",
        "modify",
        "update",
        "patch",
        "write code",
        "create file",
        "rename",
    ];
    if edit_markers
        .iter()
        .any(|marker| prompt_lower.contains(marker))
    {
        return Some(true);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{build_executor, infer_task_edit_requirement, resolve_tool, truncate_prompt};
    use csa_config::GlobalConfig;
    use csa_core::types::ToolName;

    #[test]
    fn truncate_prompt_short_string_unchanged() {
        assert_eq!(truncate_prompt("hello", 10), "hello");
    }

    #[test]
    fn truncate_prompt_exact_length_unchanged() {
        assert_eq!(truncate_prompt("hello", 5), "hello");
    }

    #[test]
    fn truncate_prompt_ascii_truncated() {
        let result = truncate_prompt("hello world this is long", 15);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 15);
    }

    #[test]
    fn truncate_prompt_multibyte_no_panic() {
        // 10 CJK chars (3 bytes each = 30 bytes); truncate to 6 chars should not panic
        let cjk =
            "\u{4f60}\u{597d}\u{4e16}\u{754c}\u{6d4b}\u{8bd5}\u{8fd9}\u{662f}\u{4e2d}\u{6587}";
        let result = truncate_prompt(cjk, 6);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 6);
    }

    #[test]
    fn truncate_prompt_emoji_no_panic() {
        let emoji = "Hello \u{1f30d}\u{1f525}\u{1f680} world test";
        let result = truncate_prompt(emoji, 10);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 10);
    }

    #[test]
    fn truncate_prompt_empty_string() {
        assert_eq!(truncate_prompt("", 10), "");
    }

    #[test]
    fn truncate_prompt_mixed_multibyte() {
        // Mix of ASCII, CJK, emoji
        let mixed = "Fix \u{4fee}\u{590d} bug \u{1f41b} in auth";
        let result = truncate_prompt(mixed, 12);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 12);
    }

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
    fn build_executor_model_and_thinking_coexist() {
        let exec = build_executor(
            &ToolName::Codex,
            None,
            Some("gpt-5.1-codex-mini"),
            Some("low"),
            None,
        )
        .unwrap();
        let debug = format!("{:?}", exec);
        assert!(
            debug.contains("gpt-5.1-codex-mini"),
            "model missing: {debug}"
        );
        assert!(debug.contains("Low"), "thinking budget missing: {debug}");
    }

    #[test]
    fn build_executor_thinking_only() {
        let exec = build_executor(&ToolName::Codex, None, None, Some("high"), None).unwrap();
        let debug = format!("{:?}", exec);
        assert!(debug.contains("High"), "thinking budget missing: {debug}");
    }

    #[test]
    fn build_executor_invalid_thinking_errors() {
        let result = build_executor(&ToolName::Codex, None, None, Some("bogus"), None);
        assert!(result.is_err());
    }

    // --- is_compress_command tests ---

    #[test]
    fn is_compress_command_slash_compress() {
        assert!(super::is_compress_command("/compress"));
    }

    #[test]
    fn is_compress_command_slash_compact() {
        assert!(super::is_compress_command("/compact"));
    }

    #[test]
    fn is_compress_command_slash_compact_with_args() {
        assert!(super::is_compress_command(
            "/compact Keep design decisions."
        ));
    }

    #[test]
    fn is_compress_command_with_whitespace_padding() {
        assert!(super::is_compress_command("  /compress  "));
    }

    #[test]
    fn is_compress_command_not_compress() {
        assert!(!super::is_compress_command("analyze the code"));
    }

    #[test]
    fn is_compress_command_empty_string() {
        assert!(!super::is_compress_command(""));
    }

    #[test]
    fn is_compress_command_partial_match_rejected() {
        assert!(!super::is_compress_command("/compressor"));
    }

    // --- parse_tool_name tests ---

    #[test]
    fn parse_tool_name_all_valid() {
        use super::parse_tool_name;
        assert!(matches!(
            parse_tool_name("gemini-cli").unwrap(),
            ToolName::GeminiCli
        ));
        assert!(matches!(
            parse_tool_name("opencode").unwrap(),
            ToolName::Opencode
        ));
        assert!(matches!(parse_tool_name("codex").unwrap(), ToolName::Codex));
        assert!(matches!(
            parse_tool_name("claude-code").unwrap(),
            ToolName::ClaudeCode
        ));
    }

    #[test]
    fn resolve_tool_prefers_detected_value() {
        let mut config = GlobalConfig::default();
        config.defaults.tool = Some("claude-code".to_string());

        let resolved = resolve_tool(Some("codex".to_string()), &config);
        assert_eq!(resolved.as_deref(), Some("codex"));
    }

    #[test]
    fn resolve_tool_uses_config_default_when_detection_missing() {
        let mut config = GlobalConfig::default();
        config.defaults.tool = Some("codex".to_string());

        let resolved = resolve_tool(None, &config);
        assert_eq!(resolved.as_deref(), Some("codex"));
    }

    #[test]
    fn resolve_tool_returns_none_when_both_missing() {
        let config = GlobalConfig::default();
        let resolved = resolve_tool(None, &config);
        assert!(resolved.is_none());
    }

    #[test]
    fn parse_tool_name_unknown_errors() {
        assert!(super::parse_tool_name("nvim").is_err());
    }

    #[test]
    fn parse_tool_name_empty_errors() {
        assert!(super::parse_tool_name("").is_err());
    }

    // --- parse_token_usage tests ---

    #[test]
    fn parse_token_usage_all_fields() {
        let output = "input_tokens: 1000\noutput_tokens: 500\ntotal_tokens: 1500\ncost: $0.05";
        let usage = super::parse_token_usage(output).unwrap();
        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(500));
        assert_eq!(usage.total_tokens, Some(1500));
        assert!((usage.estimated_cost_usd.unwrap() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_token_usage_input_output_sums_to_total() {
        // When only input_tokens and output_tokens are present (no explicit total),
        // total_tokens should be their sum. The generic "tokens:" pattern must NOT
        // match "output_tokens:" or "input_tokens:".
        let output = "input_tokens: 200\noutput_tokens: 300";
        let usage = super::parse_token_usage(output).unwrap();
        assert_eq!(usage.input_tokens, Some(200));
        assert_eq!(usage.output_tokens, Some(300));
        assert_eq!(usage.total_tokens, Some(500));
    }

    #[test]
    fn parse_token_usage_explicit_total_preferred() {
        let output = "total_tokens: 1500";
        let usage = super::parse_token_usage(output).unwrap();
        assert_eq!(usage.total_tokens, Some(1500));
    }

    #[test]
    fn parse_token_usage_generic_tokens_field() {
        let output = "Tokens: 5000";
        let usage = super::parse_token_usage(output).unwrap();
        assert_eq!(usage.total_tokens, Some(5000));
    }

    #[test]
    fn parse_token_usage_no_match_returns_none() {
        let output = "Hello world, no token info here.";
        assert!(super::parse_token_usage(output).is_none());
    }

    #[test]
    fn parse_token_usage_empty_string_returns_none() {
        assert!(super::parse_token_usage("").is_none());
    }

    // --- extract_number tests ---

    #[test]
    fn extract_number_basic() {
        assert_eq!(super::extract_number("tokens: 42"), Some(42));
    }

    #[test]
    fn extract_number_with_spaces() {
        assert_eq!(super::extract_number("tokens:  123  "), Some(123));
    }

    #[test]
    fn extract_number_no_colon_returns_none() {
        assert!(super::extract_number("tokens 42").is_none());
    }

    #[test]
    fn extract_number_no_digits_returns_none() {
        assert!(super::extract_number("tokens: abc").is_none());
    }

    // --- extract_cost tests ---

    #[test]
    fn extract_cost_basic() {
        let result = super::extract_cost("cost: $1.50");
        assert!((result.unwrap() - 1.50).abs() < f64::EPSILON);
    }

    #[test]
    fn extract_cost_small_value() {
        let result = super::extract_cost("estimated_cost: $0.0042");
        assert!((result.unwrap() - 0.0042).abs() < f64::EPSILON);
    }

    #[test]
    fn extract_cost_no_dollar_returns_none() {
        assert!(super::extract_cost("cost: 1.50").is_none());
    }

    #[test]
    fn extract_cost_empty_returns_none() {
        assert!(super::extract_cost("").is_none());
    }

    #[test]
    fn build_executor_model_spec_overrides_both() {
        let exec = build_executor(
            &ToolName::Codex,
            Some("codex/openai/gpt-5.3-codex/xhigh"),
            Some("ignored-model"),
            Some("ignored-thinking"),
            None,
        )
        .unwrap();
        let debug = format!("{:?}", exec);
        assert!(
            debug.contains("gpt-5.3-codex"),
            "model_spec model missing: {debug}"
        );
        assert!(
            debug.contains("Xhigh"),
            "model_spec thinking missing: {debug}"
        );
    }
}
