use std::path::Path;

use anyhow::Result;

use crate::config::ProjectConfig;

impl ProjectConfig {
    /// Resolve `[github].config_dir` from the merged user/project config view.
    ///
    /// Resolution order:
    /// 1. project `.csa/config.toml`
    /// 2. user `~/.config/cli-sub-agent/config.toml`
    /// 3. `~/.config/gh-aider`
    pub fn resolve_github_config_dir(project_root: &Path) -> Result<Option<String>> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = Self::user_config_path();
        Self::resolve_github_config_dir_with_paths(user_path.as_deref(), &project_path)
    }

    pub(crate) fn resolve_github_config_dir_with_paths(
        user_path: Option<&Path>,
        project_path: &Path,
    ) -> Result<Option<String>> {
        Ok(Self::load_with_paths(user_path, project_path)?
            .as_ref()
            .and_then(Self::configured_github_config_dir)
            .or_else(Self::default_github_config_dir))
    }

    /// Resolve the configured GitHub CLI auth directory from the merged config view.
    pub fn resolved_github_config_dir(&self) -> Option<String> {
        self.configured_github_config_dir()
            .or_else(Self::default_github_config_dir)
    }

    pub fn default_github_config_dir() -> Option<String> {
        directories::BaseDirs::new().map(|dirs| {
            dirs.home_dir()
                .join(".config")
                .join("gh-aider")
                .to_string_lossy()
                .into_owned()
        })
    }

    fn configured_github_config_dir(&self) -> Option<String> {
        self.github
            .as_ref()
            .and_then(|config| config.config_dir.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
}
