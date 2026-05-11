use csa_config::MergedConfig;

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
