use anyhow::{Context, Result, bail};
use csa_config::{MergedConfig, ProjectConfig};

pub(crate) fn resolve_gh_env(config: &MergedConfig) -> Option<(String, String)> {
    config
        .resolved_github_config_dir()
        .map(|value| ("GH_CONFIG_DIR".to_string(), value))
}

pub(crate) fn is_auth_error(stderr: &str) -> bool {
    let normalized = stderr.to_ascii_lowercase();
    [
        "could not resolve to a repository",
        "repository not found",
        "permission denied",
        "authentication failed",
        "http 404",
        "http 403",
    ]
    .iter()
    .any(|pattern| normalized.contains(pattern))
}

// --- GitHub issue fetch (backing `csa plan run --issue <N>`) ---

/// Fetch a GitHub issue body via `gh issue view`, resolving the project's
/// configured `GH_CONFIG_DIR` (defaulting to the `~/.config/gh-aider` dir) and
/// retrying with default `gh` auth when the configured auth hits a
/// permission/not-found error.
///
/// The auth-fallback chain is: configured `GH_CONFIG_DIR` → on auth error,
/// retry with `GH_CONFIG_DIR` removed (default auth). Any non-auth failure, or
/// a failure of both attempts, is propagated as an error so the caller fails
/// loudly rather than feeding an empty issue body into the workflow.
pub(crate) async fn fetch_issue_body(issue: u64) -> Result<String> {
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let merged_config = ProjectConfig::load(&cwd)
        .context("Failed to load project config while resolving GitHub auth")?;
    let configured_env = merged_config.as_ref().and_then(resolve_gh_env).or_else(|| {
        ProjectConfig::default_github_config_dir().map(|dir| ("GH_CONFIG_DIR".to_string(), dir))
    });
    fetch_issue_body_with_retry(issue, configured_env.as_ref()).await
}

async fn fetch_issue_body_with_retry(
    issue: u64,
    configured_env: Option<&(String, String)>,
) -> Result<String> {
    let issue_arg = issue.to_string();
    let mut command = tokio::process::Command::new("gh");
    if let Some((key, value)) = configured_env {
        command.env(key, value);
    }
    let output = command
        .args([
            "issue",
            "view",
            issue_arg.as_str(),
            "--json",
            "body",
            "-q",
            ".body",
        ])
        .output()
        .await
        .with_context(|| format!("Failed to run gh issue view {issue}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if configured_env.is_some() && is_auth_error(&stderr) {
            let mut retry_command = tokio::process::Command::new("gh");
            let retry_output = retry_command
                .env_remove("GH_CONFIG_DIR")
                .args([
                    "issue",
                    "view",
                    issue_arg.as_str(),
                    "--json",
                    "body",
                    "-q",
                    ".body",
                ])
                .output()
                .await
                .with_context(|| {
                    format!("Failed to retry gh issue view {issue} with default auth")
                })?;
            if retry_output.status.success() {
                return Ok(String::from_utf8_lossy(&retry_output.stdout)
                    .trim_end_matches(['\r', '\n'])
                    .to_string());
            }
            let retry_stderr = String::from_utf8_lossy(&retry_output.stderr);
            bail!(
                "gh issue view {issue} failed with configured auth: {}; retry with default auth also failed: {}",
                stderr.trim(),
                retry_stderr.trim()
            );
        }
        bail!("gh issue view {issue} failed: {}", stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::MergedConfig;

    #[test]
    fn resolve_gh_env_uses_merged_config_dir() {
        let merged: MergedConfig = toml::from_str(
            r#"
[github]
config_dir = "/tmp/project-gh"
"#,
        )
        .expect("deserialize merged config");

        assert_eq!(
            resolve_gh_env(&merged),
            Some(("GH_CONFIG_DIR".to_string(), "/tmp/project-gh".to_string()))
        );
    }

    #[test]
    fn is_auth_error_matches_permission_patterns_case_insensitively() {
        assert!(is_auth_error("HTTP 403 Forbidden"));
        assert!(is_auth_error("Repository not found"));
        assert!(is_auth_error("authentication failed for repo"));
    }

    #[test]
    fn is_auth_error_ignores_non_auth_failures() {
        assert!(!is_auth_error("gh: command not found"));
        assert!(!is_auth_error("unknown flag: --json"));
        assert!(!is_auth_error(
            "network timeout while contacting api.github.com"
        ));
    }
}
