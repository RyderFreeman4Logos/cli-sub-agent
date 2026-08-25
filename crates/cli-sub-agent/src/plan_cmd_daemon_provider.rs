use std::path::Path;

pub(super) const PR_BOT_PROVIDER_VAR: &str = "CSA_MODEL_PROVIDER";

pub(super) fn native_parent_provider(
    parent_tool: Option<&str>,
    hermes_provider: Option<&str>,
) -> Option<String> {
    match parent_tool {
        Some("codex") => Some("openai".to_string()),
        Some("claude-code") => Some("claude".to_string()),
        Some("hermes") => hermes_provider.map(str::to_string),
        _ => None,
    }
}

pub(super) fn inject_pr_bot_parent_provider(
    file: &Option<String>,
    pattern: &Option<String>,
    vars: &mut Vec<String>,
    parent_provider: Option<&str>,
) -> Option<String> {
    let is_pr_bot = pattern.as_deref() == Some("pr-bot")
        || file.as_deref().is_some_and(|path| {
            Path::new(path).ends_with(Path::new("patterns/pr-bot/workflow.toml"))
        });
    let explicit = vars.iter().any(|entry| {
        entry
            .split_once('=')
            .is_some_and(|(key, value)| key == PR_BOT_PROVIDER_VAR && !value.trim().is_empty())
    });
    if !is_pr_bot || explicit {
        return None;
    }

    let provider = parent_provider?;
    vars.push(format!("{PR_BOT_PROVIDER_VAR}={provider}"));
    Some(provider.to_string())
}
