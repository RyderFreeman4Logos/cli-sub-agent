use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use csa_config::ProjectConfig;
use csa_session::{ResourceResolutionInfo, ResourceValueSource, SourcedResourceValue};
use serde::{Deserialize, Serialize};

pub(crate) const INHERITED_RESOURCE_OVERRIDES_ENV: &str = "CSA_INHERITED_RESOURCE_OVERRIDES";

static INHERITED_RESOURCE_OVERRIDES: OnceLock<InheritedResourceOverrides> = OnceLock::new();

/// Explicit resource values admitted at this command boundary.
///
/// The snapshot never contains config/default values. It keeps the immediate
/// parent's explicit value separately so child metadata can distinguish an
/// inherited value from the final value after child CLI precedence is applied.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunResourceOverrides {
    memory_max_mb: Option<u64>,
    min_free_memory_mb: Option<u64>,
    memory_max_mb_source: Option<ResourceValueSource>,
    min_free_memory_mb_source: Option<ResourceValueSource>,
    inherited_memory_max_mb: Option<u64>,
    inherited_min_free_memory_mb: Option<u64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InheritedResourceOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_max_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_free_memory_mb: Option<u64>,
}

/// Freeze the inherited resource contract once at process ingress.
pub(crate) fn initialize_inherited_resource_overrides(internal_invocation: bool) -> Result<()> {
    let inherited = inherited_resource_overrides_from_raw(
        std::env::var_os(INHERITED_RESOURCE_OVERRIDES_ENV),
        internal_invocation,
    )?;

    if let Some(existing) = INHERITED_RESOURCE_OVERRIDES.get() {
        if *existing != inherited {
            bail!("inherited resource override snapshot was already initialized");
        }
        return Ok(());
    }
    INHERITED_RESOURCE_OVERRIDES
        .set(inherited)
        .map_err(|_| anyhow::anyhow!("failed to initialize inherited resource overrides"))
}

fn inherited_resource_overrides_from_raw(
    raw: Option<std::ffi::OsString>,
    internal_invocation: bool,
) -> Result<InheritedResourceOverrides> {
    match raw {
        None => Ok(InheritedResourceOverrides::default()),
        Some(raw) => {
            if !internal_invocation {
                bail!(
                    "{INHERITED_RESOURCE_OVERRIDES_ENV} is reserved for internal CSA child invocations"
                );
            }
            let raw = raw.into_string().map_err(|_| {
                anyhow::anyhow!("{INHERITED_RESOURCE_OVERRIDES_ENV} must contain valid UTF-8 JSON")
            })?;
            parse_inherited_resource_overrides(&raw)
        }
    }
}

fn parse_inherited_resource_overrides(raw: &str) -> Result<InheritedResourceOverrides> {
    let inherited: InheritedResourceOverrides = serde_json::from_str(raw).with_context(|| {
        format!("invalid {INHERITED_RESOURCE_OVERRIDES_ENV} resource override contract")
    })?;
    if inherited.memory_max_mb.is_some_and(|value| value < 256) {
        bail!("invalid {INHERITED_RESOURCE_OVERRIDES_ENV}: memory_max_mb must be at least 256");
    }
    Ok(inherited)
}

impl RunResourceOverrides {
    pub(crate) fn new(memory_max_mb: Option<u64>, min_free_memory_mb: Option<u64>) -> Self {
        Self::from_cli_and_parent(
            memory_max_mb,
            min_free_memory_mb,
            INHERITED_RESOURCE_OVERRIDES
                .get()
                .copied()
                .unwrap_or_default(),
        )
    }

    fn from_cli_and_parent(
        memory_max_mb: Option<u64>,
        min_free_memory_mb: Option<u64>,
        parent: InheritedResourceOverrides,
    ) -> Self {
        let memory_max_mb_source = memory_max_mb
            .map(|_| ResourceValueSource::ExplicitCli)
            .or_else(|| {
                parent
                    .memory_max_mb
                    .map(|_| ResourceValueSource::InheritedParentExplicit)
            });
        let min_free_memory_mb_source = min_free_memory_mb
            .map(|_| ResourceValueSource::ExplicitCli)
            .or_else(|| {
                parent
                    .min_free_memory_mb
                    .map(|_| ResourceValueSource::InheritedParentExplicit)
            });
        Self {
            memory_max_mb: memory_max_mb.or(parent.memory_max_mb),
            min_free_memory_mb: min_free_memory_mb.or(parent.min_free_memory_mb),
            memory_max_mb_source,
            min_free_memory_mb_source,
            inherited_memory_max_mb: parent.memory_max_mb,
            inherited_min_free_memory_mb: parent.min_free_memory_mb,
        }
    }

    /// Convert this command's explicit ancestry into the next child's contract.
    pub(crate) fn for_child(self) -> Self {
        let inherited_memory_max_mb = self.memory_max_mb;
        let inherited_min_free_memory_mb = self.min_free_memory_mb;
        Self {
            memory_max_mb: inherited_memory_max_mb,
            min_free_memory_mb: inherited_min_free_memory_mb,
            memory_max_mb_source: inherited_memory_max_mb
                .map(|_| ResourceValueSource::InheritedParentExplicit),
            min_free_memory_mb_source: inherited_min_free_memory_mb
                .map(|_| ResourceValueSource::InheritedParentExplicit),
            inherited_memory_max_mb,
            inherited_min_free_memory_mb,
        }
    }

    /// Prefer current boundary values and fall back to the persisted plan snapshot.
    pub(crate) fn with_resume_fallback(self, persisted: Self) -> Self {
        Self {
            memory_max_mb: self.memory_max_mb.or(persisted.memory_max_mb),
            min_free_memory_mb: self.min_free_memory_mb.or(persisted.min_free_memory_mb),
            memory_max_mb_source: self.memory_max_mb_source.or(persisted.memory_max_mb_source),
            min_free_memory_mb_source: self
                .min_free_memory_mb_source
                .or(persisted.min_free_memory_mb_source),
            inherited_memory_max_mb: self
                .inherited_memory_max_mb
                .or(persisted.inherited_memory_max_mb),
            inherited_min_free_memory_mb: self
                .inherited_min_free_memory_mb
                .or(persisted.inherited_min_free_memory_mb),
        }
    }

    pub(crate) fn child_env_value(self) -> Result<Option<String>> {
        let inherited = InheritedResourceOverrides {
            memory_max_mb: self.memory_max_mb,
            min_free_memory_mb: self.min_free_memory_mb,
        };
        if inherited == InheritedResourceOverrides::default() {
            return Ok(None);
        }
        serde_json::to_string(&inherited)
            .map(Some)
            .context("failed to serialize inherited resource overrides")
    }

    pub(crate) fn apply_to_child_env(
        self,
        env: &mut std::collections::HashMap<String, String>,
    ) -> Result<()> {
        env.remove(INHERITED_RESOURCE_OVERRIDES_ENV);
        if let Some(value) = self.child_env_value()? {
            env.insert(INHERITED_RESOURCE_OVERRIDES_ENV.to_string(), value);
        }
        Ok(())
    }

    pub(crate) const fn has_memory_max_override(self) -> bool {
        self.memory_max_mb.is_some()
    }

    pub(crate) fn resolve_memory_max_mb(
        self,
        config: Option<&ProjectConfig>,
        tool_name: &str,
    ) -> Option<u64> {
        self.resolve_memory_max_mb_with_source(config, tool_name)
            .map(|resolved| resolved.value)
    }

    pub(crate) fn resolve_min_free_memory_mb(self, config: Option<&ProjectConfig>) -> u64 {
        self.resolve_min_free_memory_mb_with_source(config).value
    }

    pub(crate) fn resolution_info(
        self,
        config: Option<&ProjectConfig>,
        tool_name: &str,
    ) -> ResourceResolutionInfo {
        ResourceResolutionInfo {
            inherited_memory_max_mb: self.inherited_memory_max_mb.map(|value| {
                SourcedResourceValue {
                    value,
                    source: ResourceValueSource::InheritedParentExplicit,
                }
            }),
            effective_memory_max_mb: self.resolve_memory_max_mb_with_source(config, tool_name),
            inherited_min_free_memory_mb: self.inherited_min_free_memory_mb.map(|value| {
                SourcedResourceValue {
                    value,
                    source: ResourceValueSource::InheritedParentExplicit,
                }
            }),
            effective_min_free_memory_mb: Some(self.resolve_min_free_memory_mb_with_source(config)),
        }
    }

    fn resolve_memory_max_mb_with_source(
        self,
        config: Option<&ProjectConfig>,
        tool_name: &str,
    ) -> Option<SourcedResourceValue> {
        if let (Some(value), Some(source)) = (self.memory_max_mb, self.memory_max_mb_source) {
            return Some(SourcedResourceValue { value, source });
        }
        if let Some(value) = config.and_then(|cfg| cfg.sandbox_memory_max_mb(tool_name)) {
            return Some(SourcedResourceValue {
                value,
                source: ResourceValueSource::Configuration,
            });
        }
        csa_config::default_sandbox_for_tool(tool_name)
            .memory_max_mb
            .map(|value| SourcedResourceValue {
                value,
                source: ResourceValueSource::ToolDefault,
            })
    }

    fn resolve_min_free_memory_mb_with_source(
        self,
        config: Option<&ProjectConfig>,
    ) -> SourcedResourceValue {
        if let (Some(value), Some(source)) =
            (self.min_free_memory_mb, self.min_free_memory_mb_source)
        {
            return SourcedResourceValue { value, source };
        }
        if let Some(config) = config {
            return SourcedResourceValue {
                value: config.resources.min_free_memory_mb,
                source: ResourceValueSource::Configuration,
            };
        }
        SourcedResourceValue {
            value: csa_config::ResourcesConfig::default().min_free_memory_mb,
            source: ResourceValueSource::DocumentedDefault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_cli_override_wins_and_preserves_inherited_provenance() {
        let parent = parse_inherited_resource_overrides(
            r#"{"memory_max_mb":17000,"min_free_memory_mb":2048}"#,
        )
        .expect("parent contract should parse");
        let overrides = RunResourceOverrides::from_cli_and_parent(Some(12_000), None, parent);
        let info = overrides.resolution_info(None, "codex");

        assert_eq!(overrides.resolve_memory_max_mb(None, "codex"), Some(12_000));
        assert_eq!(
            info.inherited_memory_max_mb,
            Some(SourcedResourceValue {
                value: 17_000,
                source: ResourceValueSource::InheritedParentExplicit,
            })
        );
        assert_eq!(
            info.effective_memory_max_mb,
            Some(SourcedResourceValue {
                value: 12_000,
                source: ResourceValueSource::ExplicitCli,
            })
        );
        assert_eq!(overrides.resolve_min_free_memory_mb(None), 2048);
    }

    #[test]
    fn no_parent_override_preserves_config_and_tool_default_resolution() {
        let cfg: ProjectConfig = toml::from_str("[resources]\nmemory_max_mb = 8192\n").unwrap();
        let overrides = RunResourceOverrides::from_cli_and_parent(
            None,
            None,
            InheritedResourceOverrides::default(),
        );

        assert_eq!(
            overrides.resolve_memory_max_mb(Some(&cfg), "codex"),
            Some(8192)
        );
        assert_eq!(overrides.resolve_memory_max_mb(None, "codex"), Some(16_384));
        assert!(overrides.child_env_value().unwrap().is_none());
    }

    #[test]
    fn inherited_contract_rejects_malformed_and_unsupported_fields() {
        assert!(parse_inherited_resource_overrides("not-json").is_err());
        assert!(
            parse_inherited_resource_overrides(
                r#"{"memory_max_mb":17000,"unsupported_cpu_max":2}"#
            )
            .is_err()
        );
        assert!(parse_inherited_resource_overrides(r#"{"memory_max_mb":255}"#).is_err());
        assert!(
            inherited_resource_overrides_from_raw(
                Some(r#"{"memory_max_mb":17000}"#.into()),
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn resume_cli_override_wins_over_persisted_snapshot() {
        let persisted = RunResourceOverrides::from_cli_and_parent(
            Some(17_000),
            Some(2048),
            InheritedResourceOverrides::default(),
        );
        let resumed = RunResourceOverrides::from_cli_and_parent(
            Some(12_000),
            None,
            InheritedResourceOverrides::default(),
        )
        .with_resume_fallback(persisted);

        assert_eq!(resumed.resolve_memory_max_mb(None, "codex"), Some(12_000));
        assert_eq!(resumed.resolve_min_free_memory_mb(None), 2048);
    }
}
