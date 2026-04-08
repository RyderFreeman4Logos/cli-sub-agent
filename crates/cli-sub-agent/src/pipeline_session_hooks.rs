use csa_config::ProjectConfig;

pub(crate) fn build_project_hook_overrides(
    config: Option<&ProjectConfig>,
    task_type: Option<&str>,
) -> Option<std::collections::HashMap<String, csa_hooks::HookConfig>> {
    let config = config.filter(|c| !c.hooks.is_default())?;
    let mut overrides = std::collections::HashMap::new();
    if let Some(ref cmd) = config.hooks.pre_run {
        overrides.insert(
            "pre_run".to_string(),
            csa_hooks::HookConfig {
                enabled: true,
                command: Some(cmd.clone()),
                timeout_secs: config.hooks.timeout_secs,
                fail_policy: csa_hooks::FailPolicy::default(),
                waivers: Vec::new(),
            },
        );
    }
    if matches!(task_type, Some("run"))
        && let Some(ref cmd) = config.hooks.post_run
    {
        overrides.insert(
            "post_run".to_string(),
            csa_hooks::HookConfig {
                enabled: true,
                command: Some(cmd.clone()),
                timeout_secs: config.hooks.timeout_secs,
                fail_policy: csa_hooks::FailPolicy::default(),
                waivers: Vec::new(),
            },
        );
    }
    Some(overrides)
}

#[cfg(test)]
mod tests {
    use super::build_project_hook_overrides;
    use csa_config::ProjectConfig;

    fn project_config_with_hooks() -> ProjectConfig {
        toml::from_str(
            r#"
[hooks]
pre_run = "echo pre"
post_run = "echo post"
timeout_secs = 42
"#,
        )
        .expect("project config with hooks")
    }

    #[test]
    fn build_project_hook_overrides_keeps_post_run_for_run_only() {
        let config = project_config_with_hooks();

        let run_overrides =
            build_project_hook_overrides(Some(&config), Some("run")).expect("run overrides");
        let review_overrides =
            build_project_hook_overrides(Some(&config), Some("review")).expect("review overrides");
        let debate_overrides =
            build_project_hook_overrides(Some(&config), Some("debate")).expect("debate overrides");
        let unknown_overrides =
            build_project_hook_overrides(Some(&config), None).expect("unknown overrides");

        assert!(run_overrides.contains_key("pre_run"));
        assert!(run_overrides.contains_key("post_run"));
        assert!(review_overrides.contains_key("pre_run"));
        assert!(!review_overrides.contains_key("post_run"));
        assert!(debate_overrides.contains_key("pre_run"));
        assert!(!debate_overrides.contains_key("post_run"));
        assert!(unknown_overrides.contains_key("pre_run"));
        assert!(!unknown_overrides.contains_key("post_run"));
    }
}
