use std::path::Path;

use anyhow::{Result, bail};

pub(super) const PR_BOT_PROVIDER_VAR: &str = "CSA_MODEL_PROVIDER";

pub(super) fn native_parent_provider(
    parent_tool: Option<&str>,
    startup_env: &crate::startup_env::StartupSubtreeEnv,
    hermes_provider: Option<&str>,
) -> Option<String> {
    match parent_tool {
        Some("hermes") => hermes_provider.map(str::to_string),
        Some("codex" | "claude-code" | "opencode" | "antigravity-cli") => {
            crate::daemon_caller_hints::explicit_wait_provider_from_launch_routing(
                None,
                startup_env,
            )
            .map(|provider| provider.as_str().to_string())
        }
        _ => None,
    }
}

pub(super) fn inject_pr_bot_parent_provider(
    file: &Option<String>,
    pattern: &Option<String>,
    vars: &mut Vec<String>,
    parent_provider: Option<&str>,
) -> Result<Option<String>> {
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
        return Ok(None);
    }

    let Some(provider) = parent_provider else {
        bail!(
            "pr-bot requires the calling parent provider before execution; pass \
             --var {PR_BOT_PROVIDER_VAR}=<configured provider> when live parent routing cannot \
             supply a trusted model spec"
        );
    };
    vars.push(format!("{PR_BOT_PROVIDER_VAR}={provider}"));
    Ok(Some(provider.to_string()))
}
