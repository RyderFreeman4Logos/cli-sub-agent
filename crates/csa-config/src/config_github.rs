use std::path::Path;

use crate::config::{ProjectConfig, read_optional_toml};
use crate::config_merge::merge_toml_values;

impl ProjectConfig {
    /// Resolve `[github].config_dir` from the merged user/project config view.
    ///
    /// Resolution order:
    /// 1. project `.csa/config.toml`
    /// 2. user `~/.config/cli-sub-agent/config.toml`
    /// 3. `~/.config/gh-aider`
    pub fn resolve_github_config_dir(project_root: &Path) -> Option<String> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = Self::user_config_path();
        Self::resolve_github_config_dir_with_paths(user_path.as_deref(), &project_path)
    }

    pub(crate) fn resolve_github_config_dir_with_paths(
        user_path: Option<&Path>,
        project_path: &Path,
    ) -> Option<String> {
        let project_raw = read_optional_toml(project_path, "project");
        let user_raw = user_path.and_then(|path| read_optional_toml(path, "user"));
        let merged = match (user_raw, project_raw) {
            (Some(base), Some(overlay)) => merge_toml_values(base, overlay),
            (Some(base), None) => base,
            (None, Some(overlay)) => overlay,
            (None, None) => toml::Value::Table(toml::map::Map::new()),
        };

        parse_github_config_dir(&merged).or_else(Self::default_github_config_dir)
    }

    /// Resolve the configured GitHub CLI auth directory from the merged config view.
    pub fn resolved_github_config_dir(&self) -> Option<String> {
        self.github
            .as_ref()
            .and_then(|config| config.config_dir.clone())
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
}

fn parse_github_config_dir(raw: &toml::Value) -> Option<String> {
    raw.get("github")
        .and_then(|github| github.get("config_dir"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}
