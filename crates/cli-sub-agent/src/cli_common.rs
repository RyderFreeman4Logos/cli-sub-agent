// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use anyhow::Result;

/// Build version string combining Cargo.toml version and git describe.
pub(crate) fn build_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let cargo_ver = env!("CARGO_PKG_VERSION");
        let git_desc = env!("CSA_GIT_DESCRIBE");
        if git_desc.is_empty() {
            cargo_ver.to_string()
        } else {
            format!("{cargo_ver} ({git_desc})")
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnTarget {
    Last,
    Auto,
    SessionId(String),
}

pub fn parse_return_to(value: &str) -> Result<ReturnTarget> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("return target cannot be empty");
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "last" => Ok(ReturnTarget::Last),
        "auto" => Ok(ReturnTarget::Auto),
        _ => Ok(ReturnTarget::SessionId(trimmed.to_string())),
    }
}

pub(crate) fn validate_return_to(value: &str) -> std::result::Result<String, String> {
    parse_return_to(value)
        .map(|_| value.to_string())
        .map_err(|e| e.to_string())
}

pub(crate) fn parse_cli_tool_name(
    tool: &str,
) -> std::result::Result<csa_core::types::ToolName, String> {
    if csa_core::types::is_removed_tool_name(tool) {
        return Err(csa_core::types::removed_tool_error(tool));
    }
    match tool {
        "opencode" => Ok(csa_core::types::ToolName::Opencode),
        "codex" => Ok(csa_core::types::ToolName::Codex),
        "claude-code" => Ok(csa_core::types::ToolName::ClaudeCode),
        "openai-compat" => Ok(csa_core::types::ToolName::OpenaiCompat),
        "hermes" => Ok(csa_core::types::ToolName::Hermes),
        "antigravity-cli" => Ok(csa_core::types::ToolName::AntigravityCli),
        "claude" => Ok(csa_core::types::ToolName::ClaudeCode),
        "antigravity" => Ok(csa_core::types::ToolName::AntigravityCli),
        _ => Err(format!(
            "unknown tool '{tool}'. Valid values: opencode, codex, claude-code, openai-compat, hermes, antigravity-cli"
        )),
    }
}

pub(crate) fn parse_model_spec_arg(spec: &str) -> std::result::Result<String, String> {
    let (value, warning) = parse_model_spec_arg_with_warning(spec)?;
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    Ok(value)
}

pub(crate) fn parse_model_spec_arg_with_warning(
    spec: &str,
) -> std::result::Result<(String, Option<String>), String> {
    let known_tools: Vec<&'static str> = csa_config::global::all_known_tools()
        .iter()
        .map(|tool| tool.as_str())
        .collect();
    let parsed = csa_executor::ModelSpec::parse(spec).map_err(|e| e.to_string())?;
    if parsed.tool.trim().is_empty()
        || parsed.provider.trim().is_empty()
        || parsed.model.trim().is_empty()
    {
        return Err(format!(
            "Invalid model spec '{spec}': tool/provider/model/thinking_budget segments cannot be empty"
        ));
    }
    if csa_core::types::is_removed_tool_name(&parsed.tool) {
        return Err(csa_core::types::removed_tool_error(&parsed.tool));
    }

    match parsed.validate_with_catalog(&known_tools) {
        Ok(()) => Ok((spec.to_string(), None)),
        Err(csa_executor::model_spec::ModelSpecValidationError::UnknownModel {
            tool,
            provider,
            got,
            ..
        }) => Ok((
            spec.to_string(),
            Some(format!(
                "warning: model '{got}' for tool '{tool}' provider '{provider}' is not recognized in CSA's built-in model registry; passing it through unchanged, but the backend may reject it"
            )),
        )),
        Err(err) => Err(err.to_string()),
    }
}

pub(crate) fn parse_spec_path_arg(spec: &str) -> std::result::Result<String, String> {
    csa_core::spec_validate::validate_spec(std::path::Path::new(spec))
        .map(|path| path.display().to_string())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_cli_tool_name, parse_model_spec_arg_with_warning};

    #[test]
    fn parse_model_spec_arg_warns_and_passes_unknown_model() {
        let (value, warning) =
            parse_model_spec_arg_with_warning("codex/openai/claude-opus-4-8/xhigh").unwrap();

        assert_eq!(value, "codex/openai/claude-opus-4-8/xhigh");
        let warning = warning.expect("unknown model should warn");
        assert!(warning.starts_with("warning:"), "{warning}");
        assert!(warning.contains("claude-opus-4-8"), "{warning}");
        assert!(warning.contains("not recognized"), "{warning}");
        assert!(warning.contains("backend may reject"), "{warning}");
    }

    #[test]
    fn parse_model_spec_arg_still_rejects_unknown_provider() {
        let err = parse_model_spec_arg_with_warning("codex/anthropic/gpt-5.5/xhigh")
            .expect_err("unknown provider should remain a hard error");

        assert!(err.contains("anthropic"), "{err}");
        assert!(err.contains("openai"), "{err}");
    }

    #[test]
    fn parse_model_spec_arg_still_rejects_empty_segments() {
        let err = parse_model_spec_arg_with_warning("/openai/gpt-5.5/xhigh")
            .expect_err("empty tool should remain malformed");

        assert!(err.contains("segments cannot be empty"), "{err}");
    }

    #[test]
    fn parse_model_spec_arg_rejects_removed_gemini_cli() {
        let err =
            parse_model_spec_arg_with_warning("gemini-cli/google/gemini-3-pro/xhigh").unwrap_err();

        assert!(err.contains("no longer supported"), "{err}");
        assert!(err.contains("discontinued"), "{err}");
        assert!(err.contains("antigravity-cli"), "{err}");
    }

    #[test]
    fn parse_cli_tool_name_rejects_removed_gemini_cli() {
        let err = parse_cli_tool_name("gemini-cli").unwrap_err();

        assert!(err.contains("no longer supported"), "{err}");
        assert!(err.contains("provider is discontinued"), "{err}");
    }

    #[test]
    fn parse_cli_tool_name_accepts_hermes() {
        let tool = parse_cli_tool_name("hermes").unwrap();

        assert_eq!(tool, csa_core::types::ToolName::Hermes);
    }
}
