use csa_config::ProjectConfig;

/// Per-run resource overrides resolved from `csa run` CLI flags.
///
/// These values deliberately do not mutate [`ProjectConfig`]. They apply only to
/// the current run and have normal CLI precedence over project/global config.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunResourceOverrides {
    pub(crate) memory_max_mb: Option<u64>,
    pub(crate) min_free_memory_mb: Option<u64>,
}

impl RunResourceOverrides {
    pub(crate) const fn new(memory_max_mb: Option<u64>, min_free_memory_mb: Option<u64>) -> Self {
        Self {
            memory_max_mb,
            min_free_memory_mb,
        }
    }

    pub(crate) fn resolve_memory_max_mb(
        self,
        config: Option<&ProjectConfig>,
        tool_name: &str,
    ) -> Option<u64> {
        self.memory_max_mb
            .or_else(|| config.and_then(|cfg| cfg.sandbox_memory_max_mb(tool_name)))
            .or_else(|| csa_config::default_sandbox_for_tool(tool_name).memory_max_mb)
    }

    pub(crate) fn resolve_min_free_memory_mb(self, config: Option<&ProjectConfig>) -> u64 {
        let default_resources = csa_config::ResourcesConfig::default();
        self.min_free_memory_mb
            .or_else(|| config.map(|cfg| cfg.resources.min_free_memory_mb))
            .unwrap_or(default_resources.min_free_memory_mb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_max_override_takes_precedence_over_tool_config() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[tools.codex]
memory_max_mb = 16384
"#,
        )
        .expect("config should parse");

        let overrides = RunResourceOverrides::new(Some(6144), None);

        assert_eq!(
            overrides.resolve_memory_max_mb(Some(&cfg), "codex"),
            Some(6144)
        );
    }

    #[test]
    fn memory_max_falls_back_to_config_then_tool_default() {
        let cfg: ProjectConfig =
            toml::from_str("[resources]\nmemory_max_mb = 8192\n").expect("config should parse");
        let overrides = RunResourceOverrides::default();

        assert_eq!(
            overrides.resolve_memory_max_mb(Some(&cfg), "codex"),
            Some(8192)
        );
        assert_eq!(overrides.resolve_memory_max_mb(None, "codex"), Some(12_288));
    }

    #[test]
    fn min_free_override_takes_precedence_over_config() {
        let cfg: ProjectConfig = toml::from_str("[resources]\nmin_free_memory_mb = 4096\n")
            .expect("config should parse");
        let overrides = RunResourceOverrides::new(None, Some(512));

        assert_eq!(overrides.resolve_min_free_memory_mb(Some(&cfg)), 512);
    }
}
