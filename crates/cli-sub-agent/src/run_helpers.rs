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
    force: bool,
    force_override_user_config: bool,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    // Case 1: model_spec provided → parse it to get tool
    if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        let tool_name = parse_tool_name(&parsed.tool)?;
        // Enforce tool enablement from user config
        if let Some(cfg) = config {
            cfg.enforce_tool_enabled(tool_name.as_str(), force_override_user_config)?;
        }
        // Enforce tier whitelist: model-spec must appear in tiers
        if !force {
            if let Some(cfg) = config {
                cfg.enforce_tier_whitelist(tool_name.as_str(), Some(spec))?;
            }
        }
        return Ok((tool_name, Some(spec.to_string()), None));
    }

    // Case 2: tool provided → use it with optional model (apply alias resolution)
    if let Some(tool_name) = tool {
        // Enforce tool enablement from user config
        if let Some(cfg) = config {
            cfg.enforce_tool_enabled(tool_name.as_str(), force_override_user_config)?;
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        // Enforce tier whitelist: tool must be in tiers; model name must match if provided
        if !force {
            if let Some(cfg) = config {
                cfg.enforce_tier_whitelist(tool_name.as_str(), None)?;
                cfg.enforce_tier_model_name(tool_name.as_str(), resolved_model.as_deref())?;
            }
        }
        return Ok((tool_name, None, resolved_model));
    }

    // Case 3: neither tool nor model_spec → use round-robin tier-based selection.
    // When --force is active, bypass tiers and pick any installed+enabled tool.
    if force {
        for tool in csa_config::global::all_known_tools() {
            let name = tool.as_str();
            let enabled = config.is_none_or(|cfg| cfg.is_tool_enabled(name));
            if enabled && is_tool_binary_available(name) {
                let tool_name = parse_tool_name(name)?;
                return Ok((tool_name, None, None));
            }
        }
        anyhow::bail!(
            "No installed and enabled tools found. Install at least one tool \
             (gemini-cli, opencode, codex, claude-code) or check enabled status."
        );
    }

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

    // Fallback: minimal-init configs with empty tiers — pick any auto-selectable installed tool.
    // Only activates when tiers are empty to avoid silently bypassing configured tier mappings.
    if let Some(cfg) = config {
        if cfg.tiers.is_empty() {
            for tool in csa_config::global::all_known_tools() {
                let name = tool.as_str();
                if cfg.is_tool_auto_selectable(name) && is_tool_binary_available(name) {
                    let tool_name = parse_tool_name(name)?;
                    return Ok((tool_name, None, None));
                }
            }
        }
    }

    // Case 4: no config, no tier, and no auto-selectable installed tool → error
    anyhow::bail!(
        "No tool specified and no tier-based or auto-selectable tool available. \
         Use --tool, run 'csa init --full' to configure tiers, or install a tool."
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
#[path = "run_helpers_tests.rs"]
mod tests;
